use crate::env_vars;
use crate::raw::{Config as RawConfig, Rules};
use crate::K8sLeaseConf;
use crate::K8sTrackingConf;
use fs::lookback::Lookback;
use http::types::params::{Params, Tags};
use humanize_rs::bytes::Bytes;
use std::env::var as env_var;
use std::ffi::OsString;
use std::path::PathBuf;
use structopt::StructOpt;

// Symbol that will be populated in the main.rs file
extern "Rust" {
    static PKG_VERSION: &'static str;
}

#[cfg(unix)]
pub const DEFAULT_YAML_FILE: &str = "/etc/logdna/config.yaml";

#[cfg(windows)]
pub const DEFAULT_YAML_FILE: &str = r"C:\ProgramData\logdna\config.yaml";

#[cfg(unix)]
pub fn default_conf_file() -> PathBuf {
    PathBuf::from("/etc/logdna.conf")
}

#[cfg(windows)]
pub fn default_conf_file() -> PathBuf {
    let default_str = std::env::var("ALLUSERSPROFILE").unwrap_or(r"C:\ProgramData".into());
    PathBuf::from(default_str)
        .join("logdna")
        .join("logdna.conf")
}

/// Contains the command and env var options.
#[derive(StructOpt, Debug, Default, PartialEq)]
// Using PKG_VERSION as a workaround while we centralize version management of packages
#[structopt(name = "LogDNA Agent", about = "A resource-efficient log collection agent that forwards logs to LogDNA.", version = unsafe { PKG_VERSION })]
pub struct ArgumentOptions {
    /// The ingestion key associated with your LogDNA account
    #[structopt(long, short, env = env_vars::INGESTION_KEY)]
    key: Option<String>,

