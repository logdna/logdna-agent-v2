extern crate humanize_rs;

use lazy_static::lazy_static;
use regex::Regex;
use std::collections::HashMap;
use std::convert::{TryFrom, TryInto};
use std::ffi::OsString;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;
use strum::{Display, EnumString};
use sysinfo::{RefreshKind, System, SystemExt};
use tracing::{info, warn};

use async_compression::Level;

use fs::lookback::Lookback;
use fs::rule::{RuleDef, Rules};
use fs::tail::DirPathBuf;
use http::types::params::Params;
use http::types::request::{Encoding, RequestTemplate, Schema};

pub use crate::argv::ArgumentOptions;
use crate::error::ConfigError;
use crate::raw::Config as RawConfig;

mod argv;
pub mod env_vars;
pub mod error;
mod properties;
pub mod raw;

// Symbols that will be populated in the main.rs file
extern "Rust" {
    static PKG_NAME: &'static str;
    static PKG_VERSION: &'static str;
}

#[derive(Debug, Eq, PartialEq)]
pub enum DbPath {
    Path(PathBuf),
    Empty,
}

#[cfg(unix)]
pub const DEFAULT_DB_PATH: &str = "/var/lib/logdna/";

#[cfg(windows)]
pub const DEFAULT_DB_PATH: &str = r"C:\ProgramData\logdna\";

impl DbPath {
    pub fn from(db_path: Option<PathBuf>) -> Self {
        match db_path {
            Some(path) => {
                let path_os_str = path.as_os_str();
                // if path is empty or all whitespace
                if path_os_str.is_empty()
                    || (path_os_str
                        .to_string_lossy()
                        .chars()
                        .filter(|c| !c.is_whitespace())
                        .count()
                        == 0)
                {
                    DbPath::Empty
                } else {
                    DbPath::Path(path)
                }
            }
            None => DbPath::Path(PathBuf::from(DEFAULT_DB_PATH)),
        }
    }
}

#[derive(Debug)]
pub struct Config {
    pub http: HttpConfig,
    pub log: LogConfig,
    pub journald: JournaldConfig,
    pub startup: K8sLeaseConf,
}

#[derive(Debug)]
pub struct HttpConfig {
    pub template: RequestTemplate,
    pub timeout: Duration,
    pub body_size: usize,
    pub require_ssl: bool,
    pub retry_dir: PathBuf,
    pub retry_disk_limit: Option<u64>,

    // Development only settings
    pub retry_base_delay: Duration,
    pub retry_step_delay: Duration,
}

#[derive(Clone, Display, core::fmt::Debug, Eq, PartialEq, EnumString, Default)]
pub enum K8sTrackingConf {
    #[strum(serialize = "always")]
    Always,

    #[default]
    #[strum(serialize = "never")]
    Never,
}

#[derive(Debug)]
pub struct LogConfig {
    pub dirs: Vec<DirPathBuf>,
    pub db_path: DbPath,
    pub metrics_port: Option<u16>,
    pub rules: Rules,
    pub line_exclusion_regex: Vec<String>,
    pub line_inclusion_regex: Vec<String>,
    pub line_redact_regex: Vec<String>,
    pub lookback: Lookback,
    pub use_k8s_enrichment: K8sTrackingConf,
    pub log_k8s_events: K8sTrackingConf,
    pub k8s_metadata_include: Vec<String>,
    pub k8s_metadata_exclude: Vec<String>,
    pub log_metric_server_stats: K8sTrackingConf,
    pub clear_cache_interval: Duration,
    pub tailer_cmd: Option<String>,
    pub tailer_args: Option<String>,
    pub metadata_retry_delay: Duration,
}

#[derive(Debug)]
pub struct JournaldConfig {
    pub paths: Vec<PathBuf>,
    pub systemd_journal_tailer: bool,
}

#[derive(Clone, core::fmt::Debug, Default, Display, EnumString, Eq, PartialEq)]
pub enum K8sLeaseConf {
    #[default]
    #[strum(serialize = "never")]
    Never,

    #[strum(serialize = "attempt")]
    Attempt,

