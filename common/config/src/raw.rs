use crate::error::ConfigError;
use crate::{argv, get_hostname, properties};
use http::types::params::Params;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone)]
pub struct Config {
    pub http: HttpConfig,
    pub log: LogConfig,
    pub journald: JournaldConfig,
}

impl Config {
    /// Tries to parse from java properties format and then using
    pub fn parse<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let is_default_path = path.to_string_lossy() == argv::DEFAULT_YAML_FILE;
        let mut file = File::open(path).or_else(|e| {
            if !is_default_path {
                // A path was configured but the file was NOT found
                error!(
                    "config file was configured in {} but it could not be opened",
                    path.display()
                );
                Err(e)
            } else {
                File::open(argv::DEFAULT_CONF_FILE)
            }
        })?;

        // Everything is a yaml, so validation usually passes with java properties
        // We have to validate first that is a java properties file
        match properties::read_file(&file) {
            Ok(c) => {
                debug!("config is a valid properties file");
                return Ok(c);
            }
            Err(e) => {
                debug!("config file is not a valid properties file: {:?}", e);
            }
        }

        debug!("loading config file as yaml");
        file.seek(SeekFrom::Start(0))?;
        Ok(serde_yaml::from_reader(&file)?)
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

    // Mostly for development, these settings are hidden from the user
    // There's no guarantee that these settings will exist in the future
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_base_delay_ms: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_step_delay_ms: Option<usize>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone)]
pub struct LogConfig {
    pub dirs: Vec<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub db_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include: Option<Rules>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude: Option<Rules>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_exclusion_regex: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_inclusion_regex: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_redact_regex: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lookback: Option<String>,
    pub use_k8s_enrichment: Option<String>,
    pub log_k8s_events: Option<String>,
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

impl Default for Rules {
    fn default() -> Self {
        Rules {
            glob: Vec::new(),
            regex: Vec::new(),
        }
    }
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
            retry_base_delay_ms: None,
            retry_step_delay_ms: None,
        }
    }
}

