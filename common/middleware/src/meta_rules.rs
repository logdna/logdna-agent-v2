use crate::{Middleware, Status};
use http::types::body::{KeyValueMap, LineBufferMut};
use lazy_static::lazy_static;
use log::error;
use regex::bytes::Regex as RegexB;
use regex::Regex;
use std::collections::HashMap;
use std::ops::Deref;

/// Env config options
static LOGDNA_META_APP: &str = "LOGDNA_META_APP";
static LOGDNA_META_HOST: &str = "LOGDNA_META_HOST";
static LOGDNA_META_ENV: &str = "LOGDNA_META_ENV";
static LOGDNA_META_FILE: &str = "LOGDNA_META_FILE";
static LOGDNA_META_K8S_FILE: &str = "LOGDNA_META_K8S_FILE";
static LOGDNA_META_JSON: &str = "LOGDNA_META_JSON";
static LOGDNA_META_ANNOTATIONS: &str = "LOGDNA_META_ANNOTATIONS";
static LOGDNA_META_LABELS: &str = "LOGDNA_META_LABELS";

static K8S_LOG_DIR: &str = "/var/log/containers/";

lazy_static! {
    static ref REGEX_VAR: Regex = Regex::new(r"(?P<var>\$\{(?P<key>[^|}]+?)})").unwrap();
    static ref REGEX_VAR_DEFAULT: Regex =
        Regex::new(r"(?P<var>\$\{(?P<key>[^|}]+?)\|(?P<default>[^|}]*?)})").unwrap();
    static ref REGEX_CRIO_LOG: RegexB =
        RegexB::new(r"([0-9]{4}(?:-[0-9]{2}){2}T(?:[0-9]{2}:){2}[0-9]{2}.[0-9]{1,9}(?:[+-][0-9]{2}:[0-9]{2}|Z)) (stdout|stderr) ([PF]) (?P<line>.*)").unwrap();
}

//TODO: extract to LogConfig
#[derive(Default)]
pub struct MetaRulesConfig {
    pub app: Option<String>,
    pub host: Option<String>,
    pub env: Option<String>,
    pub file: Option<String>,
    pub k8s_file: Option<String>, // for k8s lines, applied after "file"
    pub meta: Option<String>,
    pub annotations: Option<String>,
    pub labels: Option<String>,
}

impl MetaRulesConfig {
    pub fn from_env() -> Self {
        let vars = os_env_hashmap();
        MetaRulesConfig {
            app: vars.get(LOGDNA_META_APP).cloned(),
            host: vars.get(LOGDNA_META_HOST).cloned(),
            env: vars.get(LOGDNA_META_ENV).cloned(),
            file: vars.get(LOGDNA_META_FILE).cloned(),
            k8s_file: vars.get(LOGDNA_META_K8S_FILE).cloned(),
            meta: vars.get(LOGDNA_META_JSON).cloned(),
            annotations: vars.get(LOGDNA_META_ANNOTATIONS).cloned(),
            labels: vars.get(LOGDNA_META_LABELS).cloned(),
        }
    }
}

pub struct MetaRules {
    env_map: HashMap<String, String>,
    // "override" fields
    over_app: Option<String>,
    over_host: Option<String>,
    over_env: Option<String>,
    over_file: Option<String>,
    over_k8s_file: Option<String>, // k8s lines only
    over_meta: Option<String>,
    over_annotations: Option<HashMap<String, String>>, // "merge override" (for "delete and then override" use disable k8s enrichment)
    over_labels: Option<HashMap<String, String>>,      // --
}

#[derive(Debug, thiserror::Error)]
pub enum MetaRulesError {
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
}

impl MetaRules {
    pub fn new(cfg: MetaRulesConfig) -> Result<MetaRules, MetaRulesError> {
        let obj = MetaRules {
            env_map: os_env_hashmap(),
            over_app: cfg.app,
            over_host: cfg.host,
            over_env: cfg.env,
            over_file: cfg.file,
            over_k8s_file: cfg.k8s_file,
            over_meta: cfg.meta,
            over_annotations: cfg.annotations.map_or_else(
                || None,
                |str| match serde_json::from_str(str.as_str()) {
                    Ok(kvp) => kvp,
                    Err(err) => {
                        panic!(
                            "Invalid LOGDNA_META_ANNOTATIONS value: '{}', err: {}",
                            str, err
                        )
                    }
                },
            ),
            over_labels: cfg.labels.map_or_else(
                || None,
                |str| match serde_json::from_str(str.as_str()) {
                    Ok(kvp) => kvp,
                    Err(err) => {
                        panic!("Invalid LOGDNA_META_LABELS value: '{}', err: {}", str, err)
                    }
                },
            ),
        };
        Ok(obj)
    }

