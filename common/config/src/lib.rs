use std::convert::TryFrom;
use std::ffi::CString;
use std::fs::File;
use std::path::PathBuf;
use std::time::Duration;

use flate2::Compression;

use fs::rule::{GlobRule, RegexRule, Rules};
use http::types::params::{Params, Tags};
use http::types::request::{Encoding, RequestTemplate, Schema};

use crate::env::Config as EnvConfig;
use crate::error::ConfigError;
use crate::raw::{Config as RawConfig, Rules as RawRules};
use std::io::Read;

pub mod env;
pub mod error;
pub mod raw;

#[derive(Debug)]
pub struct Config {
    pub http: HttpConfig,
    pub log: LogConfig,
}

#[derive(Debug)]
pub struct HttpConfig {
    pub template: RequestTemplate,
    pub timeout: Duration,
    pub body_size: usize,
}

#[derive(Debug)]
pub struct LogConfig {
    pub dirs: Vec<PathBuf>,
    pub rules: Rules,
}

impl Config {
    pub fn new() -> Result<Self, ConfigError> {
        let env_config: EnvConfig = EnvConfig::parse();
        let raw_config: RawConfig = serde_yaml::from_reader(File::open(&env_config.config_file)?)?;
        Config::try_from((env_config, raw_config))
    }
}

impl TryFrom<(EnvConfig, RawConfig)> for Config {
    type Error = ConfigError;

    fn try_from((env_config, mut raw_config): (EnvConfig, RawConfig)) -> Result<Self, Self::Error> {
        if env_config.host.is_some() {
            raw_config.http.host = env_config.host;
        }

        if env_config.endpoint.is_some() {
            raw_config.http.endpoint = env_config.endpoint;
        }

        raw_config.http.ingestion_key = Some(
            env_config.ingestion_key
                .ok_or(ConfigError::MissingEnvVar(EnvConfig::ingestion_key_vars()))?
        );

        if env_config.use_ssl.is_some() {
            raw_config.http.use_ssl = env_config.use_ssl;
        }

        if env_config.use_compression.is_some() {
            raw_config.http.use_compression = env_config.use_compression;
        }

        if env_config.gzip_level.is_some() {
            raw_config.http.gzip_level = env_config.gzip_level;
        }

        let mut params = match raw_config.http.params {
            Some(v) => v.clone(),
            None => Params {
                hostname: "".to_string(),
                mac: None,
                ip: None,
                now: 0,
                tags: None,
            },
        };

        if let Some(v) = env_config.hostname {
            params.hostname = v;
        }

        if env_config.ip.is_some() {
            params.ip = env_config.ip;
        }

        if env_config.mac.is_some() {
            params.mac = env_config.mac;
        }

        if let Some(v) = env_config.tags {
            match params.tags {
                Some(ref mut tags) => v.iter().for_each(|t| { tags.add(t); }),
                None => params.tags = Some(Tags::from(v.0)),
            }
        }

        raw_config.http.params = Some(params);

        if let Some(mut v) = env_config.log_dirs {
            raw_config.log.dirs.append(&mut v)
        }

        if let Some(mut v) = env_config.exclusion_rules {
            match raw_config.log.exclude {
                Some(ref mut rules) => {
                    rules.glob.append(&mut v)
                }
                None => {
                    let mut rules = RawRules {
                        glob: Vec::new(),
                        regex: Vec::new(),
                    };
                    rules.glob.append(&mut v);
                    raw_config.log.exclude = Some(rules);
                }
            }
        }

        if let Some(mut v) = env_config.exclusion_regex_rules {
            match raw_config.log.exclude {
                Some(ref mut rules) => {
                    rules.regex.append(&mut v)
                }
                None => {
                    let mut rules = RawRules {
                        glob: Vec::new(),
                        regex: Vec::new(),
                    };
                    rules.regex.append(&mut v);
                    raw_config.log.exclude = Some(rules);
                }
            }
        }

        if let Some(mut v) = env_config.inclusion_rules {
            match raw_config.log.include {
                Some(ref mut rules) => {
                    rules.glob.append(&mut v)
                }
                None => {
                    let mut rules = RawRules {
                        glob: Vec::new(),
                        regex: Vec::new(),
                    };
                    rules.glob.append(&mut v);
                    raw_config.log.include = Some(rules);
                }
            }
        }

        if let Some(mut v) = env_config.inclusion_regex_rules {
            match raw_config.log.include {
                Some(ref mut rules) => {
                    rules.regex.append(&mut v)
                }
                None => {
                    let mut rules = RawRules {
                        glob: Vec::new(),
                        regex: Vec::new(),
                    };
                    rules.regex.append(&mut v);
                    raw_config.log.include = Some(rules);
                }
            }
        }

        Config::try_from(raw_config)
    }
}

