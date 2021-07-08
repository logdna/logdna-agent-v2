use crate::error::ConfigError;
use crate::{argv, get_hostname, properties};
use http::types::params::Params;
use humanize_rs::bytes::Bytes;
use serde::de::{Deserializer, Error, Unexpected, Visitor};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::File;
use std::io::{ErrorKind, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::vec::Vec;

pub fn filesize_deser<'de, D>(d: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    struct BytesVisitor;
    impl<'de> Visitor<'de> for BytesVisitor {
        type Value = u64;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("expecting byte-size")
        }

        fn visit_str<E>(self, s: &str) -> Result<u64, E>
        where
            E: Error,
        {
            let bytes = s
                .parse::<Bytes<u64>>()
                .map_err(|_| Error::invalid_value(Unexpected::Str(s), &self))?;

            Ok(bytes.size())
        }
    }

    struct OptionVisitor;
    impl<'de> Visitor<'de> for OptionVisitor {
        type Value = Option<u64>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("expecting option")
        }

        #[inline]
        fn visit_unit<E>(self) -> Result<Option<u64>, E>
        where
            E: Error,
        {
            Ok(None)
        }

        #[inline]
        fn visit_none<E>(self) -> Result<Option<u64>, E>
        where
            E: Error,
        {
            Ok(None)
        }

        #[inline]
        fn visit_some<D>(self, deserializer: D) -> Result<Option<u64>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_str(BytesVisitor).map(Some)
        }
    }

    d.deserialize_option(OptionVisitor)
}

fn merge_all_confs(
    confs: impl Iterator<Item = Result<Config, ConfigError>>,
) -> (Option<Config>, Vec<ConfigError>) {
    let default_conf = Some(Config::default());

    let mut result_conf: Option<Config> = None;
    let mut result_errs = Vec::new();
    for result in confs {
        match result {
            Ok(conf) => result_conf.merge(&Some(conf), &default_conf),
            Err(e) => result_errs.push(e),
        }
    }

    (result_conf, result_errs)
}

fn try_load_confs<'a>(
    paths: &'a [&Path],
) -> impl Iterator<Item = Result<Config, ConfigError>> + 'a {
    paths.iter().map(|path| {
        let mut conf_file = File::open(path)?;
        if let Ok(legacy_conf) = properties::read_file(&conf_file) {
            debug!("loading {} as a properties file", path.display());
            return Ok(legacy_conf);
        }

        debug!("loading {} as a yaml file", path.display());
        conf_file.seek(SeekFrom::Start(0))?;
        Ok(serde_yaml::from_reader(&conf_file)?)
    })
}

pub trait Merge {
    fn merge(&mut self, other: &Self, default: &Self);
}

impl Merge for PathBuf {
    fn merge(&mut self, other: &Self, default: &Self) {
        if *self != *other && *other != *default {
            *self = other.clone();
        }
    }
}

impl Merge for String {
    fn merge(&mut self, other: &Self, default: &Self) {
        if *self != *other && *other != *default {
            *self = other.clone();
        }
    }
}

impl Merge for bool {
    fn merge(&mut self, other: &Self, _default: &Self) {
        *self = *self || *other;
    }
}

impl Merge for u16 {
    fn merge(&mut self, other: &Self, default: &Self) {
        if *other != *default {
            *self = *other;
        }
    }
}

impl Merge for u32 {
    fn merge(&mut self, other: &Self, default: &Self) {
        if *other != *default {
            *self = *other;
        }
    }
}

impl Merge for u64 {
    fn merge(&mut self, other: &Self, default: &Self) {
        if *other != *default {
            *self = *other;
        }
    }
}

impl Merge for usize {
    fn merge(&mut self, other: &Self, default: &Self) {
        if *other != *default {
            *self = *other;
        }
    }
}

impl<T: PartialEq + Merge + Clone + Default> Merge for Option<T> {
    fn merge(&mut self, other: &Self, default: &Self) {
        if *other != *default {
            if let Some(other_value) = other {
                match self {
                    Some(self_value) => self_value.merge(other_value, &T::default()),
                    None => *self = Some(other_value.clone()),
                }
            }
        }
    }
}

