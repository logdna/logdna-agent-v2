use crate::error::ConfigError;
use crate::raw::{Config, Rules};
use crate::{argv, get_hostname};
use http::types::params::{Params, Tags};
use java_properties::PropertiesIter;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

pub fn read_file(file: &File) -> Result<Config, ConfigError> {
    let mut prop_map = HashMap::new();
    debug!("loading config file as java properties");
    match PropertiesIter::new(BufReader::new(file)).read_into(|k, v| {
        prop_map.insert(k, v);
    }) {
        Ok(_) => from_property_map(&prop_map),
        Err(e) => Err(ConfigError::SerdeProperties(e)),
    }
}

fn from_property_map(map: &HashMap<String, String>) -> Result<Config, ConfigError> {
    if map.is_empty() {
        return Err(ConfigError::MissingField("not implemented"));
    }
    let mut result = Config {
        http: Default::default(),
        log: Default::default(),
        journald: Default::default(),
    };
    result.http.ingestion_key = map.get("key").map(|s| s.to_string());
    let mut params = Params::builder()
        .hostname(get_hostname().unwrap_or_default())
        .build()
        .unwrap();
    params.tags = map.get("tags").map(|s| Tags::from(argv::split_by_comma(s)));

    if let Some(hostname) = map.get("hostname") {
        params.hostname = hostname.to_string();
    }

    result.http.params = Some(params);

    if let Some(log_dirs) = map.get("logdir") {
        // To support the legacy agent behaviour, we override the default (/var/log)
        // This results in a different behaviour depending on the format:
        //   yaml -> append to default
        //   conf -> override default when set
        result.log.dirs = argv::split_by_comma(log_dirs)
            .iter()
            .map(PathBuf::from)
            .collect();
    }

    if let Some(exclude) = map.get("exclude") {
        let rules = result.log.exclude.get_or_insert(Rules::default());
        argv::split_by_comma(exclude)
            .iter()
            .for_each(|v| rules.glob.push(v.to_string()));
    }

    if let Some(exclude_regex) = map.get("exclude_regex") {
        let rules = result.log.exclude.get_or_insert(Rules::default());
        argv::split_by_comma(exclude_regex)
            .iter()
            .for_each(|v| rules.regex.push(v.to_string()));
    }

    Ok(result)
}
