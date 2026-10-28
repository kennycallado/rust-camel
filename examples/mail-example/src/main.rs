//! Mail example for rust-camel.
//!
//! Demonstrates:
//!   - Route 1 (Producer): timer → set_body → to(smtp://...): one mail every 10 s.
//!   - Route 2 (Consumer): from(imap://...) → log: each unread mail, then `\Seen`.
//!
//! Both routes talk to a local GreenMail fixture over cleartext loopback
//! ports, which is why both URIs set `allowCleartext=true`. Start it with:
//!
//!   docker run -d --rm --name greenmail-example \
//!     -p 127.0.0.1:3025:3025 -p 127.0.0.1:3143:3143 \
//!     -e GREENMAIL_OPTS='-Dgreenmail.setup.test.smtp -Dgreenmail.setup.test.imap -Dgreenmail.hostname=0.0.0.0 -Dgreenmail.users=user:pass@local.test' \
//!     greenmail/standalone:2.1.2
//!
//! Then run:
//!
//!   cargo run -p mail-example
//!
//! Press Ctrl+C to stop.
//!
//! The `user`/`pass` login belongs to the ephemeral local fixture. It is not
//! a production secret. A real deployment keeps STARTTLS (the default) and
//! reads the password from `Camel.toml` with an `${env:...}` placeholder.

use std::collections::HashMap;
use std::sync::Arc;

use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_mail::{
    MailAccountConfig, MailComponent, MailComponentConfig, MailScheme, SecretString,
};
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;

/// Fixture login for the local GreenMail container; not a production secret.
fn fixture_account() -> MailAccountConfig {
    MailAccountConfig {
        username: Some("user".to_string()),
        password: Some(SecretString::from("pass")),
        ca_file: None,
    }
}

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    let config = Arc::new(MailComponentConfig {
        accounts: HashMap::from([("demo".to_string(), fixture_account())]),
        reconnect: None,
    });

    let mut ctx = CamelContext::builder().build().await?;
    ctx.register_component(MailComponent::new(MailScheme::Smtp, Arc::clone(&config)));
    ctx.register_component(MailComponent::new(MailScheme::Imap, config));
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // Route 1 (Producer): send one mail every 10 seconds.
    // ANCHOR: mail-producer-route
    let producer = RouteBuilder::from("timer:tick?period=10000")
        .route_id("mail-producer")
        .set_body(Value::String("Hello from rust-camel".to_string()))
        .to("smtp://127.0.0.1:3025?account=demo&allowCleartext=true\
             &from=robot@local.test&to=user@local.test&subject=Greetings")
        .build()?;
    // ANCHOR_END: mail-producer-route

    // Route 2 (Consumer): poll the inbox every 5 seconds and log each mail.
    // ANCHOR: mail-consumer-route
    let consumer =
        RouteBuilder::from("imap://127.0.0.1:3143?account=demo&allowCleartext=true&delay=5000")
            .route_id("mail-consumer")
            .to("log:info?showHeaders=true")
            .build()?;
    // ANCHOR_END: mail-consumer-route

    ctx.add_route_definition(producer).await?;
    ctx.add_route_definition(consumer).await?;

    println!("Starting mail example... Press Ctrl+C to stop.\n");
    ctx.start().await?;

    tokio::signal::ctrl_c().await.ok();
    println!("\nShutting down...");
    ctx.stop().await?;
    println!("Mail example stopped.");
    Ok(())
}
