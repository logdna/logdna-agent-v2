use crate::env_vars;
use crate::error::ConfigError;
use crate::raw::{Config, Rules};
use crate::{argv, get_hostname};
use http::types::params::{Params, Tags};
use humanize_rs::bytes::Bytes;
use java_properties::PropertiesIter;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::str::FromStr;

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

// New keys: reuse the same name from env vars, removing the LOGDNA_ prefix and
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
from_env_name!(LOOKBACK);
from_env_name!(DB_PATH);
from_env_name!(METRICS_PORT);
from_env_name!(USE_K8S_LOG_ENRICHMENT);
from_env_name!(LOG_K8S_EVENTS);
from_env_name!(K8S_METADATA_INCLUSION);
from_env_name!(K8S_METADATA_EXCLUSION);
from_env_name!(K8S_STARTUP_LEASE);
from_env_name!(LINE_EXCLUSION);
from_env_name!(LINE_INCLUSION);
from_env_name!(REDACT);
from_env_name!(INGEST_TIMEOUT);
from_env_name!(INGEST_BUFFER_SIZE);
from_env_name!(RETRY_DIR);
from_env_name!(RETRY_DISK_LIMIT);

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
            // Remove prefix "LOGDNA_" and lowercase
            Key::FromEnv(k) => self.inner.get(&k[7..].to_lowercase()),
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
    let mut result = Config {
        http: Default::default(),
        log: Default::default(),
        journald: Default::default(),
        startup: Default::default(),
    };
    result.http.ingestion_key = map.get(&INGESTION_KEY).map(|s| s.to_string());

    match (map.get_string(&HOST), map.get_string(&IBM_HOST_DEPRECATED)) {
        (Some(_), Some(_)) => {
            return Err(ConfigError::PropertyInvalid(
                "entries for both host and LOGDNA_LOGHOST found".to_string(),
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

    let mut params = Params::builder()
        .hostname(get_hostname().unwrap_or_default())
        .build()
        .unwrap();
    params.tags = map.get(&TAGS).map(|s| Tags::from(argv::split_by_comma(s)));

    if let Some(value) = map.get_string(&OS_HOSTNAME) {
        params.hostname = value;
    }
    params.ip = map.get_string(&IP);
    params.mac = map.get_string(&MAC);

    result.http.params = Some(params);

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
        let paths = result.journald.paths.get_or_insert(Vec::new());
        argv::split_by_comma(value)
            .iter()
            .for_each(|v| paths.push(PathBuf::from(v)));
    }

    result.log.lookback = map.get_string(&LOOKBACK);
    result.log.use_k8s_enrichment = map.get_string(&USE_K8S_LOG_ENRICHMENT);
    result.log.log_k8s_events = map.get_string(&LOG_K8S_EVENTS);
    result.log.db_path = map.get(&DB_PATH).map(PathBuf::from);
    result.startup.option = map.get_string(&K8S_STARTUP_LEASE);

    if let Some(value) = map.get(&LINE_EXCLUSION) {
        let regex_rules = result.log.line_exclusion_regex.get_or_insert(Vec::new());
        argv::split_by_comma(value)
            .iter()
            .for_each(|v| regex_rules.push(v.to_string()));
    }

    if let Some(value) = map.get(&LINE_INCLUSION) {
        let regex_rules = result.log.line_inclusion_regex.get_or_insert(Vec::new());
        argv::split_by_comma(value)
            .iter()
            .for_each(|v| regex_rules.push(v.to_string()));
    }

    if let Some(value) = map.get(&K8S_METADATA_INCLUSION) {
        let k8s_rules = result.log.k8s_metadata_include.get_or_insert(Vec::new());
        argv::split_by_comma(value)
            .iter()
            .for_each(|v| k8s_rules.push(v.to_string()));
    }

    if let Some(value) = map.get(&K8S_METADATA_EXCLUSION) {
        let k8s_rules = result.log.k8s_metadata_exclude.get_or_insert(Vec::new());
        argv::split_by_comma(value)
            .iter()
            .for_each(|v| k8s_rules.push(v.to_string()));
    }

    if let Some(value) = map.get(&REDACT) {
        let regex_rules = result.log.line_redact_regex.get_or_insert(Vec::new());
        argv::split_by_comma(value)
            .iter()
            .for_each(|v| regex_rules.push(v.to_string()));
    }

    // Properties parser is very permissive
    // we need to validate that parsed was valid
    if result == Config::default() {
        return Err(ConfigError::PropertyInvalid("no property parsed".into()));
    }

    Ok(result)
}
