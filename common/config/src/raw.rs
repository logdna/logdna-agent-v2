use crate::error::ConfigError;
use crate::{argv, get_hostname};
use http::types::params::{Params, Tags};
use java_properties::PropertiesIter;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Seek, SeekFrom};
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
        let mut file = File::open(path)?;

        // Everything is a yaml, so validation usually passes with java properties
        // We have to validate first that is a java properties file
        let mut prop_map = HashMap::new();
        debug!("loading config file as java properties");
        match PropertiesIter::new(BufReader::new(&file)).read_into(|k, v| {
            prop_map.insert(k, v);
        }) {
            Ok(_) => match from_property_map(&prop_map) {
                Ok(c) => {
                    return Ok(c);
                }
                Err(e) => {
                    debug!("config properties file mapping error: {:?}", e);
                }
            },
            Err(e) => {
                debug!("config file is not a valid properties file: {:?}", e);
            }
        }

        debug!("loading config file as yaml");
        file.seek(SeekFrom::Start(0))?;
        Ok(serde_yaml::from_reader(&file)?)
    }
}

fn from_property_map(map: &HashMap<String, String>) -> Result<Config, ConfigError> {
    if map.is_empty() {
        return Err(ConfigError::MissingField("not implemented"));
    }
    let mut result = Config {
        http: Default::default(),
        log: Default::default(),
        journald: Default::default(),
    };
    result.http.ingestion_key = map.get("key").map(|s| s.to_string());
    let mut params = Params::builder()
        .hostname(get_hostname().unwrap_or_default())
        .build()
        .unwrap();
    params.tags = map.get("tags").map(|s| Tags::from(argv::split_by_comma(s)));

    if let Some(hostname) = map.get("hostname") {
        params.hostname = hostname.to_string();
    }

    result.http.params = Some(params);

    if let Some(log_dirs) = map.get("logdir") {
        // To support the legacy agent behaviour, we override the default (/var/log)
        // This results in a different behaviour depending on the format:
        //   yaml -> append to default
        //   conf -> override default when set
        result.log.dirs = argv::split_by_comma(log_dirs)
            .iter()
            .map(PathBuf::from)
            .collect();
    }

    if let Some(exclude) = map.get("exclude") {
        let rules = result.log.exclude.get_or_insert(Rules::default());
        argv::split_by_comma(exclude)
            .iter()
            .for_each(|v| rules.glob.push(v.to_string()));
    }

    if let Some(exclude_regex) = map.get("exclude_regex") {
        let rules = result.log.exclude.get_or_insert(Rules::default());
        argv::split_by_comma(exclude_regex)
            .iter()
            .for_each(|v| rules.regex.push(v.to_string()));
    }

    Ok(result)
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
        let dir = tempdir().unwrap().into_path();
        let file_name = dir.join("test.conf");
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

    // #[test]
    // fn test_comments() -> io::Result<()> {
    //     Ok(())
    // }
    //
    // #[test]
    // fn test_should_read_default_conf_path() -> io::Result<()> {
    //     Ok(())
    // }
}