    #[strum(serialize = "always")]
    Always,
}

const LOGDNA_PREFIX: &str = "LOGDNA_";
const MEZMO_PREFIX: &str = "MZ_";

impl Config {
    pub fn new_from_options(argv_options: ArgumentOptions) -> Result<Self, ConfigError> {
        let config_path = argv_options.config.clone();

        let config_path_str = config_path.to_string_lossy();
        let is_default_path =
            config_path_str == argv::DEFAULT_YAML_FILE || config_path == argv::default_conf_file();
        let does_default_exist =
            Path::new(argv::DEFAULT_YAML_FILE).exists() || argv::default_conf_file().exists();
        let raw_config = if is_default_path {
            if does_default_exist {
                info!("using settings from default config file, environment variables and command line options");
                RawConfig::parse(&config_path).map_err(ConfigError::MultipleErrors)?
            } else {
                info!("using settings from environment variables and command line options");
                // Non-existing default config yields default RawConfig
                RawConfig::default()
            }
        } else {
            info!(
                "using settings from config file, environment variables and command line options"
            );
            RawConfig::parse(&config_path).map_err(ConfigError::MultipleErrors)?
        };

        // Merge with cmd line and env options into raw_config
        let raw_config = argv_options.merge(raw_config);

        // print effective config
        print_settings(&raw_config)?;

        Config::try_from(raw_config)
    }

    pub fn new<I>(args: I) -> Result<Self, ConfigError>
    where
        I: IntoIterator,
        I::Item: Into<OsString> + Clone,
    {
        let argv_options = ArgumentOptions::from_args_with_all_env_vars(args);
        Config::new_from_options(argv_options)
    }

    pub fn process_logdna_env_vars() {
        std::env::vars_os()
            .filter(|(n, _)| {
                n.clone()
                    .into_string()
                    .unwrap_or_default()
                    .starts_with(LOGDNA_PREFIX)
            })
            .for_each(|(name, value)| {
                let new_name = MEZMO_PREFIX.to_string()
                    + &name.into_string().unwrap_or_default()[LOGDNA_PREFIX.len()..];
                std::env::set_var(new_name, value);
            });
    }
}

impl TryFrom<RawConfig> for Config {
    type Error = ConfigError;

