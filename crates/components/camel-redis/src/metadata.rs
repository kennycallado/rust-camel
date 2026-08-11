use camel_component_api::UriConfig;

#[allow(dead_code)]
#[derive(UriConfig)]
#[uri_scheme = "redis"]
#[uri_config(
    skip_impl,
    metadata(
        scheme = "redis",
        description = "Redis commands / pub-sub",
        producer,
        consumer
    ),
    crate = "camel_component_api"
)]
pub(super) struct RedisMetadataDescriptor {
    #[uri_param(name = "command", default = "SET")]
    pub _command: String,

    #[uri_param(name = "channels")]
    pub _channels: String,

    #[uri_param(name = "key")]
    pub _key: Option<String>,

    #[uri_param(name = "timeout", default = "1")]
    pub _timeout: u64,

    #[uri_param(name = "password", secret)]
    pub _password: String,

    #[uri_param(name = "db", default = "0")]
    pub _db: u8,

    #[uri_param(name = "ssl")]
    pub _ssl: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::ComponentMetadata;

    #[test]
    fn redis_metadata_uri_options_parity() {
        let meta: ComponentMetadata = RedisMetadataDescriptor::metadata();
        let mut names: Vec<&str> = meta.uri_options.iter().map(|o| o.name.as_str()).collect();
        names.sort();

        let mut expected = vec![
            "command", "channels", "key", "timeout", "password", "db", "ssl",
        ];
        expected.sort();

        assert_eq!(
            names, expected,
            "metadata uri_options names must match parser keys"
        );

        // Verify secret flag on password
        let password = meta
            .uri_options
            .iter()
            .find(|o| o.name == "password")
            .unwrap();
        assert!(password.secret, "password must be secret");

        // Verify parser defaults on db and timeout
        let db = meta.uri_options.iter().find(|o| o.name == "db").unwrap();
        assert_eq!(
            db.default_value.as_deref(),
            Some("0"),
            "db must carry parser default 0"
        );

        let timeout = meta
            .uri_options
            .iter()
            .find(|o| o.name == "timeout")
            .unwrap();
        assert_eq!(
            timeout.default_value.as_deref(),
            Some("1"),
            "timeout must carry parser default 1"
        );
    }
}
