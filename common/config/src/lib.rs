#[macro_use]
extern crate log;

use std::convert::{TryFrom, TryInto};
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;
use sysinfo::{RefreshKind, System, SystemExt};

use async_compression::Level;

use fs::rule::{GlobRule, RegexRule, Rules};
use fs::tail::{DirPathBuf, Lookback};
use http::types::request::{Encoding, RequestTemplate, Schema};
use k8s::K8sEventLogConf;

use crate::env::Config as EnvConfig;
use crate::error::ConfigError;
use crate::raw::Config as RawConfig;

pub mod env;
pub mod error;
pub mod raw;

// Symbols that will be populated in the main.rs file
extern "Rust" {
    static PKG_NAME: &'static str;
    static PKG_VERSION: &'static str;
}

#[derive(Debug)]
pub struct Config {
    pub http: HttpConfig,
    pub log: LogConfig,
    pub journald: JournaldConfig,
}

#[derive(Debug)]
pub struct HttpConfig {
    pub template: RequestTemplate,
    pub timeout: Duration,
    pub body_size: usize,
}

#[derive(Debug)]
pub struct LogConfig {
    pub dirs: Vec<DirPathBuf>,
    pub rules: Rules,
    pub lookback: Lookback,
    pub log_k8s_events: K8sEventLogConf,
}

#[derive(Debug)]
pub struct JournaldConfig {
    pub paths: Vec<PathBuf>,
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

        template_builder.api_key(
            raw.http
                .ingestion_key
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    ConfigError::MissingFieldOrEnvVar(
                        "http.ingestion_key",
                        EnvConfig::ingestion_key_vars(),
                    )
                })?,
        );

        let use_ssl = raw.http.use_ssl.ok_or_else(|| {
            ConfigError::MissingFieldOrEnvVar("http.use_ssl", EnvConfig::use_ssl_vars())
        })?;

        if use_ssl {
            template_builder.schema(Schema::Https);
        } else {
            template_builder.schema(Schema::Http);
        }

        let use_compression = raw.http.use_compression.ok_or_else(|| {
            ConfigError::MissingFieldOrEnvVar(
                "http.use_compression",
                EnvConfig::use_compression_vars(),
            )
        })?;

        let gzip_level = raw.http.gzip_level.ok_or_else(|| {
            ConfigError::MissingFieldOrEnvVar("http.gzip_level", EnvConfig::gzip_level_vars())
        })?;

        if use_compression {
            template_builder.encoding(Encoding::GzipJson(Level::Precise(gzip_level)));
        } else {
            template_builder.encoding(Encoding::Json);
        }

        template_builder.host(raw.http.host.filter(|s| !s.is_empty()).ok_or_else(|| {
            ConfigError::MissingFieldOrEnvVar("http.host", EnvConfig::host_vars())
        })?);

        template_builder.endpoint(raw.http.endpoint.filter(|s| !s.is_empty()).ok_or_else(
            || ConfigError::MissingFieldOrEnvVar("http.endpoint", EnvConfig::endpoint_vars()),
        )?);

        template_builder.params(
            raw.http
                .params
                .ok_or(ConfigError::MissingField("http.params"))?,
        );

        let sys = System::new_with_specifics(RefreshKind::new());
        let info = str::replace(
            &format!(
                "{}/{}",
                sys.get_name().unwrap_or_else(|| "unknown".into()),
                sys.get_version().unwrap_or_else(|| "unknown".into()),
            ),
            |c| !matches!(c, '\x20'..='\x7e'),
            "",
        );

        // Read the PKG_NAME and PKG_VERSION defined in the main.rs or test module.
        // Safety: unsafe is required to read from extern statics. This is safe as we control
        // the externed symbols that are being referenced, they are defined within the agent code base.
        // The program will not link unless these are defined somewhere in the crate graph and
        // if there are duplicate symbols with the same name it will also result in a linker error
        // so as long as the symbols we create are &'static str's then this is completely safe.
        let (pkg_name, pkg_version) = unsafe { (PKG_NAME, PKG_VERSION) };

        template_builder.user_agent(format!("{}/{} ({})", pkg_name, pkg_version, info));

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
            dirs: raw
                .log
                .dirs
                .into_iter()
                // Filter off paths that are not directories and warn about them
                .filter_map(|d| {
                    d.try_into()
                        .map_err(|e| {
                            warn!("{}", e);
                        })
                        .ok()
                })
                .collect(),
            rules: Rules::new(),
            lookback: raw
                .log
                .lookback
                .map(|s| s.parse::<Lookback>())
                .unwrap_or_else(|| Ok(Lookback::default()))?,
            log_k8s_events: if let Some(s) = raw.log.log_k8s_events {
                match s.parse::<K8sEventLogConf>() {
                    Ok(s) => s,
                    Err(e) => {
                        warn!(
                            "Failed to parse LOGDNA_LOG_K8S_EVENTS defaulting to never. error: {}",
                            e
                        );
                        K8sEventLogConf::Never
                    }
                }
            } else {
                K8sEventLogConf::Never
            },
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

        let journald = JournaldConfig {
            paths: raw.journald.paths.unwrap_or_default().into_iter().collect(),
        };

        Ok(Config {
            http,
            log,
            journald,
        })
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

    System::new_with_specifics(RefreshKind::new()).get_host_name()
}

#[cfg(test)]
mod tests {

    // Provide values for extern symbols PKG_NAME and PKG_VERSION
    // when building this module on it's own
    #[no_mangle]
    pub static PKG_NAME: &str = "test";
    #[no_mangle]
    pub static PKG_VERSION: &str = "test";

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
    fn test_user_agent() {
        let mut raw = RawConfig::default();
        raw.http.ingestion_key = Some("anyingestionkey".to_string());
        let result = Config::try_from(raw).unwrap();
        let user_agent = result.http.template.user_agent.to_str().unwrap();
        assert!(user_agent.contains('(') && user_agent.contains(')'));
    }

    #[test]
    fn e2e() {
        let _ = remove_file("test.yaml");

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
                .for_each(|v| unsafe { env::remove_var(v) });
            env::set_var(&EnvConfig::config_file_vars()[0], "test.yaml");
            assert!(Config::new().is_err());

            env::set_var(&EnvConfig::ingestion_key_vars()[0], "ingestion_key_test");
            assert!(Config::new().is_ok());

            EnvConfig::inclusion_rules_vars()
                .iter()
                .for_each(|v| unsafe { env::remove_var(v) });
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