    fn try_from(raw: RawConfig) -> Result<Self, Self::Error> {
        let mut template_builder = RequestTemplate::builder();

        let ingestion_key = match raw.http.ingestion_key {
            Some(ref key) if !key.is_empty() => key,
            _ => {
                return Err(ConfigError::MissingFieldOrEnvVar(
                    "http.ingestion_key",
                    env_vars::INGESTION_KEY,
                ))
            }
        };
        template_builder.api_key(ingestion_key);

        let use_ssl = raw.http.use_ssl.ok_or(ConfigError::MissingFieldOrEnvVar(
            "http.use_ssl",
            env_vars::USE_SSL,
        ))?;

        if use_ssl {
            template_builder.schema(Schema::Https);
        } else {
            template_builder.schema(Schema::Http);
        }

        let use_compression = raw
            .http
            .use_compression
            .ok_or(ConfigError::MissingFieldOrEnvVar(
                "http.use_compression",
                env_vars::USE_COMPRESSION,
            ))?;

        let gzip_level = raw
            .http
            .gzip_level
            .ok_or(ConfigError::MissingFieldOrEnvVar(
                "http.gzip_level",
                env_vars::GZIP_LEVEL,
            ))?;

        if use_compression {
            template_builder.encoding(Encoding::GzipJson(Level::Precise(gzip_level)));
        } else {
            template_builder.encoding(Encoding::Json);
        }

        template_builder.host(raw.http.host.filter(|s| !s.is_empty()).ok_or(
            ConfigError::MissingFieldOrEnvVar("http.host", env_vars::HOST),
        )?);

        template_builder.endpoint(raw.http.endpoint.filter(|s| !s.is_empty()).ok_or(
            ConfigError::MissingFieldOrEnvVar("http.endpoint", env_vars::ENDPOINT),
        )?);

        let params = raw
            .http
            .params
            .and_then(|mut builder| {
                if builder.build().is_err() {
                    builder.hostname(get_hostname().unwrap_or_default());
                }
                builder.build().ok()
            })
            .unwrap_or_else(|| {
                let mut builder = Params::builder();
                builder.hostname(get_hostname().unwrap_or_default());
                builder
                    .build()
                    .expect("Failed to create default http.params")
            });

        template_builder.params(params);

        let sys = System::new_with_specifics(RefreshKind::new());
        let info = str::replace(
            &format!(
                "{}/{}",
                sys.name().unwrap_or_else(|| "unknown".into()),
                sys.os_version().unwrap_or_else(|| "unknown".into()),
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

        template_builder.user_agent(format!("{}/{} ({})", pkg_name, pkg_version, info).as_str());

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
            retry_dir: raw.http.retry_dir.unwrap_or_else(|| {
                if cfg!(windows) {
                    if let Ok(temp) = std::env::var("temp") {
                        let dir = format!("{}/{}", temp, "logdna");
                        return PathBuf::from(dir);
                    }
                }
                PathBuf::from("/tmp/logdna")
            }),
            retry_disk_limit: raw.http.retry_disk_limit,
            retry_base_delay: Duration::from_millis(
                raw.http.retry_base_delay_ms.unwrap_or(15_000) as u64
            ),
            retry_step_delay: Duration::from_millis(
                raw.http.retry_step_delay_ms.unwrap_or(50) as u64
            ),
            require_ssl: raw.http.use_ssl.unwrap_or(true),
        };

        let mut log = LogConfig {
            dirs: raw
                .log
                .dirs
                .clone()
                .into_iter()
                // Find valid directory paths and keep track of missing paths
                .filter_map(|d| {
                    d.clone()
                        .try_into()
                        .map_err(|e| {
                            warn!("{} is not a valid directory: {:?}", d.display(), e);
                        })
                        .ok()
                })
                .collect(),
            db_path: DbPath::from(raw.log.db_path),
            metrics_port: raw.log.metrics_port,
            rules: Rules::new(),
            line_exclusion_regex: raw.log.line_exclusion_regex.unwrap_or_default(),
            line_inclusion_regex: raw.log.line_inclusion_regex.unwrap_or_default(),
            line_redact_regex: raw.log.line_redact_regex.unwrap_or_default(),
            lookback: raw
                .log
                .lookback
                .map(|s| s.parse::<Lookback>())
                .unwrap_or_else(|| Ok(Lookback::default()))?,
            use_k8s_enrichment: parse_k8s_enum_config_or_warn(
                raw.log.use_k8s_enrichment,
                env_vars::USE_K8S_LOG_ENRICHMENT,
                K8sTrackingConf::Always,
            ),
            log_k8s_events: parse_k8s_enum_config_or_warn(
                raw.log.log_k8s_events,
                env_vars::LOG_K8S_EVENTS,
                K8sTrackingConf::Never,
            ),
            k8s_metadata_include: raw.log.k8s_metadata_include.unwrap_or_default(),
            k8s_metadata_exclude: raw.log.k8s_metadata_exclude.unwrap_or_default(),
            log_metric_server_stats: parse_k8s_enum_config_or_warn(
                raw.log.log_metric_server_stats,
                env_vars::LOG_METRIC_SERVER_STATS,
                K8sTrackingConf::Never,
            ),
            clear_cache_interval: Duration::from_secs(raw.log.clear_cache_interval.unwrap_or_else(
                || {
                    raw::LogConfig::default()
                        .clear_cache_interval
                        .unwrap_or_default()
                },
            ) as u64),
            tailer_cmd: raw.log.tailer_cmd,
            tailer_args: raw.log.tailer_args,
            metadata_retry_delay: Duration::from_secs(raw.log.metadata_retry_delay.unwrap_or_else(
                || {
                    raw::LogConfig::default()
                        .metadata_retry_delay
                        .unwrap_or_default()
                },
            ) as u64),
        };

        if log.use_k8s_enrichment == K8sTrackingConf::Never
            && log.log_k8s_events == K8sTrackingConf::Always
        {
            // It's unlikely that a user will want to disable k8s metadata enrichment
            // but log k8s resource events, warn the user and continue
            warn!(
                "k8s metadata enrichment is disabled while k8s resource event logging is enabled. \
                 Please verify this setting values are intended."
            );
        }

        if let Some(rules) = raw.log.include {
            for glob in rules.glob {
                log.rules.add_inclusion(RuleDef::glob_rule(&*glob)?)
            }

            for regex in rules.regex {
                log.rules.add_inclusion(RuleDef::regex_rule(&*regex)?)
            }
        }

        if let Some(rules) = raw.log.exclude {
            for glob in rules.glob {
                log.rules.add_exclusion(RuleDef::glob_rule(&*glob)?)
            }

            for regex in rules.regex {
                log.rules.add_exclusion(RuleDef::regex_rule(&*regex)?)
            }
        }

        let startup = raw.startup.map_or_else(K8sLeaseConf::default, |startup| {
            parse_k8s_enum_config_or_warn(
                startup.option,
                env_vars::K8S_STARTUP_LEASE,
                K8sLeaseConf::Never,
            )
        });

        let raw_journald = raw.journald.unwrap_or_default();
        let journald = JournaldConfig {
            paths: raw_journald.paths.unwrap_or_default().into_iter().collect(),
            systemd_journal_tailer: raw_journald.systemd_journal_tailer.unwrap_or(true),
        };

        Ok(Config {
            http,
            log,
            journald,
            startup,
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

    System::new_with_specifics(RefreshKind::new()).host_name()
}

fn print_settings(raw_config: &RawConfig) -> Result<(), ConfigError> {
    // print agent env vars
    let env_config: String = env_vars::ENV_VARS_LIST
        .iter()
        .chain(env_vars::DEPRECATED_ENV_VARS_LIST.iter())
        .chain(["RUST_LOG"].iter())
        .filter_map(|&key| {
            std::env::var(key).ok().map(|value| {
                if key.contains("KEY") || key.contains("PIN") {
                    format!("{}=REDACTED", key)
                } else {
                    format!("{}={}", key, value)
                }
            })
        })
        .collect::<Vec<String>>()
        .join("\n");
    info!("environment variables: \n{}\n", env_config);
    // get config as yaml string
    let mut tmp_config = raw_config.clone();
    if let Some(ref mut key) = tmp_config.http.ingestion_key {
        *key = "REDACTED".to_string();
    }
    let yaml_str = match serde_yaml::to_string(&tmp_config) {
        Ok(v) => v,
        Err(e) => {
            return Err(ConfigError::Serde(e));
        }
    };
    info!("effective configuration:\n{}", yaml_str);
    Ok(())
}

fn parse_k8s_enum_config_or_warn<T: FromStr + std::fmt::Display + std::fmt::Debug>(
    value: Option<String>,
    name: &str,
    default: T,
) -> T
where
    <T as FromStr>::Err: std::fmt::Display,
{
    if let Some(s) = value {
        match s.parse::<T>() {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    "Failed to parse {} defaulting to {:?}. error: {}",
                    name, default, e
                );
                default
            }
        }
    } else {
        default
    }
}

lazy_static! {
    static ref REGEX_VAR: Regex = Regex::new(r"(?P<var>\$\{(?P<key>[^|}]+?)})").unwrap();
    static ref REGEX_VAR_DEFAULT: Regex =
        Regex::new(r"(?P<var>\$\{(?P<key>[^|}]+?)\|(?P<default>[^|}]*?)})").unwrap();
}

/// Var expansion with default value support.
/// Supported cases:
///   1. ${VarName}             - simple substitute using vars dictionary,
///                               expended to empty string if var not found
///   2. ${VarName|DefaultVal}  - first try to expand as ${VarName} then
///                               use DefaultVal if var not found  
///   3. ${VarName|${VarName2}} - ${VarName2} is expanded first then case #2
///   4. ${VarName|}            - empty default, equivalent to "var not found" in case #1
///
pub fn substitute(template: &str, variables: &HashMap<String, String>) -> String {
    // handle case #1
    let mut output = String::from(template);
    for cap in REGEX_VAR.captures_iter(template) {
        cap.name("key").map(|key| {
            let k = key.as_str();
            if let Some(v) = variables.get(k) {
                cap.name("var").map(|var| {
                    output = output.replace(var.as_str(), v);
                    Some(var)
                });
            }
            Some(k)
        });
    }
    // handle cases #2,3,4
    for cap in REGEX_VAR_DEFAULT.captures_iter(template) {
        cap.name("key").map(|key| {
            let k = key.as_str();
            match variables.get(k) {
                Some(v) => {
                    cap.name("var").map(|var| {
                        output = output.replace(var.as_str(), v);
                        Some(var)
                    });
                }
                None => {
                    cap.name("default").map(|default| {
                        cap.name("var").map(|var| {
                            output = output.replace(var.as_str(), default.as_str());
                            Some(var)
                        });
                        Some(default)
                    });
                }
            }
            Some(k)
        });
    }
    output
}

#[cfg(test)]
mod tests {

    // Provide values for extern symbols PKG_NAME and PKG_VERSION
    // when building this module on it's own
    #[no_mangle]
    pub static PKG_NAME: &str = "test";
    #[no_mangle]
    pub static PKG_VERSION: &str = "test";

    #[cfg(unix)]
    static DEFAULT_LOG_DIR: &str = "/var/log/";
    #[cfg(windows)]
    static DEFAULT_LOG_DIR: &str = r"C:\ProgramData\logs";

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    use scopeguard::guard;
    use std::env;
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    use std::fs::OpenOptions;
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    use std::io::Write;

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
        let result = get_default_config();
        let user_agent = result.http.template.user_agent.to_str().unwrap();
        assert!(user_agent.contains('(') && user_agent.contains(')'));
    }

    #[test]
    fn test_db_path() {
        // Default
        assert_eq!(
            DbPath::from(None),
            DbPath::Path(PathBuf::from(DEFAULT_DB_PATH))
        );

        // Actual path
        assert_eq!(
            DbPath::from(Some(PathBuf::from("/not/var/lib/logdna"))),
            DbPath::Path(PathBuf::from("/not/var/lib/logdna"))
        );

        // gibberish value, but not DbPath's problem to deal with
        assert_eq!(
            DbPath::from(Some(PathBuf::from(" n "))),
            DbPath::Path(PathBuf::from(" n "))
        );

        // Empty values
        assert_eq!(DbPath::from(Some(PathBuf::from(""))), DbPath::Empty);
        assert_eq!(DbPath::from(Some(PathBuf::from(" "))), DbPath::Empty);
        assert_eq!(DbPath::from(Some(PathBuf::from("\n"))), DbPath::Empty);
        assert_eq!(DbPath::from(Some(PathBuf::from("   \n "))), DbPath::Empty);
    }

    #[test]
    fn test_default_parsed() {
        assert_eq!(
            raw::default_log_dirs(),
            vec![PathBuf::from(DEFAULT_LOG_DIR)]
        );
        let config = get_default_config();
        assert_eq!(config.log.use_k8s_enrichment, K8sTrackingConf::Always);
        assert_eq!(config.log.log_k8s_events, K8sTrackingConf::Never);
        assert_eq!(config.log.log_metric_server_stats, K8sTrackingConf::Never);
        assert_eq!(config.log.lookback, Lookback::None);
        let def_pathbuf = PathBuf::from(DEFAULT_LOG_DIR);
        assert_eq!(
            config.log.dirs,
            vec![DirPathBuf::try_from(def_pathbuf).unwrap()]
        );
        assert_eq!(config.startup, K8sLeaseConf::Never);
    }

    #[cfg(unix)]
    #[test]
    fn test_default_rules() {
        let config = get_default_config();

        let should_pass = ["/var/log/a.log",
            "/var/log/containers/a.log",
            "/var/log/custom/a.log",

            // These are not excluded in the rules, it's not on the default log dirs
            // so if a symlink points to it, it should pass
            "/var/data/a.log",
            "/tmp/app/a.log",
            "/var/data/kubeletlogs/some-named-service-aabb67c8fc-9ncjd_52c36bc5-4a53-4827-9dc8-082926ac1bc9/some-named-service/1.log"];

        for p in should_pass.iter().map(PathBuf::from) {
            assert!(
                config.log.rules.passes(&p).is_ok(),
                "Rule should pass for: {:?}",
                &p
            );
        }

        let should_not_pass = vec![
            "/var/log/a.gz",
            "/var/log/a.tar.gz",
            "/var/log/a.zip",
            "/var/log/a.tar",
            "/var/log/a.0",
            "/var/log/btmp",
            "/var/log/utmp",
            "/var/log/wtmpx",
            "/var/log/btmpx",
            "/var/log/utmpx",
            "/var/log/asl/a.log",
            "/var/log/sa/a.log",
            "/var/log/saradd.log",
            "/var/log/tallylog",
            "/var/log/fluentd-buffers/some/a.log",
            "/var/log/pods/a.log",
            "/var/log/pods/john/bonham.log",
        ];

        for p in should_not_pass.iter().map(PathBuf::from) {
            assert!(
                !config.log.rules.passes(&p).is_ok(),
                "Rule passed but should not pass for: {:?}",
                &p
            );
        }
    }

    #[test]
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn e2e() {
        let tempdir = tempfile::TempDir::new().unwrap();
        let path = tempdir.path().to_path_buf();
        let path = path.join("test.yaml");

        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .read(true)
            .open(&path)
            .unwrap();

        env::remove_var("LOGDNA_INGESTION_KEY");
        env::remove_var("MZ_INGESTION_KEY");
        env::remove_var(env_vars::INGESTION_KEY_ALTERNATE);
        env::remove_var(env_vars::INCLUSION_RULES_DEPRECATED);

        let _ = guard(file, |mut file| {
            let args = vec![OsString::new()];
            serde_yaml::to_writer(&mut file, &RawConfig::default()).unwrap();
            file.flush().unwrap();

            env::set_var(env_vars::CONFIG_FILE, path);

            // invalid config - no key
            assert!(Config::new(args.clone()).is_err());

            env::set_var(env_vars::INGESTION_KEY, "ingestion_key_test");

            // valid config
            assert!(Config::new(args.clone()).is_ok());

            let old_len = Config::new(args.clone())
                .unwrap()
                .log
                .rules
                .inclusion_list()
                .len();
            env::set_var(env_vars::INCLUSION_RULES, "test.log,test2.log");
            assert_eq!(
                old_len + 2,
                Config::new(args).unwrap().log.rules.inclusion_list().len()
            );
        });
    }

    /// Creates an instance in the same was as `Config::new()`, except it fills in a
    /// fake ingestion key.
    fn get_default_config() -> Config {
        let mut raw = RawConfig::default();
        raw.http.ingestion_key = Some("dummy-test-key".to_string());
        Config::try_from(raw).unwrap()
    }

    #[test]
    fn test_process_logdna_env_vars() {
        env::set_var("LOGDNA_TEST", "LOGDNA_TEST");
        env::set_var("LOGDNA_", "LOGDNA_");
        env::set_var("MZ_SOME", "MZ_SOME");
        Config::process_logdna_env_vars();
        assert_eq!(env::var("MZ_TEST").unwrap(), "LOGDNA_TEST");
        assert_eq!(env::var("MZ_").unwrap(), "LOGDNA_");
        assert_eq!(env::var("MZ_SOME").unwrap(), "MZ_SOME");
    }

    #[test]
    fn test_substitute() {
        use std::collections::HashMap;
        let vals = HashMap::from([
            ("val1".to_string(), "1".to_string()),
            ("val2".to_string(), "2".to_string()),
            ("val3".to_string(), "3".to_string()),
            ("val4".to_string(), "".to_string()),
        ]);
        let templ =
            r#"{"key1":"${val1}", "key2":"${val2}", "key3":"${val3|3}", "key4":"${val4|}"}"#;
        let res = substitute(templ, &vals);
        assert_eq!(res, r#"{"key1":"1", "key2":"2", "key3":"3", "key4":""}"#);
    }
}