    /// The config filename.
    /// When defined, it will try to parse in java properties format and in yaml format for
    /// backward compatibility.
    ///
    /// By default will look in the paths: /etc/logdna/config.yaml and /etc/logdna.conf on
    /// unix systems and %APPDATA% or C:\ProgramData on windows
    #[structopt(
        short,
        long,
        parse(from_os_str),
        env = env_vars::CONFIG_FILE,
        default_value = DEFAULT_YAML_FILE
    )]
    pub config: PathBuf,

    /// The host to forward logs to. Defaults to "logs.logdna.com"
    #[structopt(long, env = env_vars::HOST)]
    host: Option<String>,

    /// The endpoint to forward logs to. Defaults to "/logs/agent"
    #[structopt(long, env = env_vars::ENDPOINT)]
    endpoint_path: Option<String>,

    /// Determines whether to use TLS for sending logs. Defaults to "true".
    #[structopt(long, env = env_vars::USE_SSL)]
    use_ssl: Option<bool>,

    /// Determines whether to compress logs before sending. Defaults to "true".
    #[structopt(long, env = env_vars::USE_COMPRESSION)]
    use_compression: Option<bool>,

    /// If compression is enabled, this is the gzip compression level to use. Defaults to 2.
    #[structopt(long, env = env_vars::GZIP_LEVEL)]
    gzip_level: Option<u32>,

    /// The hostname metadata to attach to lines forwarded from this agent (defaults to
    /// os.hostname())
    #[structopt(long, env = env_vars::HOSTNAME)]
    os_hostname: Option<String>,

    /// The IP metadata to attach to lines forwarded from this agent
    #[structopt(long, env = env_vars::IP)]
    ip: Option<String>,

    /// The MAC metadata to attach to lines forwarded from this agent
    #[structopt(long = "mac-address", env = env_vars::MAC)]
    mac: Option<String>,

    /// Adds log directories to scan, in addition to the default.
    ///
    /// Defaults to "/var/log" on Linux and macOS and defaults to "C:\ProgramData\logs" on Windows.
    #[structopt(long = "logdir", short = "d", env = env_vars::LOG_DIRS)]
    log_dirs: Vec<String>,

    /// List of glob patterns to exclude files from monitoring, to add to the default set of
    /// exclusion rules.
    #[structopt(long = "exclude", env = env_vars::EXCLUSION_RULES)]
    exclusion_rules: Vec<String>,

    /// List of regex patterns to exclude files from monitoring
    #[structopt(long = "exclude-regex", env = env_vars::EXCLUSION_REGEX_RULES)]
    exclusion_regex: Vec<String>,

    /// List of glob patterns to includes files for monitoring, to add to the default set of
    /// inclusion rules (*.log)
    #[structopt(long = "include", env = env_vars::INCLUSION_RULES)]
    inclusion_rules: Vec<String>,

    /// List of regex patterns to include files from monitoring
    #[structopt(long = "include-regex", env = env_vars::INCLUSION_REGEX_RULES)]
    inclusion_regex: Vec<String>,

    /// List of paths (directories or files) of journald paths to monitor,
    /// for example: /var/log/journal or /run/systemd/journal
    #[structopt(long, env = env_vars::JOURNALD_PATHS)]
    journald_paths: Vec<String>,

    /// The lookback strategy on startup ("smallfiles", "start" or "none").
    /// Defaults to "smallfiles".
    #[structopt(long, env = env_vars::LOOKBACK)]
    lookback: Option<Lookback>,

    /// List of tags metadata to attach to lines forwarded from this agent
    #[structopt(long, short, env = env_vars::TAGS)]
    tags: Vec<String>,

    /// Determines whether the agent should query the K8s API to enrich log lines from
    /// other pods ("always" or "never").  Defaults to "always".
    #[structopt(long, env = env_vars::USE_K8S_LOG_ENRICHMENT)]
    use_k8s_enrichment: Option<K8sTrackingConf>,

    /// Determines whether  the agent should log Kubernetes resource events. This setting only
    /// affects tracking and logging Kubernetes resource changes via watches. When disabled,
    /// the agent may still query k8s metadata to enrich log lines from other pods depending on
    /// the value of `use_k8s_enrichment` setting value ("always" or "never"). Defaults to "never".
    #[structopt(long, env = env_vars::LOG_K8S_EVENTS)]
    log_k8s_events: Option<K8sTrackingConf>,

    /// Determine whether or not to look for available K8s startup leases before attempting
    /// to start the agent; used to throttle startup on very large K8s clusters.
    /// Defaults to "off".
    #[structopt(long = "startup-lease", env = env_vars::K8S_STARTUP_LEASE)]
    k8s_startup_lease: Option<K8sLeaseConf>,

    /// The directory in which the agent will store its state database. Note that the agent must
    /// have write access to the directory and be a persistent volume.
    /// Defaults to "/var/lib/logdna-agent/" on unix systems and %APPDATA% or C:\ProgramData\logdna\ on windows
    #[structopt(long, env = env_vars::DB_PATH)]
    db_path: Option<String>,

    /// The port number to expose a Prometheus endpoint target with the agent metrics.
    #[structopt(long, env = env_vars::METRICS_PORT)]
    metrics_port: Option<u16>,

    /// List of regex patterns to exclude log lines.
    /// When set, the Agent will NOT send log lines that match any of these patterns.
    #[structopt(long, env = env_vars::LINE_EXCLUSION)]
    line_exclusion: Vec<String>,

    /// List of regex patterns to include log lines.
    /// When set, the Agent will send ONLY log lines that match any of these patterns.
    #[structopt(long, env = env_vars::LINE_INCLUSION)]
    line_inclusion: Vec<String>,

    /// List of Kubernetes pod metadata to include in log lines.
    #[structopt(long = "k8-metadata-line-inclusion", env = env_vars::K8S_METADATA_LINE_INCLUSION)]
    k8s_metadata_line_inclusion: Option<Vec<String>>,

    /// List of Kubernetes pod metadata to exclude in log lines.
    #[structopt(long = "k8s-metadata-line-exclusion", env = env_vars::K8S_METADATA_LINE_EXCLUSION)]
    k8s_metadata_line_exclusion: Option<Vec<String>>,

    /// List of regex patterns used to mask matching sensitive information (such as PII) before
    /// sending it in the log line.
    #[structopt(long, env = env_vars::REDACT)]
    line_redact: Vec<String>,

    /// Show the current agent settings from the configuration sources (default config file
    /// and environment variables).
    #[structopt(short = "l", long = "list")]
    pub list_settings: bool,

    /// The timeout on requests to the ingestion API in milliseconds.
    /// Defaults to 10000 ms.
    #[structopt(long, env = env_vars::INGEST_TIMEOUT)]
    ingest_timeout: Option<u64>,

    /// The maximum size, in bytes, of log content that will be sent to the ingestion API.
    /// Defaults to 2097152 (2 MB).
    #[structopt(long, env = env_vars::INGEST_BUFFER_SIZE)]
    ingest_buffer_size: Option<usize>,

    /// The location where retry data is stored before successfully sent to the ingestion API.
    /// Defaults to /tmp/logdna.
    #[structopt(long, env = env_vars::RETRY_DIR)]
    retry_dir: Option<String>,

    /// When set, limits the amount of disk space the agent will use to store log lines that
    /// need to be resent to the ingestion API. Values can be defined with units of KB, MB, GB,
    /// etc. Numbers need to be integer values.
    #[structopt(long, env = env_vars::RETRY_DISK_LIMIT)]
    retry_disk_limit: Option<Bytes<u64>>,
}

