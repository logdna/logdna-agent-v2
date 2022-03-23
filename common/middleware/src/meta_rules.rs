use crate::{Middleware, Status};
//use log::debug;
use http::types::body::{KeyValueMap, LineBufferMut};
use std::collections::HashMap;

const OVER_META_APP: Option<&'static str> = option_env!("LOGDNA_META_APP");
const OVER_META_HOST: Option<&'static str> = option_env!("LOGDNA_META_HOST");
const OVER_META_ENV: Option<&'static str> = option_env!("LOGDNA_META_ENV");
const OVER_META_FILE: Option<&'static str> = option_env!("LOGDNA_META_FILE");
const OVER_META_K8S_FILE: Option<&'static str> = option_env!("LOGDNA_META_K8S_FILE");
const OVER_META_JSON: Option<&'static str> = option_env!("LOGDNA_META_JSON");
const OVER_META_ANNOTATIONS: Option<&'static str> = option_env!("LOGDNA_META_ANNOTATIONS");
const OVER_META_LABELS: Option<&'static str> = option_env!("LOGDNA_META_LABELS");

pub struct MetaRules {
    env_map: HashMap<String, String>,
    // LineMeta fields
    over_app: Option<&'static str>,
    over_host: Option<&'static str>,
    over_env: Option<&'static str>,
    over_file: Option<&'static str>,
    over_k8s_file: Option<&'static str>, // override file field in k8s lines only
    over_meta: Option<&'static str>,
    over_annotations: Option<HashMap<String, String>>,
    over_labels: Option<HashMap<String, String>>,
}

#[derive(Debug, thiserror::Error)]
pub enum MetaRulesError {
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
}

impl MetaRules {
    pub fn new() -> Result<MetaRules, MetaRulesError> {
        // for (key, value) in os_env_hashmap().into_iter() {
        //     println!("{} = {:?}", key, value);
        // }
        Ok(MetaRules {
            env_map: os_env_hashmap(),
            //TODO: extract to Config
            over_app: OVER_META_APP,
            over_host: OVER_META_HOST,
            over_env: OVER_META_ENV,
            over_file: OVER_META_FILE,
            over_k8s_file: OVER_META_K8S_FILE,
            over_meta: OVER_META_JSON,
            over_annotations: OVER_META_ANNOTATIONS
                .map_or_else(|| None, |str| serde_json::from_str(str).unwrap()),
            over_labels: OVER_META_LABELS
                .map_or_else(|| None, |str| serde_json::from_str(str).unwrap()),
        })
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

    /// Override line meta fields
    /// [ create map ]  =>  [ substitute and insert to map ]  =>  [ substitute ]  =>  [ override ]
    ///  os env vars         "overload" labels                     "overload" fields   line fields
    ///  line fields         "overload" annotations
    ///  line labels
    ///  line annotations
    /// Excluded:
    ///  line meta
    ///
    fn process_line<'a>(
        &self,
        line: &'a mut dyn LineBufferMut,
    ) -> Status<&'a mut dyn LineBufferMut> {
        if !self.is_active() {
            return Status::Ok(line);
        }
        //
        // create map
        //
        let mut meta_map: HashMap<String, String> = self.env_map.clone();
        if line.get_annotations().is_some() {
            for (k, v) in line.get_annotations().unwrap().iter() {
                meta_map.insert(k.clone(), v.clone());
            }
        }
        if line.get_labels().is_some() {
            for (k, v) in line.get_labels().unwrap().iter() {
                meta_map.insert(k.clone(), v.clone());
            }
        }
        line.get_app()
            .map(|v| meta_map.insert("line.app".to_string(), v.to_string()));
        line.get_host()
            .map(|v| meta_map.insert("line.host".to_string(), v.to_string()));
        line.get_env()
            .map(|v| meta_map.insert("line.env".to_string(), v.to_string()));
        line.get_file()
            .map(|v| meta_map.insert("line.file".to_string(), v.to_string()));
        //
        // substitute "overload" labels & annotations
        //
        if self.over_annotations.is_some() {
            let mut new_annotations = KeyValueMap::new();
            for (k, v) in self.over_annotations.clone().unwrap().iter() {
                let v = substitute(v, &meta_map);
                meta_map.insert(k.clone(), v.clone());
                new_annotations.insert(k.clone(), v.clone());
            }
            if let Err(_) = line.set_annotations(new_annotations) {}
        }
        if self.over_labels.is_some() {
            let mut new_labels = KeyValueMap::new();
            for (k, v) in self.over_labels.clone().unwrap().iter() {
                let v = substitute(v, &meta_map);
                meta_map.insert(k.clone(), v.clone());
                new_labels.insert(k.clone(), v.clone());
            }
            if let Err(_) = line.set_labels(new_labels) {}
        }
        //
        // substitute "overload" fields and override line fields
        // TODO: error handling for set_ calls
        //
        if self.over_app.is_some() {
            let app = substitute(self.over_host.unwrap(), &meta_map);
            if let Err(_) = line.set_app(app) {}
        }
        if self.over_host.is_some() {
            let host = substitute(self.over_host.unwrap(), &meta_map);
            if let Err(_) = line.set_host(host) {}
        }
        if self.over_env.is_some() {
            let env = substitute(self.over_env.unwrap(), &meta_map);
            if let Err(_) = line.set_env(env) {}
        }
        if self.over_file.is_some() && line.get_file().is_some() {
            let file = substitute(self.over_file.unwrap(), &meta_map);
            if let Err(_) = line.set_file(file) {}
        }
        // k8s line shave non empty annotations/labels
        if self.over_k8s_file.is_some()
            && (line.get_annotations().is_some() || line.get_labels().is_some())
        {
            let file = substitute(self.over_k8s_file.unwrap(), &meta_map);
            if let Err(_) = line.set_file(file) {}
        }
        if self.over_meta.is_some() {
            let meta = substitute(self.over_meta.unwrap(), &meta_map);
            let val = serde_json::from_str(&meta).unwrap();
            if let Err(_) = line.set_meta(val) {}
        }
        Status::Ok(line)
    }
}

impl Middleware for MetaRules {
    fn run(&self) {}
    fn process<'a>(&self, line: &'a mut dyn LineBufferMut) -> Status<&'a mut dyn LineBufferMut> {
        match line.get_line_buffer() {
            None => Status::Skip,
            Some(_) => self.process_line(line),
        }
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

pub fn substitute(template: &str, variables: &HashMap<String, String>) -> String {
    let mut output = String::from(template);
    for (k, v) in variables {
        let from = format!("${{{}}}", k);
        output = output.replace(&from, &v);
    }
    output
}

#[cfg(test)]
mod tests {
    use crate::meta_rules::{os_env_hashmap, substitute};

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
        ]);
        let templ = r#"{"key1":"${val1}", "key2":"${val2}"}"#;
        let res = substitute(templ, &vals);
        assert_eq!(res, r#"{"key1":"1", "key2":"2"}"#);
    }
}