    pub fn is_active(&self) -> bool {
        self.over_app.is_some()
            || self.over_host.is_some()
            || self.over_env.is_some()
            || self.over_file.is_some()
            || self.over_k8s_file.is_some()
            || self.over_meta.is_some()
            || self.over_annotations.is_some()
            || self.over_labels.is_some()
    }

    /// process_line
    /// - override line meta fields
    /// [ create map ] => [ substitute then insert to map ] => [ substitute from map ] => [ override ]
    ///  os env vars       "override" labels                    "override" fields          line fields
    ///  line fields       "override" annotations
    ///  line labels
    ///  line annotations
    /// Excluded:
    ///  line "meta"
    ///
    fn process_line<'a>(
        &self,
        line: &'a mut dyn LineBufferMut,
    ) -> Status<&'a mut dyn LineBufferMut> {
        if !self.is_active() {
            return Status::Ok(line);
        }
        //
        // [ create map ]
        //
        let mut meta_map: HashMap<String, String> = self.env_map.clone();
        if let Some(annotations) = line.get_annotations() {
            for (k, v) in annotations.iter() {
                meta_map.insert(k.clone(), v.clone());
            }
        }
        if let Some(labels) = line.get_labels() {
            for (k, v) in labels.iter() {
                meta_map.insert(k.clone(), v.clone());
            }
        }
        line.get_app()
            .map(|v| meta_map.insert("line.app".into(), v.into()));
        line.get_host()
            .map(|v| meta_map.insert("line.host".into(), v.into()));
        line.get_env()
            .map(|v| meta_map.insert("line.env".into(), v.into()));
        line.get_file()
            .map(|v| meta_map.insert("line.file".into(), v.into()));
        // k8s lines have non empty annotations/labels
        let is_k8s_line = line.get_annotations().is_some()
            || line.get_labels().is_some()
            || line.get_file().unwrap_or("").starts_with(K8S_LOG_DIR);
        //
        // [ substitute then insert to map ]
        // substitute "override" labels & annotations
        // merge "with override" + remove empty values
        //
        if let (Some(over_annotations), true) = (self.over_annotations.clone(), is_k8s_line) {
            let mut new_annotations = KeyValueMap::new();
            line.get_annotations().map(|kvm| {
                for (k, v) in kvm.iter() {
                    new_annotations.insert(k.clone(), v.clone());
                }
                Some(kvm)
            });
            for (k, v) in over_annotations.iter() {
                let v = substitute(v, &meta_map);
                meta_map.insert(k.clone(), v.clone()); // insert "with override"
                if v.is_empty() {
                    new_annotations = new_annotations.remove(&k.clone());
                } else {
                    new_annotations.insert(k.clone(), v.clone());
                }
            }
            if line.set_annotations(new_annotations).is_err() {}
        }
        if let (Some(over_labels), true) = (self.over_labels.clone(), is_k8s_line) {
            let mut new_labels = KeyValueMap::new();
            line.get_labels().map(|kvm| {
                for (k, v) in kvm.iter() {
                    new_labels.insert(k.clone(), v.clone());
                }
                Some(kvm)
            });
            for (k, v) in over_labels.iter() {
                let v = substitute(v, &meta_map);
                meta_map.insert(k.clone(), v.clone());
                if v.is_empty() {
                    new_labels = new_labels.remove(&k.clone());
                } else {
                    new_labels.insert(k.clone(), v.clone());
                }
            }
            if line.set_labels(new_labels).is_err() {}
        }
        // [ substitute from map ]
        // substitute "override" fields and then override line fields
        // TODO: add rate limited err log for setters
        //
        if let Some(over_app) = self.over_app.clone() {
            let app = substitute(over_app.deref(), &meta_map);
            if line.set_app(app).is_err() {}
        }
        if let Some(over_host) = self.over_host.clone() {
            let host = substitute(over_host.deref(), &meta_map);
            if line.set_host(host).is_err() {}
        }
        if let Some(over_env) = self.over_env.clone() {
            let env = substitute(over_env.deref(), &meta_map);
            if line.set_env(env).is_err() {}
        }
        if let Some(over_file) = self.over_file.clone() {
            let file = substitute(over_file.deref(), &meta_map);
            if line.set_file(file).is_err() {}
        }
        if let (Some(over_k8s_file), true) = (self.over_k8s_file.clone(), is_k8s_line) {
            let file = substitute(over_k8s_file.deref(), &meta_map);
            if line.set_file(file).is_err() {}
            // overriding "file" will disable server side CRIO log line parsing,
            // so we remove CRIO log prefix from line here to make regular line parser happy
            if let Some(line_text) = line.get_line_buffer() {
                let mut new_buf = Vec::with_capacity(line_text.len());
                let mut is_found = false;
                for cap in REGEX_CRIO_LOG.captures_iter(line_text).take(1) {
                    cap.name("line").map(|new_line| {
                        new_buf = Vec::from(new_line.as_bytes());
                        is_found = true;
                        Some(new_line)
                    });
                }
                if is_found && line.set_line_buffer(new_buf).is_err() {}
            }
        }
        if let Some(over_meta) = self.over_meta.clone() {
            let meta = substitute(over_meta.deref(), &meta_map);
            match serde_json::from_str(&meta) {
                Ok(val) => if line.set_meta(val).is_err() {},
                Err(err) => panic!("Invalid LOGDNA_META_JSON value: '{}', err: {}", meta, err),
            }
        }
        Status::Ok(line)
    }
}