impl ArgumentOptions {
    /// Overrides the `RawConfig` (yaml config) with the values that were defined via
    /// command line options or environment variables.
    pub fn merge(self, mut raw: RawConfig) -> RawConfig {
        if self.host.is_some() {
            raw.http.host = self.host;
        }

        if self.endpoint_path.is_some() {
            raw.http.endpoint = self.endpoint_path;
        }

        if self.key.is_some() {
            raw.http.ingestion_key = self.key;
        }

        if self.use_ssl.is_some() {
            raw.http.use_ssl = self.use_ssl;
        }

        if self.use_compression.is_some() {
            raw.http.use_compression = self.use_compression;
        }

        if self.gzip_level.is_some() {
            raw.http.gzip_level = self.gzip_level;
        }

        let mut params = raw.http.params.take().unwrap_or_else(Params::builder);
        if let Some(v) = self.os_hostname {
            params.hostname(v);
        }
        if let Some(ip) = self.ip {
            params.ip(ip);
        }
        if let Some(mac) = self.mac {
            params.mac(mac);
        }

        if !self.tags.is_empty() {
            // Params should always be valid here
            let mut tags = params
                .build()
                .ok()
                .and_then(|mut p| p.tags.take())
                .unwrap_or_else(Tags::new);
            with_csv(self.tags).iter().for_each(|v| {
                tags.add(v);
            });
            params.tags(tags);
        }
        raw.http.params = Some(params);

        if self.ingest_timeout.is_some() {
            raw.http.timeout = self.ingest_timeout;
        }

        if self.ingest_buffer_size.is_some() {
            raw.http.body_size = self.ingest_buffer_size;
        }

        if self.retry_dir.is_some() {
            raw.http.retry_dir = self.retry_dir.map(PathBuf::from);
        }

        if let Some(disk_limit) = self.retry_disk_limit {
            raw.http.retry_disk_limit = Some(disk_limit.size());
        }

        if !self.log_dirs.is_empty() {
            with_csv(self.log_dirs)
                .iter()
                .for_each(|v| raw.log.dirs.push(PathBuf::from(v)));
        }

        if let Some(ref p) = self.db_path {
            raw.log.db_path = Some(p.into())
        }

        if let Some(port) = self.metrics_port {
            raw.log.metrics_port = Some(port)
        }

        set_rules(
            &mut raw.log.exclude,
            self.exclusion_rules,
            self.exclusion_regex,
        );
        set_rules(
            &mut raw.log.include,
            self.inclusion_rules,
            self.inclusion_regex,
        );

        if !self.journald_paths.is_empty() {
            let paths = raw.journald.paths.get_or_insert(Vec::new());
            with_csv(self.journald_paths)
                .iter()
                .for_each(|v| paths.push(PathBuf::from(v)));
        }

        if self.lookback.is_some() {
            raw.log.lookback = self.lookback.map(|v| v.to_string());
        }

        if self.use_k8s_enrichment.is_some() {
            raw.log.use_k8s_enrichment = self.use_k8s_enrichment.map(|v| v.to_string());
        }

        if self.log_k8s_events.is_some() {
            raw.log.log_k8s_events = self.log_k8s_events.map(|v| v.to_string());
        }

        if self.k8s_startup_lease.is_some() {
            raw.startup.option = self.k8s_startup_lease.map(|v| v.to_string());
        }

        if self.db_path.is_some() {
            raw.log.db_path = self.db_path.map(PathBuf::from);
        }

        if !self.line_exclusion.is_empty() {
            let regex = raw.log.line_exclusion_regex.get_or_insert(Vec::new());
            with_csv(self.line_exclusion)
                .iter()
                .for_each(|v| regex.push(v.clone()));
        }

        if !self.line_inclusion.is_empty() {
            let regex = raw.log.line_inclusion_regex.get_or_insert(Vec::new());
            with_csv(self.line_inclusion)
                .iter()
                .for_each(|v| regex.push(v.clone()));
        }

        if self.k8s_metadata_line_inclusion.is_some() {
            let values = raw.log.k8s_metadata_include.get_or_insert(Vec::new());
            with_csv(self.k8s_metadata_line_inclusion.unwrap())
                .iter()
                .for_each(|v| values.push(v.clone()));
        }

        if self.k8s_metadata_line_exclusion.is_some() {
            let values = raw.log.k8s_metadata_exclude.get_or_insert(Vec::new());
            with_csv(self.k8s_metadata_line_exclusion.unwrap())
                .iter()
                .for_each(|v| values.push(v.clone()));
        }

        if !self.line_redact.is_empty() {
            let regex = raw.log.line_redact_regex.get_or_insert(Vec::new());
            with_csv(self.line_redact)
                .iter()
                .for_each(|v| regex.push(v.clone()));
        }

        raw
    }