impl Merge for Option<Params> {
    fn merge(&mut self, other: &Self, default: &Self) {
        if *other != *default && other.is_some() {
            *self = other.clone();
        }
    }
}

impl<T: PartialEq + Clone> Merge for Vec<T> {
    fn merge(&mut self, other: &Self, default: &Self) {
        if *other != *default {
            *self = other.clone();
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone, Default)]
pub struct Config {
    pub http: HttpConfig,
    pub log: LogConfig,
    pub journald: JournaldConfig,
    pub startup: K8sStartupLeaseConfig,
}

impl Config {
    /// Tries to parse from java properties format and then using
    pub fn parse<P: AsRef<Path>>(path: P) -> Result<Self, Vec<ConfigError>> {
        let path = path.as_ref();
        let is_default_path = path.to_string_lossy() == argv::DEFAULT_YAML_FILE;
        let default_conf_file = argv::default_conf_file();
        let conf_files = if is_default_path {
            vec![
                default_conf_file.as_ref(),
                Path::new(argv::DEFAULT_YAML_FILE),
            ]
        } else {
            vec![path]
        };

        let indiv_confs = try_load_confs(&conf_files);
        let (final_conf, error_list) = merge_all_confs(indiv_confs);
        for conf_err in &error_list {
            if !matches!(conf_err, ConfigError::Io(ref err) if is_default_path && err.kind() == ErrorKind::NotFound)
            {
                error!("error encountered loading configuration: {:?}", conf_err);
            }
        }

        // Default to returning the first error encountered as this would most likely be the first error
        // encounted in the old implementation. This will panic if there is nothing in the error_list but
        // that is probably fine since it's a state that should never be hit.
        match (final_conf, error_list) {
            (Some(final_conf), _) => Ok(final_conf),
            (None, error_list) if error_list.is_empty() => Ok(Config::default()),
            (None, error_list) if !error_list.is_empty() => Err(error_list),
            (None, _) => unreachable!(),
        }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_dir: Option<PathBuf>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "filesize_deser",
        default
    )]
    pub retry_disk_limit: Option<u64>,

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

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone, Default)]
pub struct K8sStartupLeaseConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub option: Option<String>,
}

impl Merge for K8sStartupLeaseConfig {
    fn merge(&mut self, other: &Self, default: &Self) {
        self.option.merge(&other.option, &default.option);
    }
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone, Default)]
pub struct JournaldConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<PathBuf>>,
}

impl Merge for JournaldConfig {
    fn merge(&mut self, other: &Self, default: &Self) {
        self.paths.merge(&other.paths, &default.paths);
    }
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone, Default)]
pub struct Rules {
    pub glob: Vec<String>,
    pub regex: Vec<String>,
}

impl Merge for Rules {
    fn merge(&mut self, other: &Self, default: &Self) {
        self.glob.merge(&other.glob, &default.glob);
        self.regex.merge(&other.regex, &default.regex);
    }
}

impl Merge for Config {
    fn merge(&mut self, other: &Self, default: &Self) {
        self.http.merge(&other.http, &default.http);
        self.log.merge(&other.log, &default.log);
        self.journald.merge(&other.journald, &default.journald);
        self.startup.merge(&other.startup, &default.startup);
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
            retry_dir: Some(PathBuf::from("/tmp/logdna")),
            retry_disk_limit: None,
            retry_base_delay_ms: None,
            retry_step_delay_ms: None,
        }
    }
}