impl TryFrom<RawConfig> for Config {
    type Error = ConfigError;

    fn try_from(raw: RawConfig) -> Result<Self, Self::Error> {
        let mut template_builder = RequestTemplate::builder();

        template_builder.api_key(
            raw.http.ingestion_key
                .ok_or(ConfigError::MissingField("http.ingestion_key"))?
        );

        let use_ssl = raw.http.use_ssl
            .ok_or(ConfigError::MissingField("http.use_ssl"))?;
        match use_ssl {
            true => template_builder.schema(Schema::Https),
            false => template_builder.schema(Schema::Http),
        };

        let use_compression = raw.http.use_compression
            .ok_or(ConfigError::MissingField("http.use_compression"))?;
        let gzip_level = raw.http.gzip_level
            .ok_or(ConfigError::MissingField("http.gzip_level"))?;
        match use_compression {
            true => template_builder.encoding(Encoding::GzipJson(Compression::new(gzip_level))),
            false => template_builder.encoding(Encoding::Json),
        };

        template_builder.host(
            raw.http.host
                .ok_or(ConfigError::MissingField("http.host"))?
        );

        template_builder.endpoint(
            raw.http.endpoint
                .ok_or(ConfigError::MissingField("http.endpoint"))?
        );

        template_builder.params(raw.http.params
            .ok_or(ConfigError::MissingField("http.params"))?);

        let http = HttpConfig {
            template: template_builder.build()?,
            timeout: Duration::from_millis(
                raw.http.timeout.
                    ok_or(ConfigError::MissingField("http.timeout"))?
            ),
            body_size: raw.http.body_size.
                ok_or(ConfigError::MissingField("http.body_size"))?,
        };

        let mut log = LogConfig {
            dirs: raw.log.dirs
                .into_iter()
                .map(|s| PathBuf::from(s))
                .collect(),
            rules: Rules::new(),
        };

        if let Some(rules) = raw.log.include {
            for glob in rules.glob {
                log.rules.add_inclusion(GlobRule::new(&*glob)?)
            }

            for regex in rules.regex {
                log.rules.add_inclusion(RegexRule::new(&*regex)?)
            }
        }

        if let Some(rules) = raw.log.exclude {
            for glob in rules.glob {
                log.rules.add_exclusion(GlobRule::new(&*glob)?)
            }

            for regex in rules.regex {
                log.rules.add_exclusion(RegexRule::new(&*regex)?)
            }
        }

        Ok(Config {
            http,
            log,
        })
    }
}

pub fn get_hostname() -> Option<String> {
    let path = PathBuf::from("/etc/logdna-hostname");
    if path.exists() {
        if let Ok(s) = File::open(&path)
            .and_then(|mut f| {
                let mut s = String::new();
                f.read_to_string(&mut s).map(|_| s)
            }) {
            return Some(s)
        }
    }

    let path = PathBuf::from("/etc/hostname");
    if path.exists() {
        if let Ok(s) = File::open(&path)
            .and_then(|mut f| {
                let mut s = String::new();
                f.read_to_string(&mut s).map(|_| s)
            }) {
            return Some(s)
        }
    }

    let name = CString::new(Vec::with_capacity(512)).ok()?.into_raw();
    if unsafe { libc::gethostname(name, 512) } == 0 {
        return unsafe { CString::from_raw(name) }.into_string().ok();
    }

    None
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs::{OpenOptions, remove_file};

    use scopeguard::guard;

    use super::*;

    #[test]
    fn test_hostname() {
        assert!(get_hostname().is_some());
    }

    #[test]
    fn test_raw_to_typed() {
        let raw = RawConfig::default();
        assert!(Config::try_from(raw).is_err());
        let mut raw = RawConfig::default();
        raw.http.ingestion_key = Some("emptyingestionkey".to_string());
        assert!(Config::try_from(raw).is_ok());
    }

    #[test]
    fn e2e() {
        remove_file("test.yaml");

        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .read(true)
            .open("test.yaml")
            .unwrap();

        guard(file, |file| {
            serde_yaml::to_writer(file, &RawConfig::default()).unwrap();

            env::set_var(&EnvConfig::config_file_vars()[0], "test.yaml");
            assert!(Config::new().is_err());
            env::set_var(&EnvConfig::ingestion_key_vars()[0], "ingestion_key_test");
            assert!(Config::new().is_ok());

            let old_len = Config::new().unwrap().log.rules.inclusion_list().len();
            env::set_var(&EnvConfig::inclusion_rules_vars()[0], "test.log,test2.log");
            assert_eq!(old_len + 2, Config::new().unwrap().log.rules.inclusion_list().len());

            remove_file("test.yaml").unwrap();
        });
    }
}