impl Default for LogConfig {
    fn default() -> Self {
        LogConfig {
            dirs: vec!["/var/log/".into()],
            db_path: None,
            metrics_port: None,
            include: Some(Rules {
                glob: vec!["*.log".parse().unwrap()],
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
            line_exclusion_regex: None,
            line_inclusion_regex: None,
            line_redact_regex: None,
            lookback: None,
            use_k8s_enrichment: None,
            log_k8s_events: None,
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
    use http::types::params::Tags;
    use std::fs;
    use std::io;
    use tempfile::tempdir;

    macro_rules! vec_strings {
        ($($str:expr),*) => ({
            vec![$(String::from($str),)*] as Vec<String>
        });
    }

    macro_rules! some_string {
        ($val: expr) => {
            Some($val.to_string())
        };
    }

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

    #[test]
    fn test_properties_file_basic() -> io::Result<()> {
        let dir = tempdir().unwrap().into_path();
        let file_name = dir.join("test.conf");
        fs::write(
            &file_name,
            "key = 123
tags = production,2ndtag
exclude = /path/to/exclude/**",
        )?;
        let config = Config::parse(&file_name).unwrap();
        // Defaults to /var/log
        assert_eq!(config.log.dirs, vec![PathBuf::from("/var/log")]);
        assert_eq!(config.http.ingestion_key, some_string!("123"));
        assert_eq!(
            config.http.params.unwrap().tags,
            Some(Tags::from(vec_strings!["production", "2ndtag"]))
        );
        let expected_exclude = LogConfig::default()
            .exclude
            .unwrap()
            .glob
            .iter()
            .map(|x| x.to_string())
            .chain(vec_strings!["/path/to/exclude/**"])
            .collect::<Vec<String>>();
        assert_eq!(
            config.log.exclude,
            Some(Rules {
                glob: expected_exclude,
                regex: vec![]
            })
        );
        Ok(())
    }

    #[test]
    fn test_properties_overrides_log_dir() -> io::Result<()> {
        let dir = tempdir()?;
        let file_name = dir.path().join("test.conf");
        fs::write(&file_name, "key = 567\nlogdir = /path/to/my/logs")?;
        let config = Config::parse(&file_name).unwrap();
        // Overrides the default
        assert_eq!(config.log.dirs, vec![PathBuf::from("/path/to/my/logs")]);
        assert_eq!(config.http.ingestion_key, some_string!("567"));
        Ok(())
    }

    #[test]
    fn test_properties_log_dir_defaults_to_var_log() -> io::Result<()> {
        let dir = tempdir()?;
        let file_name = dir.path().join("test.conf");
        fs::write(&file_name, "key = 890")?;
        let config = Config::parse(&file_name).unwrap();
        // Defaults to /var/log
        assert_eq!(config.log.dirs, vec![PathBuf::from("/var/log")]);
        assert_eq!(config.http.ingestion_key, some_string!("890"));
        Ok(())
    }

    #[test]
    fn test_properties_all_legacy_agent() -> io::Result<()> {
        let dir = tempdir()?;
        let file_name = dir.path().join("test.conf");
        fs::write(
            &file_name,
            "
key = 1234567890
logdir = /var/my_log,/var/my_log2
tags = production, stable
exclude = /path/to/exclude/**, /second/path/to/exclude/**

exclude_regex = /a/regex/exclude.*
hostname = jorge's-laptop
",
        )?;
        let config = Config::parse(&file_name).unwrap();
        assert_eq!(
            config.log.dirs,
            vec![PathBuf::from("/var/my_log"), PathBuf::from("/var/my_log2")]
        );
        assert_eq!(config.http.ingestion_key, some_string!("1234567890"));
        let params = config.http.params.unwrap();
        assert_eq!(
            params.tags,
            Some(Tags::from(vec_strings!["production", "stable"]))
        );
        assert_eq!(params.hostname, "jorge's-laptop");
        let expected_exclude = LogConfig::default()
            .exclude
            .unwrap()
            .glob
            .iter()
            .map(|x| x.to_string())
            .chain(vec_strings![
                "/path/to/exclude/**",
                "/second/path/to/exclude/**"
            ])
            .collect::<Vec<String>>();
        assert_eq!(
            config.log.exclude,
            Some(Rules {
                glob: expected_exclude,
                regex: vec_strings!["/a/regex/exclude.*"]
            })
        );
        Ok(())
    }

    #[test]
    fn test_comments() -> io::Result<()> {
        let dir = tempdir()?;
        let file_name = dir.path().join("test.conf");
        fs::write(
            &file_name,
            "
# My comment
key = abcdef01
# My second comment
# tags = production
",
        )?;
        let config = Config::parse(&file_name).unwrap();
        // Defaults to /var/log
        assert_eq!(config.log.dirs, vec![PathBuf::from("/var/log")]);
        assert_eq!(config.http.ingestion_key, some_string!("abcdef01"));
        assert_eq!(config.http.params.unwrap().tags, None);
        Ok(())
    }

    #[test]
    fn test_file_not_found() {
        assert!(matches!(
            Config::parse("/non/existent/path.conf"),
            Err(ConfigError::Io(_))
        ));
    }

    #[test]
    fn test_empty() -> io::Result<()> {
        let dir = tempdir()?;
        let file_name = dir.path().join("test.conf");
        fs::write(&file_name, "")?;
        assert!(matches!(
            Config::parse(&file_name),
            Err(ConfigError::Serde(_))
        ));
        Ok(())
    }

    #[test]
    fn test_invalid_yaml_format() -> io::Result<()> {
        let dir = tempdir()?;
        let file_name = dir.path().join("test.yaml");
        fs::write(&file_name, "SOMEPROPERTY::: AZSZ")?;
        assert!(matches!(
            Config::parse(&file_name),
            Err(ConfigError::Serde(_))
        ));
        Ok(())
    }

    #[test]
    fn test_invalid_yaml() -> io::Result<()> {
        let dir = tempdir()?;
        let file_name = dir.path().join("test.yaml");
        fs::write(&file_name, "http: true")?;
        assert!(matches!(
            Config::parse(&file_name),
            Err(ConfigError::Serde(_))
        ));
        Ok(())
    }

    #[test]
    fn test_yaml_file_properties() -> io::Result<()> {
        let dir = tempdir()?;
        let file_name = dir.path().join("test.yml");
        fs::write(
            &file_name,
            "
http:
  host: logs.logdna.prod
  endpoint: /path/to/endpoint1
  use_ssl: false
  timeout: 12000
  use_compression: true
  gzip_level: 1
  params:
    hostname: abc
    tags: tag1,tag2
    now: 0
  body_size: 2097152
log:
  dirs:
    - /var/log1/
    - /var/log2/
journald: {}
",
        )?;

        let config = Config::parse(&file_name).unwrap();
        assert_eq!(config.http.use_ssl, Some(false));
        assert_eq!(config.http.use_compression, Some(true));
        assert_eq!(config.http.host, some_string!("logs.logdna.prod"));
        assert_eq!(config.http.endpoint, some_string!("/path/to/endpoint1"));
        assert_eq!(config.http.use_compression, Some(true));
        assert_eq!(config.http.timeout, Some(12000));
        let params = config.http.params.unwrap();
        assert_eq!(params.tags, Some(Tags::from("tag1,tag2")));
        assert_eq!(
            config.log.dirs,
            vec![PathBuf::from("/var/log1/"), PathBuf::from("/var/log2/")]
        );
        Ok(())
    }

    #[test]
    fn test_all_new() -> io::Result<()> {
        let dir = tempdir()?;
        let file_name = dir.path().join("test.conf");
        fs::write(
            &file_name,
            "key = 1234567890
endpoint = /endpoint
host = my-host
use_ssl = true
use_compression = false
gzip_level = 4
ip = 10.10.10.8
mac = 00:A0:C9:14:C8:29
lookback = start
db_path = /var/lib/my-dir
metrics_port = 8901
use_k8s_log_enrichment = never
log_k8s_events = always
journald_paths = /first-j, /second-j/a
inclusion_rules = /a/glob/include/**/*
inclusion_regex_rules = /a/regex/include/.*
line_exclusion_regex = a.*, b.*
line_inclusion_regex = c.+
# needs escaping in java properties
redact_regex = \\\\S+@\\\\S+\\\\.\\\\S+",
        )?;
        let config = Config::parse(&file_name).unwrap();

        assert_eq!(config.http.use_compression, Some(false));
        assert_eq!(config.http.use_ssl, Some(true));
        assert_eq!(config.http.gzip_level, Some(4));

        let params = config.http.params.unwrap();
        assert_eq!(params.ip, some_string!("10.10.10.8"));
        assert_eq!(params.mac, some_string!("00:A0:C9:14:C8:29"));

        assert_eq!(config.log.lookback, some_string!("start"));
        assert_eq!(config.log.db_path, Some(PathBuf::from("/var/lib/my-dir")));
        assert_eq!(config.log.metrics_port, Some(8901));
        assert_eq!(config.log.use_k8s_enrichment, some_string!("never"));
        assert_eq!(config.log.log_k8s_events, some_string!("always"));
        assert_eq!(
            config.journald.paths,
            Some(vec![
                PathBuf::from("/first-j"),
                PathBuf::from("/second-j/a")
            ])
        );

        let expected_include = LogConfig::default()
            .include
            .unwrap()
            .glob
            .iter()
            .map(|x| x.to_string())
            .chain(vec_strings!["/a/glob/include/**/*"])
            .collect::<Vec<String>>();
        assert_eq!(
            config.log.include,
            Some(Rules {
                glob: expected_include,
                regex: vec_strings!["/a/regex/include/.*"]
            })
        );
        assert_eq!(
            config.log.line_exclusion_regex,
            Some(vec_strings!["a.*", "b.*"])
        );
        assert_eq!(config.log.line_inclusion_regex, Some(vec_strings!["c.+"]));
        assert_eq!(
            config.log.line_redact_regex,
            Some(vec_strings![r"\S+@\S+\.\S+"])
        );
        Ok(())
    }
}
