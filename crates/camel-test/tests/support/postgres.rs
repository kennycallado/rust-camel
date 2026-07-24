#![allow(dead_code)]

use std::io::Write;

use testcontainers::{
    ContainerAsync, CopyTargetOptions, GenericImage, ImageExt, core::WaitFor, runners::AsyncRunner,
};
use testcontainers_modules::postgres::Postgres;
use tokio::sync::OnceCell;

// ── Non-TLS Postgres ──────────────────────────────────────────────────────

static POSTGRES: OnceCell<(ContainerAsync<Postgres>, String)> = OnceCell::const_new();

/// Shared non-TLS Postgres container, started once and reused across all tests.
/// Returns the connection string
/// (e.g. `postgres://postgres:postgres@127.0.0.1:<port>/postgres`).
pub async fn shared_postgres() -> &'static str {
    POSTGRES
        .get_or_init(|| async {
            super::init_tracing();
            super::install_crypto_provider();
            sqlx::any::install_default_drivers();

            let container = Postgres::default()
                .start()
                .await
                .expect("Postgres container failed to start");
            let port = container
                .get_host_port_ipv4(5432)
                .await
                .expect("Postgres port not available");
            let conn_str = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
            eprintln!("PostgreSQL connection: {conn_str}");
            (container, conn_str)
        })
        .await
        .1
        .as_str()
}

// ── TLS Postgres ──────────────────────────────────────────────────────────

static POSTGRES_TLS: OnceCell<(ContainerAsync<GenericImage>, String)> = OnceCell::const_new();

/// Shared TLS-enabled Postgres container with self-signed certs, started once
/// and reused across all tests. Returns the SSL connection string
/// (e.g. `postgres://...?sslmode=require`).
pub async fn shared_postgres_tls() -> &'static str {
    POSTGRES_TLS
        .get_or_init(|| async {
            super::init_tracing();
            super::install_crypto_provider();
            sqlx::any::install_default_drivers();

            let (_ca_pem, cert_pem, key_pem) =
                camel_component_api::test_support::tls::gen_server_cert();

            let cert_file = tempfile::NamedTempFile::new().unwrap();
            let key_file = tempfile::NamedTempFile::new().unwrap();
            cert_file.as_file().write_all(cert_pem.as_bytes()).unwrap();
            key_file.as_file().write_all(key_pem.as_bytes()).unwrap();
            let cert_path = cert_file.into_temp_path();
            let key_path = key_file.into_temp_path();

            let entrypoint_script = r#"#!/bin/sh
set -e
if [ ! -f $PGDATA/PG_VERSION ]; then
    gosu postgres initdb -D $PGDATA
    gosu postgres pg_ctl -D $PGDATA start -w -o "-c ssl=off"
    gosu postgres psql -c "ALTER USER postgres PASSWORD 'postgres';"
    gosu postgres pg_ctl -D $PGDATA stop -w
    sed -i 's/^host /hostssl /' $PGDATA/pg_hba.conf
    echo "hostssl all all 0.0.0.0/0 md5" >> $PGDATA/pg_hba.conf
    echo "hostssl all all ::0/0 md5" >> $PGDATA/pg_hba.conf
fi
cp /etc/postgresql/tls/server.pem $PGDATA/server.pem
cp /etc/postgresql/tls/server-key.pem $PGDATA/server-key.pem
chown postgres:postgres $PGDATA/server.pem $PGDATA/server-key.pem
chmod 0600 $PGDATA/server-key.pem
gosu postgres postgres \
    -c ssl=on \
    -c ssl_cert_file=$PGDATA/server.pem \
    -c ssl_key_file=$PGDATA/server-key.pem &
PG_PID=$!
until pg_isready -h 127.0.0.1 -p 5432 -U postgres 2>/dev/null; do sleep 0.2; done
echo "SSL_POSTGRES_READY" >&2
wait $PG_PID
"#;
            let entrypoint_file = tempfile::NamedTempFile::new().unwrap();
            entrypoint_file
                .as_file()
                .write_all(entrypoint_script.as_bytes())
                .unwrap();
            let entrypoint_path = entrypoint_file.into_temp_path();

            let image = GenericImage::new("postgres", "16-alpine")
                .with_wait_for(WaitFor::message_on_stderr("SSL_POSTGRES_READY"))
                .with_entrypoint("/usr/local/bin/ssl-entrypoint.sh")
                .with_copy_to(
                    CopyTargetOptions::new("/etc/postgresql/tls/server.pem"),
                    cert_path.to_path_buf(),
                )
                .with_copy_to(
                    CopyTargetOptions::new("/etc/postgresql/tls/server-key.pem"),
                    key_path.to_path_buf(),
                )
                .with_copy_to(
                    CopyTargetOptions::new("/usr/local/bin/ssl-entrypoint.sh").with_mode(0o755),
                    entrypoint_path.to_path_buf(),
                );

            let container = image.start().await.unwrap();
            let port = container.get_host_port_ipv4(5432).await.unwrap();
            let conn_str =
                format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres?sslmode=require");
            eprintln!("PostgreSQL TLS connection: {conn_str}");
            (container, conn_str)
        })
        .await
        .1
        .as_str()
}
