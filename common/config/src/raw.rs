use serde::{Deserialize, Serialize};

use http::types::params::Params;

use crate::error::ConfigError;
use crate::get_hostname;
use std::fs::File;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone)]
pub struct Config {
    pub http: HttpConfig,
    pub log: LogConfig,
    pub journald: JournaldConfig,
}

impl Config {
    pub fn parse<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        Ok(serde_yaml::from_reader(File::open(path)?)?)
    }
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone)]
pub struct HttpConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_ssl: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_compression: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gzip_level: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ingestion_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Params>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_size: Option<usize>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone)]
pub struct LogConfig {
    pub dirs: Vec<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include: Option<Rules>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude: Option<Rules>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone)]
pub struct JournaldConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<PathBuf>>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone)]
pub struct Rules {
    pub glob: Vec<String>,
    pub regex: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            http: HttpConfig::default(),
            log: LogConfig::default(),
            journald: JournaldConfig::default(),
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
                .hostname(get_hostname().unwrap_or_default())
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
                glob: vec!["*.log".parse().unwrap(), "!(*.*)".parse().unwrap()],
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
                    "/var/log/pods/**/*".parse().unwrap(),
                ],
                regex: Vec::new(),
            }),
        }
    }
}

impl Default for JournaldConfig {
    fn default() -> Self {
        JournaldConfig { paths: None }
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
