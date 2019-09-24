use serde::{Deserialize, Serialize};

use http::types::params::Params;

use crate::get_hostname;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct Config {
    pub http: HttpConfig,
    pub log: LogConfig,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct HttpConfig {
    pub host: Option<String>,
    pub endpoint: Option<String>,
    pub use_ssl: Option<bool>,
    pub timeout: Option<u64>,
    pub use_compression: Option<bool>,
    pub gzip_level: Option<u32>,
    pub ingestion_key: Option<String>,
    pub params: Option<Params>,
    pub body_size: Option<usize>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct LogConfig {
    pub dirs: Vec<PathBuf>,
    pub include: Option<Rules>,
    pub exclude: Option<Rules>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct Rules {
    pub glob: Vec<String>,
    pub regex: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            http: HttpConfig::default(),
            log: LogConfig::default(),
        }
    }
}

impl Default for HttpConfig {
    fn default() -> Self {
        HttpConfig {
            host: Some("logs.logdna.com".to_string()),
            endpoint: Some("/logs/agent".to_string()),
            use_ssl: Some(true),
            timeout: Some(10_000),
            use_compression: Some(true),
            gzip_level: Some(2),
            ingestion_key: None,
            params: Params::builder()
                .hostname(get_hostname().unwrap_or(String::new()))
                .build()
                .ok(),
            body_size: Some(2 * 1024 * 1024),
        }
    }
}

impl Default for LogConfig {
    fn default() -> Self {
        LogConfig {
            dirs: vec!["/var/log/".into()],
            include: Some(Rules {
                glob: vec![
                    "*.log".parse().unwrap(),
                    "!(*.*)".parse().unwrap(),
                ],
                regex: Vec::new(),
            }),
            exclude: Some(Rules {
                glob: vec![
                    "/var/log/wtmp".parse().unwrap(),
                    "/var/log/btmp".parse().unwrap(),
                    "/var/log/utmp".parse().unwrap(),
                    "/var/log/wtmpx".parse().unwrap(),
                    "/var/log/btmpx".parse().unwrap(),
                    "/var/log/utmpx".parse().unwrap(),
                    "/var/log/asl/**".parse().unwrap(),
                    "/var/log/sa/**".parse().unwrap(),
                    "/var/log/sar*".parse().unwrap(),
                    "/var/log/tallylog".parse().unwrap(),
                    "/var/log/fluentd-buffers/**/*".parse().unwrap(),
                ],
                regex: Vec::new(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default() {
        // test for panic at creation
        let config = Config::default();
        // make sure the config can be serialized
        let yaml = serde_yaml::to_string(&config);
        assert!(yaml.is_ok());
        let yaml = yaml.unwrap();
        // make sure the config can be deserialized
        let new_config = serde_yaml::from_str::<Config>(&yaml);
        assert!(new_config.is_ok());
        let new_config = new_config.unwrap();
        assert_eq!(config, new_config);
    }
}