impl Merge for HttpConfig {
    fn merge(&mut self, other: &Self, default: &Self) {
        self.host.merge(&other.host, &default.host);
        self.endpoint.merge(&other.endpoint, &default.endpoint);
        self.use_ssl.merge(&other.use_ssl, &default.use_ssl);
        self.timeout.merge(&other.timeout, &default.timeout);
        self.use_compression
            .merge(&other.use_compression, &default.use_compression);
        self.gzip_level
            .merge(&other.gzip_level, &default.gzip_level);
        self.ingestion_key
            .merge(&other.ingestion_key, &default.ingestion_key);
        self.params.merge(&other.params, &default.params);
        self.body_size.merge(&other.body_size, &default.body_size);
        self.retry_dir.merge(&other.retry_dir, &default.retry_dir);
        self.retry_disk_limit
            .merge(&other.retry_disk_limit, &default.retry_disk_limit);
        self.retry_base_delay_ms
            .merge(&other.retry_base_delay_ms, &default.retry_base_delay_ms);
        self.retry_step_delay_ms
            .merge(&other.retry_step_delay_ms, &default.retry_step_delay_ms);
    }
}

#[cfg(unix)]
fn default_log_dirs() -> Vec<PathBuf> {
    vec!["/var/log/".into()]
}

#[cfg(windows)]
fn default_log_dirs() -> Vec<PathBuf> {
    let default_str = std::env::var("ALLUSERSPROFILE").unwrap_or(r"C:\ProgramData".into());
    let default_os_str: std::ffi::OsString = default_str.into();
    vec![Path::new(&default_os_str).join("logs")]
}

impl Default for LogConfig {
    fn default() -> Self {
        LogConfig {
            dirs: default_log_dirs(),
            db_path: None,
            metrics_port: None,
            include: Some(Rules {
                glob: vec!["*.log".parse().unwrap()],
                regex: Vec::new(),
            }),
            #[cfg(unix)]
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
            #[cfg(windows)]
            exclude: None,
            line_exclusion_regex: None,
            line_inclusion_regex: None,
            line_redact_regex: None,
            lookback: None,
            use_k8s_enrichment: None,
            log_k8s_events: None,
        }
    }
}

impl Merge for LogConfig {
    fn merge(&mut self, other: &Self, default: &Self) {
        self.dirs.merge(&other.dirs, &default.dirs);
        self.db_path.merge(&other.db_path, &default.db_path);
        self.metrics_port
            .merge(&other.metrics_port, &default.metrics_port);
        self.include.merge(&other.include, &default.include);
        self.exclude.merge(&other.exclude, &default.exclude);
        self.line_exclusion_regex
            .merge(&other.line_exclusion_regex, &default.line_exclusion_regex);
        self.line_inclusion_regex
            .merge(&other.line_inclusion_regex, &default.line_inclusion_regex);
        self.line_redact_regex
            .merge(&other.line_redact_regex, &default.line_redact_regex);
        self.lookback.merge(&other.lookback, &default.lookback);
        self.use_k8s_enrichment
            .merge(&other.use_k8s_enrichment, &default.use_k8s_enrichment);
        self.log_k8s_events
            .merge(&other.log_k8s_events, &default.log_k8s_events);
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
        let k8s_config = K8sStartupLeaseConfig { option: None };
        assert!(new_config.is_ok());
        let new_config = new_config.unwrap();
        assert_eq!(config, new_config);
        assert_eq!(config.startup, k8s_config);
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
        assert_eq!(config.log.dirs, default_log_dirs());
        assert_eq!(config.http.ingestion_key, some_string!("123"));
        assert_eq!(
            config.http.params.unwrap().tags,
            Some(Tags::from(vec_strings!["production", "2ndtag"]))
        );

        let expected_exclude = LogConfig::default()
            .exclude
            .unwrap_or_default()
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
        assert_eq!(config.log.dirs, default_log_dirs());
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
            .unwrap_or_default()
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
        assert_eq!(config.log.dirs, default_log_dirs());
        assert_eq!(config.http.ingestion_key, some_string!("abcdef01"));
        assert_eq!(config.http.params.unwrap().tags, None);
        Ok(())
    }

    #[test]
    fn test_file_not_found() {
        assert!(matches!(
            Config::parse("/non/existent/path.conf").map_err(|es| es.into_iter().next().unwrap()),
            Err(ConfigError::Io(_))
        ));
    }

