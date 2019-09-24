use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::str::FromStr;

use serde::Deserialize;

use config_macro::env_config;

#[env_config]
#[derive(Deserialize, Debug)]
pub struct Config {
    #[env(LOGDNA_CONFIG_FILE, DEFAULT_CONF_FILE)]
    #[example("/etc/logdna/config.yaml")]
    #[default("/etc/logdna/config.yaml")]
    pub config_file: PathBuf,

    #[env(LOGDNA_HOST, LDLOGHOST)]
    #[example("logs.logdna.com")]
    pub host: Option<String>,

    #[env(LOGDNA_ENDPOINT, LDLOGPATH)]
    #[example("/logs/agent")]
    pub endpoint: Option<String>,

    #[env(LOGDNA_INGESTION_KEY, LOGDNA_AGENT_KEY)]
    #[example("sdf79s6df3j4n3sdfs435")]
    pub ingestion_key: Option<String>,

    #[env(LOGDNA_USE_SSL, LDLOGSSL)]
    #[example("false")]
    pub use_ssl: Option<bool>,

    #[env(LOGDNA_USE_COMPRESSION, COMPRESS)]
    #[example("true")]
    pub use_compression: Option<bool>,

    #[env(LOGDNA_GZIP_LEVEL, GZIP_COMPRESS_LEVEL)]
    #[example("2")]
    pub gzip_level: Option<u32>,

    #[env(LOGDNA_HOSTNAME)]
    #[example("my-server")]
    pub hostname: Option<String>,

    #[env(LOGDNA_IP)]
    #[example("127.0.0.1")]
    pub ip: Option<String>,

    #[env(LOGDNA_TAGS)]
    #[example("some,tags,and,stuff")]
    pub tags: Option<EnvList<String>>,

    #[env(LOGDNA_MAC)]
    #[example("00:0a:95:9d:68:16")]
    pub mac: Option<String>,

    #[env(LOGDNA_LOG_DIRS, LOG_DIRS)]
    #[example("/var/log/,/var/data/,/test/logs/")]
    pub log_dirs: Option<EnvList<PathBuf>>,

    #[env(LOGDNA_EXCLUSION_RULES, LOGDNA_EXCLUDE)]
    #[example("/var/log/**,/var/data/**")]
    pub exclusion_rules: Option<EnvList<String>>,

    #[env(LOGDNA_EXCLUSION_REGEX_RULES, LOGDNA_EXCLUDE_REGEX)]
    #[example("/var/log/.*,/var/data/.*")]
    pub exclusion_regex_rules: Option<EnvList<String>>,

    #[env(LOGDNA_INCLUSION_RULES, LOGDNA_INCLUDE)]
    #[example("/var/log/**,/var/data/**")]
    pub inclusion_rules: Option<EnvList<String>>,

    #[env(LOGDNA_INCLUSION_REGEX_RULES, LOGDNA_INCLUDE_REGEX)]
    #[example("/var/log/.*,/var/data/.*")]
    pub inclusion_regex_rules: Option<EnvList<String>>,
}

#[derive(Deserialize, Debug, Ord, PartialOrd, Eq, PartialEq)]
pub struct EnvList<T: FromStr>(pub Vec<T>);

impl<T: FromStr> Deref for EnvList<T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: FromStr> DerefMut for EnvList<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T: FromStr> FromStr for EnvList<T> {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(
            EnvList(
                s.split_terminator(",").filter_map(|s| T::from_str(s).ok()).collect()
            )
        )
    }
}

impl<T: FromStr> From<Vec<T>> for EnvList<T> {
    fn from(vec: Vec<T>) -> Self {
        EnvList(vec)
    }
}