    /// Parse command line options, default env vars and additional (deprecated) env vars.
    pub fn from_args_with_all_env_vars<I>(iter: I) -> ArgumentOptions
    where
        I: IntoIterator,
        I::Item: Into<OsString> + Clone,
    {
        let options: ArgumentOptions = ArgumentOptions::from_iter(iter);
        ArgumentOptions::parse_deprecated(options)
    }

    fn parse_deprecated(options: ArgumentOptions) -> ArgumentOptions {
        let mut options = options;
        if let Ok(v) = env_var(env_vars::INGESTION_KEY_ALTERNATE) {
            // Do not warn about alternate name for key
            options.key = Some(v);
        }

        macro_rules! deprecated_env {
            ($key: ident, $var_name: ident, Option<$ftype: ty>) => {
                if let Ok(v) = env_var(env_vars::$var_name) {
                    if let Ok(parsed) = std::str::FromStr::from_str(&v) {
                        options.$key = Some(parsed);
                        info!("Using deprecated env var '$var_name'");
                    } else {
                        warn!("Deprecated env var '$var_name' could not be parsed to $ftype");
                    }
                }
            };
            ($key: ident, $var_name: ident, $ftype: ty) => {
                if let Ok(v) = env_var(env_vars::$var_name) {
                    if let Ok(parsed) = std::str::FromStr::from_str(&v) {
                        options.$key = parsed;
                        info!("Using deprecated env var '$var_name'");
                    } else {
                        warn!("Deprecated env var '$var_name' could not be parsed to $ftype");
                    }
                }
            };
        }

        macro_rules! deprecated_env_vec {
            ($key: ident, $var_name: ident) => {
                if let Ok(v) = env_var(env_vars::$var_name) {
                    options.$key = with_csv(vec![v]);
                    info!("Using deprecated env var '$var_name'");
                }
            };
        }

        deprecated_env!(config, CONFIG_FILE_DEPRECATED, String);
        deprecated_env!(host, HOST_DEPRECATED, Option<String>);
        deprecated_env!(host, IBM_HOST_DEPRECATED, Option<String>);
        deprecated_env!(endpoint_path, ENDPOINT_DEPRECATED, Option<String>);
        deprecated_env!(use_ssl, USE_SSL_DEPRECATED, Option<bool>);
        deprecated_env!(use_compression, USE_COMPRESSION_DEPRECATED, Option<bool>);
        deprecated_env!(gzip_level, GZIP_LEVEL_DEPRECATED, Option<u32>);
        deprecated_env_vec!(log_dirs, LOG_DIRS_DEPRECATED);
        deprecated_env_vec!(exclusion_rules, EXCLUSION_RULES_DEPRECATED);
        deprecated_env_vec!(exclusion_regex, EXCLUSION_REGEX_RULES_DEPRECATED);
        deprecated_env_vec!(inclusion_rules, INCLUSION_RULES_DEPRECATED);
        deprecated_env_vec!(inclusion_regex, INCLUSION_REGEX_RULES_DEPRECATED);

        options
    }
}

