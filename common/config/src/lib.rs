#[macro_use]
extern crate log;

use std::convert::TryFrom;
use std::ffi::CString;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use flate2::Compression;

use fs::rule::{GlobRule, RegexRule, Rules};
use http::types::request::{Encoding, RequestTemplate, Schema};

use crate::env::Config as EnvConfig;
use crate::error::ConfigError;
use crate::raw::Config as RawConfig;

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
        let raw_config = match RawConfig::parse(&env_config.config_file) {
            Ok(v) => v,
            Err(e) => {
                warn!(
                    "failed to load config, failing back to default \
                     (ignore if you are not using a configmap or config file): {}",
                    e
                );
                RawConfig::default()
            }
        };

        let raw_config = env_config.merge(raw_config);

        let mut tmp_config = raw_config.clone();
        if let Some(ref mut key) = tmp_config.http.ingestion_key {
            *key = "REDACTED".to_string();
        }
        if let Ok(yaml) = serde_yaml::to_string(&tmp_config) {
            info!("current config: \n{}", yaml)
        }

        Config::try_from(raw_config)
    }
}

impl TryFrom<RawConfig> for Config {
    type Error = ConfigError;

    fn try_from(raw: RawConfig) -> Result<Self, Self::Error> {
        let mut template_builder = RequestTemplate::builder();

        template_builder.api_key(raw.http.ingestion_key.filter(|s| !s.is_empty()).ok_or(
            ConfigError::MissingFieldOrEnvVar(
                "http.ingestion_key",
                EnvConfig::ingestion_key_vars(),
            ),
        )?);

        let use_ssl = raw.http.use_ssl.ok_or(ConfigError::MissingFieldOrEnvVar(
            "http.use_ssl",
            EnvConfig::use_ssl_vars(),
        ))?;

        match use_ssl {
            true => template_builder.schema(Schema::Https),
            false => template_builder.schema(Schema::Http),
        };

        let use_compression = raw
            .http
            .use_compression
            .ok_or(ConfigError::MissingFieldOrEnvVar(
                "http.use_compression",
                EnvConfig::use_compression_vars(),
            ))?;

        let gzip_level = raw
            .http
            .gzip_level
            .ok_or(ConfigError::MissingFieldOrEnvVar(
                "http.gzip_level",
                EnvConfig::gzip_level_vars(),
            ))?;

        match use_compression {
            true => template_builder.encoding(Encoding::GzipJson(Compression::new(gzip_level))),
            false => template_builder.encoding(Encoding::Json),
        };

        template_builder.host(raw.http.host.filter(|s| !s.is_empty()).ok_or(
            ConfigError::MissingFieldOrEnvVar("http.host", EnvConfig::host_vars()),
        )?);

        template_builder.endpoint(raw.http.endpoint.filter(|s| !s.is_empty()).ok_or(
            ConfigError::MissingFieldOrEnvVar("http.endpoint", EnvConfig::endpoint_vars()),
        )?);

        template_builder.params(
            raw.http
                .params
                .ok_or(ConfigError::MissingField("http.params"))?,
        );

        let mut info = "unknown".to_string();
        if let Ok(sys_info) = sys_info::linux_os_release() {
            info = format!(
                "{}/{}",
                sys_info.name.unwrap_or("unknown".to_string()),
                sys_info.version.unwrap_or("unknown".to_string()),
            )
        }

        template_builder.user_agent(format!(
            "{}/{} ({})",
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
            info
        ));

        let http = HttpConfig {
            template: template_builder.build()?,
            timeout: Duration::from_millis(
                raw.http
                    .timeout
                    .ok_or(ConfigError::MissingField("http.timeout"))?,
            ),
            body_size: raw
                .http
                .body_size
                .ok_or(ConfigError::MissingField("http.body_size"))?,
        };

        let mut log = LogConfig {
            dirs: raw.log.dirs.into_iter().map(|s| PathBuf::from(s)).collect(),
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

        Ok(Config { http, log })
    }
}

pub fn get_hostname() -> Option<String> {
    let path = PathBuf::from("/etc/logdna-hostname");
    if path.exists() {
        if let Ok(s) = File::open(&path).and_then(|mut f| {
            let mut s = String::new();
            f.read_to_string(&mut s).map(|_| s)
        }) {
            return Some(s);
        }
    }

    let path = PathBuf::from("/etc/hostname");
    if path.exists() {
        if let Ok(s) = File::open(&path).and_then(|mut f| {
            let mut s = String::new();
            f.read_to_string(&mut s).map(|_| s)
        }) {
            return Some(s);
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
    use std::fs::{remove_file, OpenOptions};

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

            EnvConfig::ingestion_key_vars()
                .iter()
                .for_each(|k| env::remove_var(k));
            env::set_var(&EnvConfig::config_file_vars()[0], "test.yaml");
            assert!(Config::new().is_err());

            env::set_var(&EnvConfig::ingestion_key_vars()[0], "ingestion_key_test");
            assert!(Config::new().is_ok());

            EnvConfig::inclusion_rules_vars()
                .iter()
                .for_each(|k| env::remove_var(k));
            let old_len = Config::new().unwrap().log.rules.inclusion_list().len();
            env::set_var(&EnvConfig::inclusion_rules_vars()[0], "test.log,test2.log");
            assert_eq!(
                old_len + 2,
                Config::new().unwrap().log.rules.inclusion_list().len()
            );

            remove_file("test.yaml").unwrap();
        });
    }
}
