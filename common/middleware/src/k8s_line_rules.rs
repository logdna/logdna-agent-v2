use crate::{Middleware, Status};
use http::types::body::LineBufferMut;
use lazy_static::lazy_static;
use multimap::MultiMap;
use regex::{Regex, RegexSet};
use thiserror::Error;

const NAMESPACE_KEY: &str = "namespace";
const POD_NAME_KEY: &str = "pod";
const LABEL_KEY: &str = "label";
const ANNOTATION_KEY: &str = "annotation";

lazy_static! {
    static ref REG_KEYVAL: Regex = Regex::new(r#"^([a-z0-9]+):([a-z0-9]*(|-)[a-z0-9]+)"#).unwrap();
    static ref REG_NAMESPACE: Regex = Regex::new(r#"^namespace:([a-z0-9]*(|-)[a-z0-9]+)"#).unwrap();
    static ref REG_POD: Regex = Regex::new(r#"^pod:([a-z0-9]*(|-)[a-z0-9]+)"#).unwrap();
    static ref REG_LABEL: Regex =
        Regex::new(r#"^label.([a-z0-9]*(|-)[a-z0-9]+:[a-z0-9]*(|-)[a-z0-9]+)"#).unwrap();
    static ref REG_ANNOTATION: Regex =
        Regex::new(r#"^annotation.([a-z0-9]*(|-)[a-z0-9]+:[a-z0-9]*(|-)[a-z0-9]+)"#).unwrap();
}

pub struct K8sLineRules {
    namespace: RegexSet,
    pod_name: RegexSet,
    labels: MultiMap<String, String>,
    annotations: MultiMap<String, String>,
}

pub struct K8sLineFilter {
    exclusion: Option<K8sLineRules>,
    inclusion: Option<K8sLineRules>,
}

#[derive(Clone, Debug, Error)]
pub enum K8sLineRulesError {
    #[error(transparent)]
    RegexError(regex::Error),
}

impl K8sLineRules {
    pub fn new(
        namespace: &[String],
        pod_name: &[String],
        labels: &[String],
        annotations: &[String],
    ) -> Result<K8sLineRules, K8sLineRulesError> {
        let mut namespace_regex_set = Vec::with_capacity(namespace.len());
        let mut pod_regex_set = Vec::with_capacity(pod_name.len());
        let mut label_map: MultiMap<String, String> = MultiMap::new();
        let mut annotation_map: MultiMap<String, String> = MultiMap::new();

        for ns in namespace.iter() {
            namespace_regex_set.push(format!(
                r#"/([a-z0-9A-Z\-.]+)_{}_([a-z0-9A-Z\-.]+)-([a-z0-9]{{64}}).log$"#,
                regex::escape(ns),
            ));
        }

        for p in pod_name.iter() {
            pod_regex_set.push(format!(
                r#"/{}_([a-z0-9A-Z\-.]+)_([a-z0-9A-Z\-.]+)-([a-z0-9]{{64}}).log$"#,
                regex::escape(p),
            ));
        }

        for label in labels.iter() {
            let capture = REG_KEYVAL.captures(label).unwrap();
            let key: String = capture.get(1).map(|m| m.as_str()).unwrap().to_string();
            let value: String = capture.get(2).map(|m| m.as_str()).unwrap().to_string();
            label_map.insert(key, value)
        }
        for annotation in annotations.iter() {
            let capture = REG_KEYVAL.captures(annotation).unwrap();
            let key: String = capture.get(1).map(|m| m.as_str()).unwrap().to_string();
            let value: String = capture.get(2).map(|m| m.as_str()).unwrap().to_string();
            annotation_map.insert(key, value)
        }

        Ok(K8sLineRules {
            namespace: RegexSet::new(namespace_regex_set).map_err(K8sLineRulesError::RegexError)?,
            pod_name: RegexSet::new(pod_regex_set).map_err(K8sLineRulesError::RegexError)?,
            labels: label_map,
            annotations: annotation_map,
        })

        //Ok(K8sLineFilter {
        //    exclusion: Some(k8s_exclusion_line_rules),
        //    inclusion: None,
        //})
    }

    fn process_line<'a>(
        &self,
        line: &'a mut dyn LineBufferMut,
    ) -> Status<&'a mut dyn LineBufferMut> {
        let file_value = line.get_file().unwrap();
        let label_value = line.get_labels().unwrap();
        let annotation_value = line.get_annotations().unwrap();

        // If it doesn't match any inclusion rule -> skip
        //if !self.inclusion.is_empty() && !self.inclusion.is_match(value) {
        //    return Status::Skip;
        //}

        // If any exclusion rule matches -> skip
        if RegexSet::is_match(&self.namespace, file_value) {
            return Status::Skip;
        }

        if RegexSet::is_match(&self.pod_name, file_value) {
            return Status::Skip;
        }

        // TODO: need to account for HashMap<String, Vec<String>> using map.get_vec()
        for (k, v) in label_value.iter() {
            if let Some(value) = self.labels.get_vec(k) {
                for i in value.iter() {
                    if i == v {
                        return Status::Skip;
                    }
                }
            }
        }
        for (k, v) in annotation_value.iter() {
            if let Some(value) = self.annotations.get_vec(k) {
                for i in value.iter() {
                    if i == v {
                        return Status::Skip;
                    }
                }
            }
        }

        Status::Ok(line)
    }
}

impl Middleware for K8sLineRules {
    fn run(&self) {}

    fn process<'a>(&self, line: &'a mut dyn LineBufferMut) -> Status<&'a mut dyn LineBufferMut> {
        if self.namespace.is_empty()
            && self.pod_name.is_empty()
            && self.labels.is_empty()
            && self.annotations.is_empty()
        {
            // Avoid unnecessary allocations when no rules were defined
            return Status::Ok(line);
        }

        match line.get_line_buffer() {
            None => Status::Skip,
            Some(_) => self.process_line(line),
        }
    }
}

fn get_rule_object(rules: &[String]) -> MultiMap<&str, String> {
    let mut rules_vec = MultiMap::new();
    for rule in rules.iter() {
        match REG_NAMESPACE.captures(rule) {
            Some(val) => {
                rules_vec.insert(
                    NAMESPACE_KEY,
                    val.get(1).map(|m| m.as_str()).unwrap().to_string(),
                );
            }
            None => (),
        }
        match REG_POD.captures(rule) {
            Some(val) => {
                rules_vec.insert(
                    POD_NAME_KEY,
                    val.get(1).map(|m| m.as_str()).unwrap().to_string(),
                );
            }
            None => (),
        }
        match REG_LABEL.captures(rule) {
            Some(val) => {
                rules_vec.insert(
                    LABEL_KEY,
                    val.get(1).map(|m| m.as_str()).unwrap().to_string(),
                );
            }
            None => (),
        }
        match REG_ANNOTATION.captures(rule) {
            Some(val) => {
                rules_vec.insert(
                    ANNOTATION_KEY,
                    val.get(1).map(|m| m.as_str()).unwrap().to_string(),
                );
            }
            None => (),
        }
    }

    rules_vec
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::types::body::{KeyValueMap, LineBuilder};

    #[test]
    fn test_option_parsing_rules() {
        let test_vec: Vec<String> = vec![
            "namespace:testnamespace".to_string(),
            "pod:some-name".to_string(),
            "namespace:othernamespace".to_string(),
            "label.app:name".to_string(),
            "label.type:network".to_string(),
            "annotation.owner:secret-agent".to_string(),
        ];

        let rule_results = get_rule_object(&test_vec);
        let namespace_results = rule_results.get_vec("namespace").unwrap();
        let pod_results = rule_results.get_vec("pod").unwrap();
        let label_results = rule_results.get_vec("label").unwrap();
        let annotation_results = rule_results.get_vec("annotation").unwrap();

        assert_eq!(namespace_results[0], "testnamespace");
        assert_eq!(namespace_results[1], "othernamespace");
        assert_eq!(pod_results[0], "some-name");
        assert_eq!(label_results[0], "app:name");
        assert_eq!(label_results[1], "type:network");
        assert_eq!(annotation_results[0], "owner:secret-agent");
    }

    #[test]
    fn test_k8s_line_rules_undefined() {
        let namespace = &[];
        let pod_name = &[];
        let test_label = &[];
        let test_annotation = &[];
        let k8s_rules =
            K8sLineRules::new(namespace, pod_name, test_label, test_annotation).unwrap();

        let mut test_line = LineBuilder::new();

        let status = k8s_rules.process(&mut test_line);
        assert!(matches!(status, Status::Ok(_)));
    }

    #[test]
    fn test_k8s_line_rule_new() {
        let test_file_path = r#"/var/log/containers/pod-name_namespace_app-name-63d7c40bf1ece5ff559f49ef2da8f01163df85f611027a9d4bf5fef6e1a643bc.log"#;
        let test_app_name_0 = "app-name".to_string();
        let test_app_name_1 = "other-name".to_string();
        let test_owner_name = "secret-agent".to_string();

        let namespace = &[String::from("namespace")];
        let pod_name = &[String::from("pod-name")];
        let test_label = &[String::from("app:app-name"), String::from("app:other-name")];
        let test_annotation = &[String::from("owner:secret-agent")];
        let k8s_rules =
            K8sLineRules::new(namespace, pod_name, test_label, test_annotation).unwrap();

        assert!(k8s_rules.namespace.is_match(test_file_path));
        assert!(k8s_rules.pod_name.is_match(test_file_path));
        assert_eq!(
            k8s_rules
                .labels
                .get_vec("app")
                .iter()
                .map(|v| v[0].clone())
                .collect::<String>(),
            test_app_name_0
        );
        assert_eq!(
            k8s_rules
                .labels
                .get_vec("app")
                .iter()
                .map(|v| v[1].clone())
                .collect::<String>(),
            test_app_name_1
        );
        assert_eq!(
            k8s_rules
                .annotations
                .get_vec("owner")
                .iter()
                .map(|v| v[0].clone())
                .collect::<String>(),
            test_owner_name
        );
    }

    #[test]
    fn test_k8s_line_rule_exclude() {
        let test_namespace = &[String::from("logdna-agent")];
        let test_pod_name = &[String::from("pod-name")];
        let test_label = &[
            String::from("app:name"),
            String::from("app:other-name"),
            String::from("type:network"),
        ];
        let test_annotation = &[String::from("owner:secret-agent")];

        let k8s_rules =
            K8sLineRules::new(test_namespace, test_pod_name, test_label, test_annotation);

        // Test no match
        let mut label_kv_map = KeyValueMap::new();
        let mut annotation_kv_map = KeyValueMap::new();
        let mut test_line = LineBuilder::new()
            .line("test-info")
            .file("/var/log/containers/random-name_namespace_app-name-63d7c40bf1ece5ff559f49ef2da8f01163df85f611027a9d4bf5fef6e1a643bc.log")
            .labels(label_kv_map.add("app", "crazyapp"))
            .annotations(annotation_kv_map.add("owner", "random-agent"));
        let mut status = k8s_rules.as_ref().unwrap().process(&mut test_line);
        assert!(matches!(status, Status::Ok(_)));

        // Test namespace
        label_kv_map = KeyValueMap::new();
        annotation_kv_map = KeyValueMap::new();
        test_line = LineBuilder::new()
            .line("test-info")
            .file("/var/log/containers/random-pod_logdna-agent_app-name-63d7c40bf1ece5ff559f49ef2da8f01163df85f611027a9d4bf5fef6e1a643bc.log")
            .labels(label_kv_map.add("app", "crazyapp"))
            .annotations(annotation_kv_map.add("owner", "random-agent"));
        status = k8s_rules.as_ref().unwrap().process(&mut test_line);
        assert!(matches!(status, Status::Skip));

        // Test pod-name
        label_kv_map = KeyValueMap::new();
        annotation_kv_map = KeyValueMap::new();
        test_line = LineBuilder::new()
            .line("test-info")
            .file("/var/log/containers/pod-name_namespace_app-name-63d7c40bf1ece5ff559f49ef2da8f01163df85f611027a9d4bf5fef6e1a643bc.log")
            .labels(label_kv_map.add("app", "randomapp"))
            .annotations(annotation_kv_map.add("owner", "random-agent"));
        status = k8s_rules.as_ref().unwrap().process(&mut test_line);
        assert!(matches!(status, Status::Skip));

        // Test labels
        label_kv_map = KeyValueMap::new();
        annotation_kv_map = KeyValueMap::new();
        test_line = LineBuilder::new()
            .line("test-info")
            .file("/var/log/containers/random-pod_namespace_app-name-63d7c40bf1ece5ff559f49ef2da8f01163df85f611027a9d4bf5fef6e1a643bc.log")
            .labels(label_kv_map.add("app", "other-name"))
            .annotations(annotation_kv_map.add("owner", "random-agent"));
        status = k8s_rules.as_ref().unwrap().process(&mut test_line);
        assert!(matches!(status, Status::Skip));

        // Test annotations
        label_kv_map = KeyValueMap::new();
        annotation_kv_map = KeyValueMap::new();
        test_line = LineBuilder::new()
            .line("test-info")
            .file("/var/log/containers/random-pod_namespace_app-name-63d7c40bf1ece5ff559f49ef2da8f01163df85f611027a9d4bf5fef6e1a643bc.log")
            .labels(label_kv_map.add("app", "randomapp"))
            .annotations(annotation_kv_map.add("owner", "secret-agent"));
        status = k8s_rules.as_ref().unwrap().process(&mut test_line);
        assert!(matches!(status, Status::Skip));
    }
}
