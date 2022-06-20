#[macro_use]
extern crate log;
extern crate humanize_rs;

use std::convert::{TryFrom, TryInto};
use std::ffi::OsString;
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;
use sysinfo::{RefreshKind, System, SystemExt};

use async_compression::Level;

use fs::lookback::Lookback;
use fs::rule::{RuleDef, Rules};
use fs::tail::DirPathBuf;
use http::types::request::{Encoding, RequestTemplate, Schema};

use crate::argv::ArgumentOptions;
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

#[derive(Debug, PartialEq)]
pub enum DbPath {
    Path(PathBuf),
    Empty,
}

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
            None => DbPath::Path(PathBuf::from("/var/lib/logdna/")),
        }
    }
}

#[derive(Debug)]
pub struct Config {
    pub http: HttpConfig,
    pub log: LogConfig,
    pub journald: JournaldConfig,
    pub startup: K8sStartupLeaseConfig,
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

#[derive(Clone, core::fmt::Debug, PartialEq)]
pub enum K8sTrackingConf {
    Always,
    Never,
}

impl Default for K8sTrackingConf {
    fn default() -> Self {
        K8sTrackingConf::Never
    }
}

impl fmt::Display for K8sTrackingConf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                K8sTrackingConf::Always => "always",
                K8sTrackingConf::Never => "never",
            }
        )
    }
}

#[derive(thiserror::Error, Debug)]
#[error("{0}")]
pub struct ParseK8sTrackingConf(String);

impl std::str::FromStr for K8sTrackingConf {
    type Err = ParseK8sTrackingConf;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "always" => Ok(K8sTrackingConf::Always),
            "never" => Ok(K8sTrackingConf::Never),
            _ => Err(ParseK8sTrackingConf(format!("failed to parse {}", s))),
        }
    }
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
}

