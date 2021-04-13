use crate::raw::{Config as RawConfig, Rules as RawRules};
use config_macro::env_config;
use http::types::params::{Params, Tags};
use serde::Deserialize;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::str::FromStr;

#[env_config]
#[derive(Deserialize, Debug, Clone)]
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

    #[env(LOGDNA_LINE_EXCLUSION_REGEX)]
    #[example("DEBUG")]
    pub line_exclusion_regex: Option<EnvList<String>>,

    #[env(LOGDNA_LINE_INCLUSION_REGEX)]
    pub line_inclusion_regex: Option<EnvList<String>>,

    #[env(LOGDNA_REDACT_REGEX)]
    #[example(r"\S+@\S+\.\S+")]
    pub line_redact_regex: Option<EnvList<String>>,

    #[env(LOGDNA_JOURNALD_PATHS)]
    #[example("/var/log/journal")]
    pub journald_paths: Option<EnvList<PathBuf>>,

    #[env(LOGDNA_LOOKBACK)]
    #[example("none")]
    pub lookback: Option<String>,

    #[env(LOGDNA_USE_K8S_LOG_ENRICHMENT)]
    #[example("always")]
    pub use_k8s_enrichment: Option<String>,

    #[env(LOGDNA_LOG_K8S_EVENTS)]
    #[example("always")]
    pub log_k8s_events: Option<String>,

    #[env(LOGDNA_DB_PATH)]
    #[example("/var/lib/logdna-agent/")]
    pub db_path: Option<String>,
}

impl Config {
    pub fn merge(self, mut raw: RawConfig) -> RawConfig {
        if self.host.is_some() {
            raw.http.host = self.host;
        }

        if self.endpoint.is_some() {
            raw.http.endpoint = self.endpoint;
        }

        if self.ingestion_key.is_some() {
            raw.http.ingestion_key = self.ingestion_key;
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

        let mut params = match raw.http.params {
            Some(v) => v,
            None => Params {
                hostname: "".to_string(),
                mac: None,
                ip: None,
                now: 0,
                tags: None,
            },
        };

        if let Some(v) = self.hostname {
            params.hostname = v;
        }

        if self.ip.is_some() {
            params.ip = self.ip;
        }

        if self.mac.is_some() {
            params.mac = self.mac;
        }

        if let Some(v) = self.tags {
            match params.tags {
                Some(ref mut tags) => v.iter().for_each(|t| {
                    tags.add(t);
                }),
                None => params.tags = Some(Tags::from(v.0)),
            }
        }

        raw.http.params = Some(params);

        if let Some(mut v) = self.log_dirs {
            raw.log.dirs.append(&mut v)
        }

        if let Some(p) = self.db_path {
            raw.log.db_path = Some(p.into())
        }

        if let Some(mut v) = self.exclusion_rules {
            match raw.log.exclude {
                Some(ref mut rules) => rules.glob.append(&mut v),
                None => {
                    let mut rules = RawRules {
                        glob: Vec::new(),
                        regex: Vec::new(),
                    };
                    rules.glob.append(&mut v);
                    raw.log.exclude = Some(rules);
                }
            }
        }

        if let Some(mut v) = self.exclusion_regex_rules {
            match raw.log.exclude {
                Some(ref mut rules) => rules.regex.append(&mut v),
                None => {
                    let mut rules = RawRules {
                        glob: Vec::new(),
                        regex: Vec::new(),
                    };
                    rules.regex.append(&mut v);
                    raw.log.exclude = Some(rules);
                }
            }
        }

        if let Some(mut v) = self.inclusion_rules {
            match raw.log.include {
                Some(ref mut rules) => rules.glob.append(&mut v),
                None => {
                    let mut rules = RawRules {
                        glob: Vec::new(),
                        regex: Vec::new(),
                    };
                    rules.glob.append(&mut v);
                    raw.log.include = Some(rules);
                }
            }
        }

        if let Some(mut v) = self.inclusion_regex_rules {
            match raw.log.include {
                Some(ref mut rules) => rules.regex.append(&mut v),
                None => {
                    let mut rules = RawRules {
                        glob: Vec::new(),
                        regex: Vec::new(),
                    };
                    rules.regex.append(&mut v);
                    raw.log.include = Some(rules);
                }
            }
        }

        if let Some(mut v) = self.journald_paths {
            let paths = raw.journald.paths.get_or_insert(Vec::new());
            paths.append(&mut v);
        }

        if self.use_k8s_enrichment.is_some() {
            raw.log.use_k8s_enrichment = self.use_k8s_enrichment;
        }

        if self.log_k8s_events.is_some() {
            raw.log.log_k8s_events = self.log_k8s_events;
        }

        if self.lookback.is_some() {
            raw.log.lookback = self.lookback;
        }

        if let Some(list) = self.line_exclusion_regex {
            raw.log.line_exclusion_regex = Some(list.deref().clone());
        }

        if let Some(list) = self.line_inclusion_regex {
            raw.log.line_inclusion_regex = Some(list.deref().clone());
        }

        if let Some(list) = self.line_redact_regex {
            raw.log.line_redact_regex = Some(list.deref().clone());
        }

        raw
    }
}

#[derive(Deserialize, Debug, Ord, PartialOrd, Eq, PartialEq, Clone)]
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
        Ok(EnvList(
            s.split_terminator(',')
                .filter_map(|s| T::from_str(s).ok())
                .collect(),
        ))
    }
}

impl<T: FromStr> From<Vec<T>> for EnvList<T> {
    fn from(vec: Vec<T>) -> Self {
        EnvList(vec)
    }
}