impl Middleware for MetaRules {
    fn run(&self) {}
    fn process<'a>(&self, line: &'a mut dyn LineBufferMut) -> Status<&'a mut dyn LineBufferMut> {
        self.process_line(line)
    }
}

fn os_env_hashmap() -> HashMap<String, String> {
    let mut map = HashMap::new();
    use std::env;
    for (key, val) in env::vars_os() {
        // Use pattern bindings instead of testing .is_some() followed by .unwrap()
        if let (Ok(k), Ok(v)) = (key.into_string(), val.into_string()) {
            map.insert(k, v);
        }
    }
    map
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
                    output = output.replace(&var.as_str(), v);
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
                        output = output.replace(&var.as_str(), v);
                        Some(var)
                    });
                }
                None => {
                    cap.name("default").map(|default| {
                        cap.name("var").map(|var| {
                            output = output.replace(&var.as_str(), default.as_str());
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

//################################################################################## Tests

#[cfg(test)]
mod tests {
    use crate::meta_rules::{os_env_hashmap, substitute};

    #[test]
    fn test_meta_config() {
        env::set_var(LOGDNA_META_APP, "some_app");
        env::set_var(LOGDNA_META_HOST, "some_host");
        env::set_var(LOGDNA_META_ENV, "some_env");
        env::set_var(LOGDNA_META_FILE, "some_file");
        env::set_var(LOGDNA_META_JSON, "some_json");
        env::set_var(LOGDNA_META_ANNOTATIONS, "some_annotations");
        env::set_var(LOGDNA_META_LABELS, "some_labels");
        let cfg = MetaRulesConfig::from_env();
        assert_eq!(cfg.app, Some("some_app".into()));
        assert_eq!(cfg.host, Some("some_host".into()));
        assert_eq!(cfg.env, Some("some_env".into()));
        assert_eq!(cfg.file, Some("some_file".into()));
        assert_eq!(cfg.meta, Some("some_json".into()));
        assert_eq!(cfg.annotations, Some("some_annotations".into()));
        assert_eq!(cfg.labels, Some("some_labels".into()));
    }

    #[test]
    fn test_os_env_hashmap() {
        let vars = os_env_hashmap();
        let host_name = vars.get("PATH");
        assert_ne!(host_name, None);
        for (key, value) in vars.into_iter() {
            println!("{} = {:?}", key, value);
        }
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

    use super::*;
    use http::types::body::LineBuilder;
    use serde_json::Value;
    use std::env;

    #[test]
    /// k8s case: annotations and/or labels are defined
    fn test_override_existing_k8s() {
        let cfg = MetaRulesConfig {
            app: Some("REDACTED_APP".into()),
            host: Some("REDACTED_HOST".into()),
            env: Some("REDACTED_ENV".into()),
            file: Some("REDACTED_FILE".into()),
            k8s_file: Some("REDACTED_K8S_FILE".into()),
            meta: Some(r#"{"key1":"val1"}"#.into()),
            annotations: Some(r#"{"key1":"val1"}"#.into()),
            labels: Some(r#"{"key1":"val1"}"#.into()),
        };
        let p = MetaRules::new(cfg).unwrap();
        let redacted_meta: Value = serde_json::from_str(r#"{"key1":"val1"}"#).unwrap();
        let redacted_annotations = KeyValueMap::new().add("key1", "val1");
        let redacted_labels = KeyValueMap::new().add("key1", "val1");
        let some_meta: Value = serde_json::from_str(r#"{"some_key1":"some_val1"}"#).unwrap();
        let some_annotations = KeyValueMap::new().add("key1", "some_val1");
        let some_labels = KeyValueMap::new().add("key1", "some_val1");
        let mut line = LineBuilder::new()
            .line("SOME_LINE")
            .app("SOME_APP")
            .file("SOME_FILE")
            .host("SOME_HOST")
            .level("SOME_LEVEL")
            .meta(some_meta)
            .annotations(some_annotations)
            .labels(some_labels);
        let status = p.process(&mut line);
        assert!(matches!(status, Status::Ok(_)));
        assert_eq!(line.app.unwrap(), "REDACTED_APP");
        assert_eq!(line.host.unwrap(), "REDACTED_HOST");
        assert_eq!(line.env.unwrap(), "REDACTED_ENV");
        assert_eq!(line.file.unwrap(), "REDACTED_K8S_FILE");
        assert_eq!(line.meta.unwrap(), redacted_meta); // replaced as a whole
        assert_eq!(line.annotations.unwrap(), redacted_annotations);
        assert_eq!(line.labels.unwrap(), redacted_labels);
    }

    #[test]
    /// k8s case: annotations and/or labels are defined
    fn test_override_non_existing_k8s() {
        let cfg = MetaRulesConfig {
            app: Some("REDACTED_APP".into()),
            host: Some("REDACTED_HOST".into()),
            env: Some("REDACTED_ENV".into()),
            file: Some("REDACTED_FILE".into()),
            k8s_file: Some("REDACTED_K8S_FILE".into()),
            meta: Some(r#"{"key1":"val1"}"#.into()),
            annotations: Some(r#"{"key1":"val1"}"#.into()),
            labels: Some(r#"{"key1":"val1"}"#.into()),
        };
        let p = MetaRules::new(cfg).unwrap();
        let redacted_meta: Value = serde_json::from_str(r#"{"key1":"val1"}"#).unwrap();
        let redacted_annotations = KeyValueMap::new().add("key1", "val1");
        let redacted_labels = KeyValueMap::new().add("key1", "val1");
        let mut line = LineBuilder::new()
            .annotations(KeyValueMap::new().add("key1", "some_val1"))
            .labels(KeyValueMap::new().add("key1", "some_val1"));
        let status = p.process(&mut line);
        assert!(matches!(status, Status::Ok(_)));
        assert_eq!(line.app.unwrap(), "REDACTED_APP");
        assert_eq!(line.host.unwrap(), "REDACTED_HOST");
        assert_eq!(line.env.unwrap(), "REDACTED_ENV");
        assert_eq!(line.file.unwrap(), "REDACTED_K8S_FILE"); // k8s overrides last
        assert_eq!(line.meta.unwrap(), redacted_meta);
        assert_eq!(line.annotations.unwrap(), redacted_annotations);
        assert_eq!(line.labels.unwrap(), redacted_labels);
    }

    #[test]
    /// non k8s case: annotations and/or labels are NOT defined
    fn test_override_non_existing() {
        let cfg = MetaRulesConfig {
            app: Some("REDACTED_APP".into()),
            host: Some("REDACTED_HOST".into()),
            env: Some("REDACTED_ENV".into()),
            file: Some("REDACTED_FILE".into()),
            k8s_file: Some("REDACTED_K8S_FILE".into()),
            meta: Some(r#"{"key1":"val1"}"#.into()),
            annotations: Some(r#"{"key1":"val1"}"#.into()),
            labels: Some(r#"{"key1":"val1"}"#.into()),
        };
        let p = MetaRules::new(cfg).unwrap();
        let redacted_meta: Value = serde_json::from_str(r#"{"key1":"val1"}"#).unwrap();
        let mut line = LineBuilder::new();
        let status = p.process(&mut line);
        assert!(matches!(status, Status::Ok(_)));
        assert_eq!(line.app.unwrap(), "REDACTED_APP");
        assert_eq!(line.host.unwrap(), "REDACTED_HOST");
        assert_eq!(line.env.unwrap(), "REDACTED_ENV");
        assert_eq!(line.file.unwrap(), "REDACTED_FILE");
        assert_eq!(line.meta.unwrap(), redacted_meta);
        assert_eq!(line.annotations, None);
        assert_eq!(line.labels, None);
    }

    #[test]
    fn transparent_if_not_configured_k8s() {
        let p = MetaRules::new(MetaRulesConfig::default()).unwrap();
        let some_annotations: KeyValueMap = serde_json::from_str(r#"{"key1":"val1"}"#).unwrap();
        let some_labels: KeyValueMap = serde_json::from_str(r#"{"key1":"val1"}"#).unwrap();
        let mut line = LineBuilder::new()
            .line("SOME_LINE")
            .app("SOME_APP")
            .file("SOME_FILE")
            .host("SOME_HOST")
            .level("SOME_LEVEL")
            .annotations(some_annotations.clone())
            .labels(some_labels.clone());
        let status = p.process(&mut line);
        assert!(matches!(status, Status::Ok(_)));
        assert_eq!(line.line.unwrap(), "SOME_LINE");
        assert_eq!(line.app.unwrap(), "SOME_APP");
        assert_eq!(line.file.unwrap(), "SOME_FILE");
        assert_eq!(line.host.unwrap(), "SOME_HOST");
        assert_eq!(line.level.unwrap(), "SOME_LEVEL");
        assert_eq!(line.annotations.unwrap(), some_annotations);
        assert_eq!(line.labels.unwrap(), some_labels);
    }

    #[test]
    fn test_no_changes_if_not_configured() {
        let p = MetaRules::new(MetaRulesConfig::default()).unwrap();
        let mut line = LineBuilder::new()
            .line("SOME_LINE")
            .app("SOME_APP")
            .file("SOME_FILE")
            .host("SOME_HOST")
            .level("SOME_LEVEL");
        // no annotations & labels
        let status = p.process(&mut line);
        assert!(matches!(status, Status::Ok(_)));
        assert_eq!(line.line.unwrap(), "SOME_LINE");
        assert_eq!(line.app.unwrap(), "SOME_APP");
        assert_eq!(line.file.unwrap(), "SOME_FILE");
        assert_eq!(line.host.unwrap(), "SOME_HOST");
        assert_eq!(line.level.unwrap(), "SOME_LEVEL");
        assert_eq!(line.annotations, None);
        assert_eq!(line.labels, None);
    }

    #[test]
    /// delete value in annotations and labels
    fn test_delete_value_in_annotations_labels() {
        let some_annotations: KeyValueMap =
            serde_json::from_str(r#"{"key1":"val1", "key2":"val2"}"#).unwrap();
        let some_labels: KeyValueMap =
            serde_json::from_str(r#"{"key1":"val1", "key2":"val2"}"#).unwrap();
        let mut line = LineBuilder::new()
            .annotations(some_annotations)
            .labels(some_labels);
        let cfg = MetaRulesConfig {
            app: None,
            host: None,
            env: None,
            file: None,
            k8s_file: None,
            meta: None,
            annotations: Some(r#"{"key1":"val1", "key2":""}"#.into()), // empty key2 >> delete
            labels: Some(r#"{"key1":"val1", "key2":""}"#.into()),      // empty key2 >> delete
        };
        let p = MetaRules::new(cfg).unwrap();
        let redacted_annotations = KeyValueMap::new().add("key1", "val1");
        let redacted_labels = KeyValueMap::new().add("key1", "val1");
        let status = p.process(&mut line);
        assert!(matches!(status, Status::Ok(_)));
        assert_eq!(line.annotations, redacted_annotations.into());
        assert_eq!(line.labels, redacted_labels.into());
    }

    #[test]
    /// make meta empty
    fn test_delete_meta() {
        let some_meta: Value = serde_json::from_str(r#"{"some_key1":"some_val1"}"#).unwrap();
        let mut line = LineBuilder::new().meta(some_meta);
        let cfg = MetaRulesConfig {
            app: None,
            host: None,
            env: None,
            file: None,
            k8s_file: None,
            meta: Some("{}".into()),
            annotations: None,
            labels: None,
        };
        let p = MetaRules::new(cfg).unwrap();
        let redacted_meta: Value = serde_json::from_str("{}").unwrap();
        let status = p.process(&mut line);
        assert!(matches!(status, Status::Ok(_)));
        assert_eq!(line.meta.unwrap(), redacted_meta);
    }

    #[test]
    /// override APP with a values from labels
    fn test_override_app_with_value_from_labels_k8s() {
        let some_labels: KeyValueMap =
            serde_json::from_str(r#"{"key1":"val1", "key2":"val2"}"#).unwrap();
        let mut line = LineBuilder::new().labels(some_labels);
        let cfg = MetaRulesConfig {
            app: Some("app_${key1}_${key2}".into()),
            host: None,
            env: None,
            file: None,
            k8s_file: None,
            meta: None,
            annotations: None,
            labels: None,
        };
        let p = MetaRules::new(cfg).unwrap();
        let status = p.process(&mut line);
        assert!(matches!(status, Status::Ok(_)));
        assert_eq!(line.app.unwrap(), "app_val1_val2");
    }

    #[test]
    /// override APP with default including empty (delete var)
    fn test_override_app_with_default() {
        let mut line = LineBuilder::new().app("some_app");
        let cfg = MetaRulesConfig {
            app: Some("app_${key1|default1}_${key2|}".into()),
            host: None,
            env: None,
            file: None,
            k8s_file: None,
            meta: None,
            annotations: None,
            labels: None,
        };
        let p = MetaRules::new(cfg).unwrap();
        let status = p.process(&mut line);
        assert!(matches!(status, Status::Ok(_)));
        assert_eq!(line.app.unwrap(), "app_default1_");
    }

    #[test]
    #[should_panic]
    /// invalid meta override value in config
    fn test_invalid_meta_override() {
        let some_meta: Value = serde_json::from_str("{}").unwrap();
        let mut line = LineBuilder::new().meta(some_meta);
        let cfg = MetaRulesConfig {
            app: None,
            host: None,
            env: None,
            file: None,
            k8s_file: None,
            meta: Some("bad value".into()),
            annotations: None,
            labels: None,
        };
        let p = MetaRules::new(cfg).unwrap();
        p.process(&mut line);
    }

    #[test]
    #[should_panic]
    /// invalid annotations override value in config
    fn test_invalid_annotations_override() {
        let some_annotations: KeyValueMap = serde_json::from_str("{}").unwrap();
        let mut line = LineBuilder::new().annotations(some_annotations);
        let cfg = MetaRulesConfig {
            app: None,
            host: None,
            env: None,
            file: None,
            k8s_file: None,
            meta: None,
            annotations: Some("bad value".into()),
            labels: None,
        };
        let p = MetaRules::new(cfg).unwrap();
        p.process(&mut line);
    }

    #[test]
    #[should_panic]
    /// invalid labels override value in config
    fn test_invalid_labels_override() {
        let some_labels: KeyValueMap = serde_json::from_str("{}").unwrap();
        let mut line = LineBuilder::new().annotations(some_labels);
        let cfg = MetaRulesConfig {
            app: None,
            host: None,
            env: None,
            file: None,
            k8s_file: None,
            meta: None,
            annotations: None,
            labels: Some("bad value".into()),
        };
        let p = MetaRules::new(cfg).unwrap();
        p.process(&mut line);
    }

    #[test]
    /// k8s case: overriding File should trigger removal of CRIO log prefix
    fn test_override_file_with_crio_k8s() {
        let cfg = MetaRulesConfig {
            app: None,
            host: None,
            env: None,
            file: None,
            k8s_file: Some("REDACTED_K8S_FILE".into()),
            meta: None,
            annotations: None,
            labels: None,
        };
        let p = MetaRules::new(cfg).unwrap();
        let mut line = LineBuilder::new()
            .file("/var/log/containers/a.log")
            .line(r#"2022-04-20T00:44:16.848418974-07:00 stdout F 2022-04-20T07:44:16.848Z DEBUG {"color":"yellow","sold":true,"rating":68}"#);
        let status = p.process(&mut line);
        assert!(matches!(status, Status::Ok(_)));
        assert_eq!(
            line.get_line_buffer().unwrap(),
            r#"2022-04-20T07:44:16.848Z DEBUG {"color":"yellow","sold":true,"rating":68}"#
                .as_bytes()
        );
    }
}
