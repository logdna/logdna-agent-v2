use crate::argv;
use crate::env_vars;
use crate::error::ConfigError;
use crate::raw::{Config, JournaldConfig, K8sStartupLeaseConfig, Rules};
use http::types::params::{Params, Tags};
use humanize_rs::bytes::Bytes;
use java_properties::PropertiesIter;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::str::FromStr;
use tracing::debug;

macro_rules! from_env_name {
    ($key: ident) => {
        static $key: Key = Key::FromEnv(env_vars::$key);
    };
}

// Names from the legacy agent
static INGESTION_KEY: Key = Key::Name("key");
static LOG_DIRS: Key = Key::Name("logdir");
static TAGS: Key = Key::Name("tags");
static OS_HOSTNAME: Key = Key::Name("hostname");
static EXCLUSION_RULES: Key = Key::Name("exclude");
static EXCLUSION_REGEX_RULES: Key = Key::Name("exclude_regex");
static IBM_HOST_DEPRECATED: Key = Key::Name(env_vars::IBM_HOST_DEPRECATED);

// New keys: reuse the same name from env vars, removing the MZ_ prefix and
// setting it to lowercase
from_env_name!(ENDPOINT);
from_env_name!(HOST);
from_env_name!(USE_SSL);
from_env_name!(USE_COMPRESSION);
from_env_name!(GZIP_LEVEL);
from_env_name!(INCLUSION_RULES);
from_env_name!(INCLUSION_REGEX_RULES);
from_env_name!(IP);
from_env_name!(MAC);
from_env_name!(JOURNALD_PATHS);
from_env_name!(SYSTEMD_JOURNAL_TAILER);
from_env_name!(LOOKBACK);
from_env_name!(DB_PATH);
from_env_name!(METRICS_PORT);
from_env_name!(USE_K8S_LOG_ENRICHMENT);
from_env_name!(LOG_K8S_EVENTS);
from_env_name!(K8S_METADATA_LINE_INCLUSION);
from_env_name!(K8S_METADATA_LINE_EXCLUSION);
from_env_name!(LOG_METRIC_SERVER_STATS);
from_env_name!(K8S_STARTUP_LEASE);
from_env_name!(LINE_EXCLUSION_REGEX);
from_env_name!(LINE_INCLUSION_REGEX);
from_env_name!(REDACT_REGEX);
from_env_name!(INGEST_TIMEOUT);
from_env_name!(INGEST_BUFFER_SIZE);
from_env_name!(RETRY_DIR);
from_env_name!(RETRY_DISK_LIMIT);
from_env_name!(CLEAR_CACHE_INTERVAL);
from_env_name!(METADATA_RETRY_DELAY);

enum Key {
    FromEnv(&'static str),
    Name(&'static str),
}

// Use a wrapper to only access via the enum
struct Map {
    inner: HashMap<String, String>,
}

impl<'a> Map {
    fn get(&'a self, key: &'static Key) -> Option<&'a String> {
        match key {
            // Remove prefix "MZ_" and lowercase
            Key::FromEnv(k) => self
                .inner
                .get(&k[crate::MEZMO_PREFIX.len()..].to_lowercase()),
            Key::Name(k) => self.inner.get(*k),
        }
    }

    fn get_string(&'a self, key: &'static Key) -> Option<String> {
        self.get(key).map(|x| x.to_string())
    }
}

pub fn read_file(file: &File) -> Result<Config, ConfigError> {
    debug!("loading config file as java properties");
    let mut list = Vec::new();
    match PropertiesIter::new(BufReader::new(file)).read_into(|k, v| list.push((k, v))) {
        Ok(_) => {
            let mut prop_map = HashMap::new();
            for item in list {
                // properties format is permissive, so we have to look for clues that its a
                // yaml file by looking at invalid keys.
                // "http", "log" and "journald" are parent yaml keys
                if !(item.0 != "-" && item.0 != "http" && item.0 != "log") {
                    return Err(ConfigError::PropertyInvalid("key is invalid".into()));
                }

                if prop_map.insert(item.0, item.1).is_some() {
                    return Err(ConfigError::PropertyInvalid("duplicated property".into()));
                }
            }
            from_property_map(prop_map)
        }
        Err(e) => Err(ConfigError::SerdeProperties(e)),
    }
}