#[derive(Debug)]
pub struct JournaldConfig {
    pub paths: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct K8sStartupLeaseConfig {
    pub option: String,
}

const LOGDNA_PREFIX: &str = "LOGDNA_";
const MEZMO_PREFIX: &str = "MZ_";

impl Config {
    pub fn new<I>(args: I) -> Result<Self, ConfigError>
    where
        I: IntoIterator,
        I::Item: Into<OsString> + Clone,
    {
        let argv_options = ArgumentOptions::from_args_with_all_env_vars(args);
        let list_settings = argv_options.list_settings;
        let config_path = argv_options.config.clone();
        let raw_config = match RawConfig::parse(&config_path) {
            Ok(v) => {
                info!("using settings defined in config file, env vars and command line options");
                v
            }
            Err(e) => {
                debug!("config file could not be parsed: {:?}", e);
                info!("using settings defined in env variables and command line options");
                RawConfig::default()
            }
        };

        // Merge with cmd line and env options
        let raw_config = argv_options.merge(raw_config);

        // Get a copy of the yaml with the merge values
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

        if list_settings {
            print_settings(&yaml_str, &config_path);
        }

        info!("starting with the following options: \n{}", yaml_str);

        Config::try_from(raw_config)
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

        template_builder.api_key(raw.http.ingestion_key.filter(|s| !s.is_empty()).ok_or(
            ConfigError::MissingFieldOrEnvVar("http.ingestion_key", env_vars::INGESTION_KEY),
        )?);

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
            retry_dir: raw
                .http
                .retry_dir
                .unwrap_or_else(|| PathBuf::from("/tmp/logdna")),
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
                .into_iter()
                // Filter off paths that are not directories and warn about them
                .filter_map(|d| {
                    d.clone()
                        .try_into()
                        .map_err(|e| {
                            warn!("{} is not a valid directory {}", d.display(), e);
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
            use_k8s_enrichment: parse_k8s_tracking_or_warn(
                raw.log.use_k8s_enrichment,
                env_vars::USE_K8S_LOG_ENRICHMENT,
                K8sTrackingConf::Always,
            ),
            log_k8s_events: parse_k8s_tracking_or_warn(
                raw.log.log_k8s_events,
                env_vars::LOG_K8S_EVENTS,
                K8sTrackingConf::Never,
            ),
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

        let startup = K8sStartupLeaseConfig {
            option: raw.startup.option.unwrap_or_default(),
        };

        let journald = JournaldConfig {
            paths: raw.journald.paths.unwrap_or_default().into_iter().collect(),
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

    System::new_with_specifics(RefreshKind::new()).get_host_name()
}

fn print_settings(yaml: &str, config_path: &Path) {
    print!("Listing current settings ");

    let config_path_str = config_path.to_string_lossy();
    let is_default_path =
        config_path_str == argv::DEFAULT_YAML_FILE || config_path_str == argv::DEFAULT_CONF_FILE;
    let does_default_exist =
        Path::new(argv::DEFAULT_YAML_FILE).exists() || Path::new(argv::DEFAULT_CONF_FILE).exists();

    if is_default_path && does_default_exist {
        print!("from default conf, ");
    } else if config_path.exists() {
        print!("from config ({}), ", config_path.display());
    } else {
        print!("from ")
    }

    println!("environment variables and command line options in yaml format");

    println!("{}", yaml);
    std::process::exit(0);
}

fn parse_k8s_tracking_or_warn(
    value: Option<String>,
    name: &str,
    default: K8sTrackingConf,
) -> K8sTrackingConf {
    if let Some(s) = value {
        match s.parse::<K8sTrackingConf>() {
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

#[cfg(test)]
mod tests {

    // Provide values for extern symbols PKG_NAME and PKG_VERSION
    // when building this module on it's own
    #[no_mangle]
    pub static PKG_NAME: &str = "test";
    #[no_mangle]
    pub static PKG_VERSION: &str = "test";

    use std::env;
    use std::fs::OpenOptions;
    use std::io::Write;

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
        let result = get_default_config();
        let user_agent = result.http.template.user_agent.to_str().unwrap();
        assert!(user_agent.contains('(') && user_agent.contains(')'));
    }

    #[test]
    fn test_db_path() {
        // Default
        assert_eq!(
            DbPath::from(None),
            DbPath::Path(PathBuf::from("/var/lib/logdna/"))
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
        let config = get_default_config();
        assert_eq!(config.log.use_k8s_enrichment, K8sTrackingConf::Always);
        assert_eq!(config.log.log_k8s_events, K8sTrackingConf::Never);
        assert_eq!(config.log.lookback, Lookback::None);
        assert_eq!(
            config
                .log
                .dirs
                .iter()
                .map(|p| p.to_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["/var/log/"]
        );
    }

    #[test]
    fn test_default_rules() {
        let config = get_default_config();

        let should_pass = vec![
            "/var/log/a.log",
            "/var/log/containers/a.log",
            "/var/log/custom/a.log",

            // These are not excluded in the rules, it's not on the default log dirs
            // so if a symlink points to it, it should pass
            "/var/data/a.log",
            "/tmp/app/a.log",
            "/var/data/kubeletlogs/some-named-service-aabb67c8fc-9ncjd_52c36bc5-4a53-4827-9dc8-082926ac1bc9/some-named-service/1.log",
        ];

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

        guard(file, |mut file| {
            let args = vec![OsString::new()];
            serde_yaml::to_writer(&mut file, &RawConfig::default()).unwrap();
            file.flush().unwrap();

            env::remove_var(env_vars::INGESTION_KEY);
            env::remove_var(env_vars::INGESTION_KEY_ALTERNATE);
            env::remove_var(env_vars::INCLUSION_RULES_DEPRECATED);
            env::set_var(env_vars::CONFIG_FILE, path);

            Config::process_logdna_env_vars();

            assert!(Config::new(args.clone()).is_err());

            env::set_var(env_vars::INGESTION_KEY, "ingestion_key_test");

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
}