fn set_rules(existing: &mut Option<Rules>, glob: Vec<String>, regex: Vec<String>) {
    let rules = existing.get_or_insert(Rules::default());
    rules.glob.append(&mut with_csv(glob));
    rules.regex.append(&mut with_csv(regex));
}

pub fn split_by_comma(v: &str) -> Vec<String> {
    with_csv(vec![v.to_string()])
}

fn with_csv(mut values: Vec<String>) -> Vec<String> {
    if values.len() != 1 {
        // The user can either use a single value with commas
        // or multiple values (i.e. from command line spaces)
        // but we don't support mixing.
        return values;
    }

    let v = values.remove(0);
    let mut s = v.as_str();
    let mut escaped = String::new();

    // regex crate doesn't feature negative lookbehind, use find indexes
    if v.contains(',') {
        let mut result: Vec<String> = Vec::new();
        while let Some(index) = s.find(',') {
            if index > 0 {
                if s.chars().nth(index - 1).unwrap() == '\\' {
                    // Store escaped "\," and continue
                    escaped = format!("{}{},", escaped, &s[..index - 1]);
                    s = &s[index + 1..];
                    continue;
                }

                let token = &s[0..index];

                result.push(combine(&escaped, token));
                escaped = String::new();
            }
            s = &s[index + 1..];
        }

        if !escaped.is_empty() || !s.is_empty() {
            result.push(combine(&escaped, s));
        }

        return result;
    }

    vec![v]
}