fn from_property_map(map: HashMap<String, String>) -> Result<Config, ConfigError> {
    if map.is_empty() {
        return Err(ConfigError::MissingField("empty property map"));
    }
    let map = Map { inner: map };
    let mut result = Config::default();
    result.http.ingestion_key = map.get(&INGESTION_KEY).map(|s| s.to_string());

    match (map.get_string(&HOST), map.get_string(&IBM_HOST_DEPRECATED)) {
        (Some(_), Some(_)) => {
            return Err(ConfigError::PropertyInvalid(
                "entries for both host and LOGDNA_LOGHOST(deprecated) found".to_string(),
            ))
        }
        (Some(value), _) => result.http.host = Some(value),
        (_, Some(value)) => result.http.host = Some(value),
        _ => (),
    }

    if let Some(value) = map.get_string(&ENDPOINT) {
        result.http.endpoint = Some(value);
    }

    if let Some(value) = map.get_string(&USE_SSL) {
        result.http.use_ssl = Some(bool::from_str(&value).map_err(|e| {
            ConfigError::PropertyInvalid(format!("use ssl property is invalid: {}", e))
        })?);
    }

    if let Some(value) = map.get_string(&USE_COMPRESSION) {
        result.http.use_compression = Some(bool::from_str(&value).map_err(|e| {
            ConfigError::PropertyInvalid(format!("use compression property is invalid: {}", e))
        })?);
    }

    if let Some(value) = map.get_string(&GZIP_LEVEL) {
        result.http.gzip_level = Some(u32::from_str(&value).map_err(|e| {
            ConfigError::PropertyInvalid(format!("gzip level property is invalid: {}", e))
        })?);
    }

    if let Some(tags) = map.get(&TAGS).map(|s| Tags::from(argv::split_by_comma(s))) {
        result
            .http
            .params
            .get_or_insert_with(Params::builder)
            .tags(tags);
    };

    if let Some(value) = map.get_string(&OS_HOSTNAME) {
        result
            .http
            .params
            .get_or_insert_with(Params::builder)
            .hostname(value);
    }
    if let Some(ip) = map.get_string(&IP) {
        result
            .http
            .params
            .get_or_insert_with(Params::builder)
            .ip(ip);
    }
    if let Some(mac) = map.get_string(&MAC) {
        result
            .http
            .params
            .get_or_insert_with(Params::builder)
            .mac(mac);
    }

    if let Some(value) = map.get(&INGEST_TIMEOUT) {
        result.http.timeout = Some(value.parse().map_err(|e| {
            ConfigError::PropertyInvalid(format!("ingest_timeout is invalid: {}", e))
        })?);
    }

    if let Some(value) = map.get(&INGEST_BUFFER_SIZE) {
        result.http.body_size = Some(value.parse().map_err(|e| {
            ConfigError::PropertyInvalid(format!("ingest_buffer_size is invalid: {}", e))
        })?);
    }

    if let Some(value) = map.get(&RETRY_DIR) {
        result.http.retry_dir = Some(PathBuf::from(value));
    }

    if let Some(value) = map.get(&RETRY_DISK_LIMIT) {
        let limit = Bytes::<u64>::from_str(value).map_err(|e| {
            ConfigError::PropertyInvalid(format!("retry disk limit property is invalid: {}", e))
        })?;
        result.http.retry_disk_limit = Some(limit.size());
    }

    if let Some(log_dirs) = map.get(&LOG_DIRS) {
        // To support the legacy agent behaviour, we override the default (/var/log)
        // This results in a different behaviour depending on the format:
        //   yaml -> append to default
        //   conf -> override default when set
        result.log.dirs = argv::split_by_comma(log_dirs)
            .iter()
            .map(PathBuf::from)
            .collect();
    }

    if let Some(value) = map.get(&METRICS_PORT) {
        result.log.metrics_port = Some(u16::from_str(value).map_err(|e| {
            ConfigError::PropertyInvalid(format!("metrics port property is invalid: {}", e))
        })?);
    }

    if let Some(value) = map.get(&EXCLUSION_RULES) {
        let rules = result.log.exclude.get_or_insert(Rules::default());
        argv::split_by_comma(value)
            .iter()
            .for_each(|v| rules.glob.push(v.to_string()));
    }

    if let Some(value) = map.get(&EXCLUSION_REGEX_RULES) {
        let rules = result.log.exclude.get_or_insert(Rules::default());
        argv::split_by_comma(value)
            .iter()
            .for_each(|v| rules.regex.push(v.to_string()));
    }

    if let Some(value) = map.get(&INCLUSION_RULES) {
        let rules = result.log.include.get_or_insert(Rules::default());
        argv::split_by_comma(value)
            .iter()
            .for_each(|v| rules.glob.push(v.to_string()));
    }

    if let Some(value) = map.get(&INCLUSION_REGEX_RULES) {
        let rules = result.log.include.get_or_insert(Rules::default());
        argv::split_by_comma(value)
            .iter()
            .for_each(|v| rules.regex.push(v.to_string()));
    }

    if let Some(value) = map.get(&JOURNALD_PATHS) {
        let paths = result
            .journald
            .get_or_insert_with(JournaldConfig::default)
            .paths
            .get_or_insert(Vec::new());
        argv::split_by_comma(value)
            .iter()
            .for_each(|v| paths.push(PathBuf::from(v)));
    }

    if let Some(value) = map.get(&SYSTEMD_JOURNAL_TAILER) {
        if let Some(journald) = result.journald.as_mut() {
            journald.systemd_journal_tailer = Some(value.parse().unwrap_or(true));
        }
    }

    result.log.lookback = map.get_string(&LOOKBACK);
    result.log.use_k8s_enrichment = map.get_string(&USE_K8S_LOG_ENRICHMENT);
    result.log.log_k8s_events = map.get_string(&LOG_K8S_EVENTS);
    result.log.log_metric_server_stats = map.get_string(&LOG_METRIC_SERVER_STATS);
    result.log.db_path = map.get(&DB_PATH).map(PathBuf::from);

    if let Some(k8s_startup_lease) = map.get_string(&K8S_STARTUP_LEASE) {
        let startup = result
            .startup
            .get_or_insert_with(K8sStartupLeaseConfig::default);
        startup.option = Some(k8s_startup_lease)
    }

    if let Some(value) = map.get(&LINE_EXCLUSION_REGEX) {
        let regex_rules = result.log.line_exclusion_regex.get_or_insert(Vec::new());
        argv::split_by_comma(value)
            .iter()
            .for_each(|v| regex_rules.push(v.to_string()));
    }

    if let Some(value) = map.get(&LINE_INCLUSION_REGEX) {
        let regex_rules = result.log.line_inclusion_regex.get_or_insert(Vec::new());
        argv::split_by_comma(value)
            .iter()
            .for_each(|v| regex_rules.push(v.to_string()));
    }

    if let Some(value) = map.get(&K8S_METADATA_LINE_INCLUSION) {
        let k8s_rules = result.log.k8s_metadata_include.get_or_insert(Vec::new());
        argv::split_by_comma(value)
            .iter()
            .for_each(|v| k8s_rules.push(v.to_string()));
    }

    if let Some(value) = map.get(&K8S_METADATA_LINE_EXCLUSION) {
        let k8s_rules = result.log.k8s_metadata_exclude.get_or_insert(Vec::new());
        argv::split_by_comma(value)
            .iter()
            .for_each(|v| k8s_rules.push(v.to_string()));
    }

    if let Some(value) = map.get(&REDACT_REGEX) {
        let regex_rules = result.log.line_redact_regex.get_or_insert(Vec::new());
        argv::split_by_comma(value)
            .iter()
            .for_each(|v| regex_rules.push(v.to_string()));
    }

    if let Some(value) = map.get(&CLEAR_CACHE_INTERVAL) {
        result.log.clear_cache_interval = Some(u32::from_str(value).map_err(|e| {
            ConfigError::PropertyInvalid(format!("clear cache interval property is invalid: {}", e))
        })?);
    }

    if let Some(value) = map.get(&METADATA_RETRY_DELAY) {
        result.log.metadata_retry_delay = Some(u32::from_str(value).map_err(|e| {
            ConfigError::PropertyInvalid(format!("metadata retry delay property is invalid: {}", e))
        })?);
    }

    // Properties parser is very permissive
    // we need to validate that parsed was valid
    if result == Config::default() {
        return Err(ConfigError::PropertyInvalid("no property parsed".into()));
    }

    Ok(result)
}