    #[test]
    fn test_empty() -> io::Result<()> {
        let dir = tempdir()?;
        let file_name = dir.path().join("test.conf");
        fs::write(&file_name, "")?;
        assert!(matches!(
            Config::parse(&file_name).map_err(|es| es.into_iter().next().unwrap()),
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
            Config::parse(&file_name).map_err(|es| es.into_iter().next().unwrap()),
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
            Config::parse(&file_name).map_err(|es| es.into_iter().next().unwrap()),
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
  retry_disk_limit: 3 MiB
log:
  dirs:
    - /var/log1/
    - /var/log2/
journald: {}
startup: {}
",
        )?;

        let config = Config::parse(&file_name).unwrap();
        assert_eq!(config.http.use_ssl, Some(false));
        assert_eq!(config.http.use_compression, Some(true));
        assert_eq!(config.http.host, some_string!("logs.logdna.prod"));
        assert_eq!(config.http.endpoint, some_string!("/path/to/endpoint1"));
        assert_eq!(config.http.use_compression, Some(true));
        assert_eq!(config.http.timeout, Some(12000));
        assert_eq!(config.http.retry_disk_limit, Some(3_145_728));
        let params = config.http.params.unwrap();
        assert_eq!(params.tags, Some(Tags::from("tag1,tag2")));
        assert_eq!(
            config.log.dirs,
            vec![PathBuf::from("/var/log1/"), PathBuf::from("/var/log2/")]
        );
        assert_eq!(config.startup, K8sStartupLeaseConfig { option: None });
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
redact_regex = \\\\S+@\\\\S+\\\\.\\\\S+
ingest_timeout = 9999
ingest_buffer_size = 3145728
",
        )?;
        let config = Config::parse(&file_name).unwrap();

        assert_eq!(config.http.use_compression, Some(false));
        assert_eq!(config.http.use_ssl, Some(true));
        assert_eq!(config.http.gzip_level, Some(4));
        assert_eq!(config.http.body_size, Some(3 * 1024 * 1024));
        assert_eq!(config.http.timeout, Some(9999));

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

    macro_rules! test_merge_standard_types {
        ($($name:ident: $values:expr,)*) => {
            mod merge_primitive_types {
                use super::super::*;
                $(
                    #[test]
                    fn $name() {
                        let (mut current_val, new_val, default_val) = $values;

                        current_val.merge(&new_val, &default_val);
                        assert_eq!(
                            current_val,
                            new_val,
                            "current value was not updated to match new value"
                        );

                        current_val.merge(&default_val, &default_val);
                        assert_ne!(
                            current_val,
                            default_val,
                            "default value should not replace current value"
                        );
                    }
                )*
            }
        }
    }

    test_merge_standard_types! {
        pathbuf: (PathBuf::from("/tmp/cur"), PathBuf::from("/tmp/new"), PathBuf::from("/tmp/default")),
        string: ("current".to_string(), "new".to_string(), "default".to_string()),
        bool: (false, true, false),
        u16: (100_u16, 5_u16, 0_u16),
        u32: (100_u32, 5_u32, 0_u32),
        u64: (100_u64, 5_u64, 0_u64),
        usize: (100_usize, 5_usize, 0_usize),
        vec: (vec!["a", "b", "c"], vec!["d"], vec!["e", "f"]),
    }

    #[test]
    fn option_merge() {
        // Case 1: Attempting to merge a None onto Some value should not
        // replace the Some value.
        let default_val = Some("default".to_string());
        let mut target = Some("case 1".to_string());
        target.merge(&None, &default_val);
        assert!(
            matches!(target, Some(val) if val == "case 1"),
            "None should not replace Some caller value"
        );

        // Case 2: Attempting to merge Some value onto a None value should
        // replace the None value.
        target = None;
        target.merge(&Some("new".to_string()), &default_val);
        assert!(
            matches!(target, Some(val) if val == "new"),
            "None caller should be replaced by Some value"
        );

        // Case 3: Attempting to merge a default value onto Some value should
        // not replace the Some value.
        target = Some("case 3".to_string());
        target.merge(&Some("default".to_string()), &default_val);
        assert!(
            matches!(target, Some(val) if val == "case 3"),
            "default value should not replace existing value"
        );

        // Case 4: With two non-default values, the values should merge according
        // to the underlying type.
        target = Some("case 4".to_string());
        target.merge(&Some("new case 4".to_string()), &default_val);
        assert!(
            matches!(target, Some(val) if val == "new case 4"),
            "new value should have replaced the existing value"
        );

        // Case 5: With two None values, the existing value should remain.
        target = None;
        target.merge(&None, &default_val);
        assert!(
            matches!(target, None),
            "None expected when merging with None"
        );
    }

    #[test]
    fn option_merge_param() {
        // Case 1: Merging None value with Some should not replace the Some value
        let default = Params::builder().hostname("default").build().ok();
        let mut target = Params::builder().hostname("target").build().ok();
        target.merge(&None, &default);
        assert_eq!(target.unwrap().hostname, "target".to_string());

        // Case 2: Merging Some value with None should replace the None
        let other = Params::builder().hostname("other").build().ok();
        target = None;
        target.merge(&other, &default);
        assert_eq!(target.unwrap().hostname, "other".to_string());

        // Case 3: Merging a default value with Some value should not override the
        // default value
        target = Params::builder().hostname("target").build().ok();
        target.merge(&default, &default);
        assert_eq!(target.unwrap().hostname, "target".to_string());

        // Case 4: Merging two values should have the target replaced by the new value
        target = Params::builder().hostname("target").build().ok();
        target.merge(&other, &default);
        assert_eq!(target.unwrap().hostname, "other".to_string());
    }

    #[test]
    fn journald_config_merge() {
        let mut left_conf = JournaldConfig {
            paths: Some(vec![Path::new("/left").to_path_buf()]),
        };

        let right_conf = JournaldConfig {
            paths: Some(vec![Path::new("/right").to_path_buf()]),
        };

        left_conf.merge(&right_conf, &JournaldConfig::default());

        let actual_paths = left_conf
            .paths
            .expect("expected paths to not be None after merge");
        assert_eq!(actual_paths, vec![PathBuf::from("/right")]);
    }

    #[test]
    fn rules_merge() {
        let mut left_conf = Rules {
            glob: vec!["left glob".to_string()],
            regex: vec!["left regex".to_string()],
        };

        let right_conf = Rules {
            glob: vec!["right glob".to_string()],
            regex: vec!["right regex".to_string()],
        };

        left_conf.merge(&right_conf, &Rules::default());

        assert_eq!(left_conf.glob, vec!["right glob".to_string()]);
        assert_eq!(left_conf.regex, vec!["right regex".to_string()]);
    }

    #[test]
    fn http_config_merge() {
        let mut left_conf = HttpConfig {
            host: Some("left.logdna.test".to_string()),
            endpoint: Some("/left/endpoint".to_string()),
            use_ssl: Some(false),
            timeout: Some(1337),
            use_compression: Some(false),
            gzip_level: Some(0),
            ingestion_key: Some("KEY".to_string()),
            params: Params::builder()
                .hostname("left.local".to_string())
                .build()
                .ok(),
            body_size: Some(1337),
            retry_dir: Some(PathBuf::from("/tmp/logdna/left")),
            retry_disk_limit: Some(12345),
            retry_base_delay_ms: Some(10_000),
            retry_step_delay_ms: Some(10_000),
        };

        let right_conf = HttpConfig {
            host: Some("right.logdna.test".to_string()),
            endpoint: Some("/right/endpoint".to_string()),
            use_ssl: Some(true),
            timeout: Some(7331),
            use_compression: Some(true),
            gzip_level: Some(1),
            ingestion_key: Some("VAL".to_string()),
            params: Params::builder()
                .hostname("right.local".to_string())
                .build()
                .ok(),
            body_size: Some(7331),
            retry_dir: Some(PathBuf::from("/tmp/logdna/right")),
            retry_disk_limit: Some(98765),
            retry_base_delay_ms: Some(2_000),
            retry_step_delay_ms: Some(2_000),
        };

        left_conf.merge(&right_conf, &HttpConfig::default());

        assert_eq!(left_conf.host, Some("right.logdna.test".to_string()));
        assert_eq!(left_conf.endpoint, Some("/right/endpoint".to_string()));
        assert_eq!(left_conf.use_ssl, Some(false)); // Note: default does not override value
        assert_eq!(left_conf.timeout, Some(7331));
        assert_eq!(left_conf.use_compression, Some(false)); // Note: default does not override value
        assert_eq!(left_conf.gzip_level, Some(1));
        assert_eq!(left_conf.ingestion_key, Some("VAL".to_string()));
        assert_eq!(
            left_conf.params.map(|p| p.hostname),
            Some("right.local".to_string())
        );
        assert_eq!(left_conf.body_size, Some(7331));
        assert_eq!(
            left_conf.retry_dir,
            Some(PathBuf::from("/tmp/logdna/right"))
        );
        assert_eq!(left_conf.retry_disk_limit, Some(98765));
        assert_eq!(left_conf.retry_base_delay_ms, Some(2_000));
        assert_eq!(left_conf.retry_step_delay_ms, Some(2_000));
    }

    #[test]
    fn merge_all_confs_empty_iter() {
        let empty_vec: Vec<Result<Config, ConfigError>> = Vec::new();
        let (actual_conf, actual_errs) = merge_all_confs(empty_vec.into_iter());
        assert!(actual_conf.is_none());
        assert!(actual_errs.is_empty());
    }

    #[test]
    fn merge_all_confs_only_errors() {
        let error_vec: Vec<Result<Config, ConfigError>> = vec![
            Err(ConfigError::MissingField("foo")),
            Err(ConfigError::PropertyInvalid(String::from("bar"))),
        ];
        let (actual_conf, actual_errs) = merge_all_confs(error_vec.into_iter());
        assert!(actual_conf.is_none());
        assert!(matches!(actual_errs[0], ConfigError::MissingField(value) if value == "foo"));
        assert!(matches!(&actual_errs[1], ConfigError::PropertyInvalid(value) if value == "bar"));
    }

    #[test]
    fn merge_all_confs_overrides_lower_config() {
        let mut lower_conf = Config::default();
        lower_conf.http.host = Some("lower_conf.api.logdna.test".to_string());

        let mut higher_conf = Config::default();
        higher_conf.http.host = Some("higher_conf.api.logdna.test".to_string());

        let (actual_conf, actual_errs) =
            merge_all_confs(vec![Ok(lower_conf), Ok(higher_conf)].into_iter());
        assert_eq!(
            actual_conf.unwrap().http.host,
            Some("higher_conf.api.logdna.test".to_string())
        );
        assert!(actual_errs.is_empty());
    }

    #[test]
    fn merge_all_confs_skip_override_with_default() {
        let mut lower_conf = Config::default();
        lower_conf.http.host = Some("lower_conf.api.logdna.test".to_string());

        let higher_conf = Config::default();

        let (actual_conf, actual_errs) =
            merge_all_confs(vec![Ok(lower_conf), Ok(higher_conf)].into_iter());
        assert_eq!(
            actual_conf.unwrap().http.host,
            Some("lower_conf.api.logdna.test".to_string())
        );
        assert!(actual_errs.is_empty());
    }

    #[test]
    fn megre_all_confs_mixed_config_and_errors() {
        let mut lower_conf = Config::default();
        lower_conf.http.host = Some("lower_conf.api.logdna.test".to_string());

        let mut higher_conf = Config::default();
        higher_conf.http.host = Some("higher_conf.api.logdna.test".to_string());

        let result_vec = vec![
            Ok(lower_conf),
            Err(ConfigError::MissingField("foo")),
            Ok(higher_conf),
        ];
        let (actual_conf, actual_errs) = merge_all_confs(result_vec.into_iter());

        assert_eq!(
            actual_conf.unwrap().http.host,
            Some("higher_conf.api.logdna.test".to_string())
        );
        assert_eq!(actual_errs.len(), 1);
        assert!(matches!(actual_errs[0], ConfigError::MissingField(value) if value == "foo"));
    }

    #[test]
    fn try_load_confs_empty_path_vector() {
        let conf_paths: Vec<&Path> = Vec::new();
        let result = try_load_confs(&conf_paths);
        assert_eq!(result.count(), 0);
    }

    #[test]
    fn try_load_confs_valid_config_files() -> io::Result<()> {
        let test_dir = tempdir()?;

        let legacy_conf_path = test_dir.path().join("legacy.conf");
        fs::write(
            &legacy_conf_path,
            "LOGDNA_LOGHOST = legacy.logdna.test
key = 1234567890
logdir = /var/my_log,/var/my_log2
tags = production, stable
exclude = /path/to/exclude/**, /second/path/to/exclude/**

exclude_regex = /a/regex/exclude.*
hostname = jorge's-laptop        
",
        )?;

        let new_conf_path = test_dir.path().join("new_conf.yaml");
        fs::write(
            &new_conf_path,
            "
http:
    host: new.logdna.test
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
startup: {}",
        )?;

        let conf_paths: Vec<&Path> = vec![&legacy_conf_path, &new_conf_path];
        let result: Vec<Result<Config, ConfigError>> = try_load_confs(&conf_paths).collect();

        assert_eq!(result.len(), 2);
        assert!(matches!(&result[0], Ok(_)));
        assert!(matches!(&result[1], Ok(_)));

        Ok(())
    }

    #[test]
    fn try_load_confs_invalid_files() -> io::Result<()> {
        let test_dir = tempdir()?;
        let missing_file = test_dir.path().join("ghost.yaml");

        let invalid_file = test_dir.path().join("invalid.dat");
        fs::write(&invalid_file, "!@##()#()(#)%%$^$^$*^)*%#*@$@)$#*%#*%)%)#")?;

        let conf_paths: Vec<&Path> = vec![&missing_file, &invalid_file];
        let result: Vec<Result<Config, ConfigError>> = try_load_confs(&conf_paths).collect();
        assert_eq!(result.len(), 2);
        assert!(matches!(result[0], Err(ConfigError::Io(_))));
        assert!(matches!(result[1], Err(ConfigError::Serde(_))));

        Ok(())
    }

    #[test]
    fn try_load_confs_completes_when_errors_exist() -> io::Result<()> {
        let test_dir = tempdir()?;
        let missing_file = test_dir.path().join("ghost.yaml");

        let legacy_conf_path = test_dir.path().join("legacy.conf");
        fs::write(
            &legacy_conf_path,
            "
LOGDNA_LOGHOST = legacy.logdna.test
key = 1234567890
logdir = /var/my_log,/var/my_log2
tags = production, stable
exclude = /path/to/exclude/**, /second/path/to/exclude/**

exclude_regex = /a/regex/exclude.*
hostname = jorge's-laptop        
",
        )?;

        let new_conf_path = test_dir.path().join("new_conf.yaml");
        fs::write(
            &new_conf_path,
            "
http:
    host: new.logdna.test
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
startup: {}",
        )?;

        let conf_paths: Vec<&Path> = vec![&legacy_conf_path, &missing_file, &new_conf_path];
        let result: Vec<Result<Config, ConfigError>> = try_load_confs(&conf_paths).collect();

        assert_eq!(result.len(), 3);
        assert!(matches!(result[0], Ok(_)));
        assert!(matches!(result[1], Err(ConfigError::Io(_))));
        assert!(matches!(result[2], Ok(_)));

        Ok(())
    }
}