fn combine(escaped: &str, token: &str) -> String {
    if escaped.is_empty() {
        token.trim().to_string()
    } else {
        format!("{}{}", escaped.trim_start(), token.trim_end())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use crate::raw::{Config as RawConfig, K8sStartupLeaseConfig, Rules};

    use humanize_rs::bytes::Unit;

    use std::env::set_var;

    #[cfg(unix)]
    static EXCLUSION_GLOB_DEFAULT: &str = "/var/log/wtmp,/var/log/btmp,/var/log/utmp,/var/log/wtmpx,/var/log/btmpx,/var/log/utmpx,/var/log/asl/**,/var/log/sa/**,/var/log/sar*,/var/log/tallylog,/var/log/fluentd-buffers/**/*,/var/log/pods/**/*";

    #[cfg(unix)]
    static DEFAULT_LOG_DIR: &str = "/var/log";
    #[cfg(windows)]
    static DEFAULT_LOG_DIR: &str = r"C:\ProgramData\logs";

    macro_rules! vec_strings {
        ($($str:expr),*) => ({
            vec![$(String::from($str),)*] as Vec<String>
        });
    }

    macro_rules! vec_paths {
        ($($str:expr),*) => ({
            vec![$(PathBuf::from($str),)*] as Vec<PathBuf>
        });
    }

    macro_rules! some_string {
        ($val: expr) => {
            Some($val.to_string())
        };
    }

    #[test]
    fn test_with_csv() {
        assert_eq!(with_csv(vec_strings!["a,b"]), vec_strings!["a", "b"]);
        assert_eq!(with_csv(vec_strings![",a"]), vec_strings!["a"]);
        assert_eq!(with_csv(vec_strings!["a,"]), vec_strings!["a"]);
        assert_eq!(with_csv(vec_strings![r"a,b\,c"]), vec_strings!["a", "b,c"]);
        assert_eq!(with_csv(vec_strings![r"a\,b\,c"]), vec_strings!["a,b,c"]);
        assert_eq!(with_csv(vec_strings![r"a\,b,c"]), vec_strings!["a,b", "c"]);
        assert_eq!(
            with_csv(vec_strings![r"a\\,b\\,c"]),
            vec_strings![r"a\,b\,c"]
        );
        assert_eq!(
            with_csv(vec_strings![" a,b, c, d "]),
            vec_strings!["a", "b", "c", "d"]
        );
        assert_eq!(
            with_csv(vec_strings![r"a,b\, c, d"]),
            vec_strings!["a", "b, c", "d"]
        );
    }

    #[test]
    fn merge_should_combine_existing_tags() {
        let mut config = RawConfig::default();
        let mut params = Params::builder();
        params.hostname("");
        params.tags(Tags::from(vec!["a".to_owned()]));
        config.http.params = Some(params);
        let options = ArgumentOptions {
            tags: vec!["b".to_owned()],
            ..Default::default()
        };
        let config = options.merge(config);

        assert_eq!(
            config
                .http
                .params
                .unwrap()
                .build()
                .as_ref()
                .unwrap()
                .tags
                .as_ref()
                .unwrap(),
            &Tags::from(vec_strings!["a", "b"])
        );
    }

    #[test]
    fn merge_use_tags_when_empty() {
        let options = ArgumentOptions {
            tags: vec!["b".to_owned()],
            ..Default::default()
        };
        let mut config = options.merge(RawConfig::default());

        config.http.params.as_mut().unwrap().hostname("");
        assert_eq!(
            config
                .http
                .params
                .unwrap()
                .build()
                .as_ref()
                .unwrap()
                .tags
                .as_ref()
                .unwrap(),
            &Tags::from(vec_strings!["b"])
        );
    }

    #[test]
    fn merge_should_leave_tags_when_empty() {
        let mut config = RawConfig::default();
        let mut params = Params::builder();
        params.hostname("");
        params.tags(Tags::from(vec!["a".to_owned()]));
        config.http.params = Some(params);
        let options = ArgumentOptions::default();
        let config = options.merge(config);

        assert_eq!(
            config
                .http
                .params
                .unwrap()
                .build()
                .as_ref()
                .unwrap()
                .tags
                .as_ref()
                .unwrap(),
            &Tags::from(vec_strings!["a"])
        );
    }

    #[test]
    fn merge_should_separate_tags_by_comma() {
        let mut config = RawConfig::default();
        let mut params = Params::builder();
        params.hostname("");
        params.tags(Tags::from(vec!["a".to_owned()]));
        config.http.params = Some(params);
        let options = ArgumentOptions {
            tags: vec!["b,c".to_owned()],
            ..Default::default()
        };
        let config = options.merge(config);

        assert_eq!(
            config
                .http
                .params
                .unwrap()
                .build()
                .as_ref()
                .unwrap()
                .tags
                .as_ref()
                .unwrap(),
            &Tags::from(vec_strings!["a", "b", "c"])
        );
    }

    #[test]
    fn defaults_should_be_as_defined_in_raw_config() {
        let config = ArgumentOptions::default().merge(RawConfig::default());
        assert_eq!(config.http.host, some_string!("logs.logdna.com"));
        assert_eq!(config.http.endpoint, some_string!("/logs/agent"));
        assert_eq!(config.http.use_ssl, Some(true));
        assert_eq!(config.http.timeout, Some(10_000));
        assert_eq!(config.http.use_compression, Some(true));
        assert_eq!(config.http.gzip_level, Some(2));
        assert_eq!(config.http.body_size, Some(2 * 1024 * 1024));
        assert_eq!(config.log.lookback, None);
        assert_eq!(config.log.dirs, vec![PathBuf::from(DEFAULT_LOG_DIR)]);
        assert_eq!(
            config.log.include,
            Some(Rules {
                glob: vec!["*.log".parse().unwrap()],
                regex: Vec::new(),
            })
        );
        assert_eq!(config.log.use_k8s_enrichment, None);
        assert_eq!(config.log.log_k8s_events, None);
        assert_eq!(config.log.k8s_metadata_include, None);
        assert_eq!(config.log.k8s_metadata_exclude, None);
        assert_eq!(config.log.db_path, None);
        assert_eq!(config.log.metrics_port, None);
        assert_eq!(config.startup, K8sStartupLeaseConfig { option: None });
    }

    #[test]
    fn merge_should_override_values_from_config_file() {
        let argv = ArgumentOptions {
            log_dirs: vec_strings!("/my/path", "/my/other/path"),
            os_hostname: some_string!("my_host"),
            host: some_string!("server_host"),
            endpoint_path: some_string!("/path/to/endpoint"),
            use_ssl: Some(false),
            use_compression: Some(true),
            gzip_level: Some(3),
            ip: some_string!("1.2.3.4"),
            mac: some_string!("ac::dc"),
            db_path: some_string!("a/b/c"),
            metrics_port: Some(9089),
            tags: vec_strings!("a", "b"),
            lookback: Some(Lookback::Start),
            use_k8s_enrichment: Some(K8sTrackingConf::Always),
            log_k8s_events: Some(K8sTrackingConf::Never),
            journald_paths: vec_strings!("/a"),
            k8s_startup_lease: Some(K8sLeaseConf::Always),
            ingest_timeout: Some(1111111),
            ingest_buffer_size: Some(222222),
            retry_dir: some_string!("/tmp/argv"),
            retry_disk_limit: Some(Bytes::new(123456, Unit::Byte).unwrap()),
            ..ArgumentOptions::default()
        };
        let config = argv.merge(RawConfig::default());
        assert_eq!(config.http.host, some_string!("server_host"));
        assert_eq!(config.http.endpoint, some_string!("/path/to/endpoint"));
        assert_eq!(config.http.use_ssl, Some(false));
        assert_eq!(config.http.use_compression, Some(true));
        assert_eq!(config.http.gzip_level, Some(3));
        assert_eq!(config.http.timeout, Some(1111111));
        assert_eq!(config.http.body_size, Some(222222));
        assert_eq!(config.http.retry_dir, Some(PathBuf::from("/tmp/argv")));
        assert_eq!(config.http.retry_disk_limit, Some(123456));
        let params = config.http.params.unwrap().build().unwrap();
        assert_eq!(params.hostname, "my_host");
        assert_eq!(params.tags, Some(Tags::from(vec_strings!("a", "b"))));
        assert_eq!(params.ip, some_string!("1.2.3.4"));
        assert_eq!(params.mac, some_string!("ac::dc"));
        assert_eq!(
            config.log.dirs,
            vec_paths![DEFAULT_LOG_DIR, "/my/path", "/my/other/path"]
        );
        assert_eq!(config.log.lookback, some_string!("start"));
        assert_eq!(config.log.use_k8s_enrichment, some_string!("always"));
        assert_eq!(config.log.log_k8s_events, some_string!("never"));
        assert_eq!(config.log.db_path, Some(PathBuf::from("a/b/c")));
        assert_eq!(config.log.metrics_port, Some(9089));
        assert_eq!(config.journald.paths, Some(vec_paths!["/a"]));
        assert_eq!(config.startup.option, Some(String::from("always")));
    }

    #[test]
    fn merge_should_separate_values_by_comma() {
        let argv = ArgumentOptions {
            log_dirs: vec_strings!("/my/path,/other"),
            journald_paths: vec_strings!("/a,/b"),
            ..ArgumentOptions::default()
        };
        let config = argv.merge(RawConfig::default());
        assert_eq!(
            config.log.dirs,
            vec_paths![DEFAULT_LOG_DIR, "/my/path", "/other"]
        );
        assert_eq!(config.journald.paths, Some(vec_paths!["/a", "/b"]));
    }

    #[test]
    fn merge_regex_and_globs() {
        let argv = ArgumentOptions {
            exclusion_rules: vec_strings!["/my/path,/other"],
            exclusion_regex: vec_strings!["a", "b"],
            inclusion_rules: vec_strings!["included.ext", "another.*"],
            inclusion_regex: vec_strings!["a,b, c"],
            line_exclusion: vec_strings!["d,e, f"],
            line_inclusion: vec_strings![" g, h, i"],
            line_redact: vec_strings!["j,k"],
            ..ArgumentOptions::default()
        };
        let config = argv.merge(RawConfig::default());
        let exclusion = config.log.exclude.unwrap();
        let inclusion = config.log.include.unwrap();

        #[cfg(unix)]
        let expected_exclusion = EXCLUSION_GLOB_DEFAULT
            .split(',')
            .map(|x| x.to_string())
            .chain(vec_strings!["/my/path", "/other"])
            .collect::<Vec<String>>();

        #[cfg(windows)]
        let expected_exclusion = vec_strings!["/my/path", "/other"];

        assert_eq!(exclusion.glob, expected_exclusion);
        assert_eq!(exclusion.regex, vec_strings!["a", "b"]);

        assert_eq!(
            inclusion.glob,
            vec_strings!["*.log", "included.ext", "another.*"]
        );
        assert_eq!(inclusion.regex, vec_strings!["a", "b", "c"])
    }

    #[test]
    fn merge_line_regex() {
        let argv = ArgumentOptions {
            line_exclusion: vec_strings!["d,e, f"],
            line_inclusion: vec_strings![" g, h, i"],
            line_redact: vec_strings![r"j\,k,l"],
            ..ArgumentOptions::default()
        };
        let config = argv.merge(RawConfig::default());

        assert_eq!(
            config.log.line_exclusion_regex,
            Some(vec_strings!["d", "e", "f"])
        );
        assert_eq!(
            config.log.line_inclusion_regex,
            Some(vec_strings!["g", "h", "i"])
        );
        assert_eq!(config.log.line_redact_regex, Some(vec_strings!["j,k", "l"]));
    }

    #[test]
    fn merge_k8s_inclusion_exclusion() {
        let argv = ArgumentOptions {
            k8s_metadata_line_inclusion: Some(vec_strings![
                "namespace:test-namespace",
                "label.app:test-name"
            ]),
            k8s_metadata_line_exclusion: Some(vec_strings![
                "name:another-namespace",
                "annotation:another-name"
            ]),
            ..ArgumentOptions::default()
        };
        let config = argv.merge(RawConfig::default());

        assert_eq!(
            config.log.k8s_metadata_include,
            Some(vec_strings![
                "namespace:test-namespace",
                "label.app:test-name"
            ])
        );
        assert_eq!(
            config.log.k8s_metadata_exclude,
            Some(vec_strings![
                "name:another-namespace",
                "annotation:another-name"
            ])
        );
    }

    #[test]
    fn merge_paths() {
        let argv = ArgumentOptions {
            log_dirs: vec_strings!("/my/path", "/my/other/path"),
            journald_paths: vec_strings!("/journal,/journald"),
            ..ArgumentOptions::default()
        };
        let mut config = RawConfig::default();
        config.log.dirs = vec_paths!["/log_dir"];
        config.journald.paths = Some(vec_paths!["/default_journald"]);
        let config = argv.merge(config);

        assert_eq!(
            config.log.dirs,
            vec_paths!["/log_dir", "/my/path", "/my/other/path"]
        );
        assert_eq!(
            config.journald.paths,
            Some(vec_paths!["/default_journald", "/journal", "/journald"])
        );
    }

    #[test]
    fn parse_deprecated_test() {
        // Avoid reusing the constants from above on purpose
        // to prevent accidental name changes
        set_var("LOGDNA_AGENT_KEY", "123");
        set_var("DEFAULT_CONF_FILE", "a/b/c.yaml");
        set_var("LDLOGHOST", "abc");
        set_var("LDLOGPATH", "def");
        set_var("COMPRESS", "true");
        set_var("GZIP_COMPRESS_LEVEL", "1");
        set_var("LOG_DIRS", "a/b/c,/d/");
        set_var("LOGDNA_EXCLUDE", "ghi, a");
        set_var("LOGDNA_EXCLUDE_REGEX", "jkl,b");
        set_var("LOGDNA_INCLUDE", "mno,c");
        set_var("LOGDNA_INCLUDE_REGEX", "pqr,d");

        let options = ArgumentOptions::parse_deprecated(ArgumentOptions::default());
        assert_eq!(options.key, some_string!("123"));
        assert_eq!(options.config, PathBuf::from("a/b/c.yaml"));
        assert_eq!(options.host, some_string!("abc"));
        assert_eq!(options.endpoint_path, some_string!("def"));
        assert_eq!(options.use_compression, Some(true));
        assert_eq!(options.gzip_level, Some(1));
        assert_eq!(options.log_dirs, vec_strings!["a/b/c", "/d/"]);
        assert_eq!(options.exclusion_rules, vec_strings!["ghi", "a"]);
        assert_eq!(options.exclusion_regex, vec_strings!["jkl", "b"]);
        assert_eq!(options.inclusion_rules, vec_strings!["mno", "c"]);
        assert_eq!(options.inclusion_regex, vec_strings!["pqr", "d"]);
    }

    #[test]
    fn argument_options_equality() {
        let empty = ArgumentOptions {
            ..ArgumentOptions::default()
        };
        let with_key1 = ArgumentOptions {
            key: some_string!("123"),
            ..ArgumentOptions::default()
        };
        let with_key2 = ArgumentOptions {
            key: some_string!("123"),
            ..ArgumentOptions::default()
        };
        let with_key3 = ArgumentOptions {
            key: some_string!("00"),
            ..ArgumentOptions::default()
        };
        assert_eq!(empty, ArgumentOptions::default());
        assert_ne!(empty, with_key1);
        assert_eq!(with_key1, with_key2);
        assert_ne!(with_key1, with_key3);
    }
}
