#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;
#[macro_use]
extern crate quick_error;

use std::collections::HashMap;
use std::env;
use std::ffi::OsStr;
use std::fs::read_dir;
use std::path::PathBuf;
use std::sync::Arc;
use std::{io, process};

use crossbeam::scope;
use inotify::{EventMask, Inotify, WatchMask};
use kube::{api::Api, client::APIClient, config};
use parking_lot::Mutex;
use regex::Regex;

use http::types::body::{KeyValueMap, LineBuilder};
use kube::api::{Informer, WatchEvent};
use middleware::{Middleware, Status};
use std::str::FromStr;

lazy_static! {
    static ref K8S_REG: Regex = Regex::new(
        r#"^/var/log/containers/([a-z0-9A-Z\-.]+)_([a-z0-9A-Z\-.]+)_([a-z0-9A-Z\-.]+)-([a-z0-9]{64}).log$"#
    ).expect("Regex::new()");
}

quick_error! {
    #[derive(Debug)]
    enum Error {
        Io(e: std::io::Error) {
            from()
            display("{}", e)
        }
        Utf(e: std::string::FromUtf8Error) {
            from()
            display("{}", e)
        }
        Regex {
            from()
            display("failed to parse path")
        }
        K8s(e: kube::Error) {
            from()
            display("{}", e)
        }
    }
}

pub struct K8s {
    metadata: Mutex<HashMap<PathBuf, Metadata>>,
    inotify: Arc<Mutex<Inotify>>,
    api: APIClient,
}

impl K8s {
    pub fn new() -> Self {
        let config = match config::incluster_config() {
            Ok(v) => v,
            Err(e) => {
                error!("failed to connect to k8s cluster: {}", e);
                process::exit(1);
            }
        };

        let api = APIClient::new(config);

        let this = K8s {
            metadata: Mutex::new(HashMap::new()),
            inotify: Arc::new(Mutex::new(create_inotify().expect("Inotify::create()"))),
            api,
        };

        this.update_all();
        this
    }

    fn update_all(&self) {
        if let Ok(files) = read_dir("/var/log/containers") {
            for file in files {
                if let Ok(file) = file {
                    let symlink = file.path();
                    if symlink.is_dir() {
                        continue;
                    }
                    if let Err(e) = self.update_metadata(symlink) {
                        error!("error updating k8s metadata: {}", e)
                    }
                };
            }
        }
    }

    fn update_metadata(&self, symlink: PathBuf) -> Result<(), Error> {
        let (name, namespace) = parse_container_path(&symlink).ok_or(Error::Regex)?;
        if let Ok(Some(real)) = canonicalize(&symlink) {
            let resource = Api::v1Pod(self.api.clone());
            let object = resource.within(&namespace).get(&name)?;
            let pod_meta = PodMetadata {
                name,
                namespace,
                labels: object.metadata.labels.into(),
                annotations: object.metadata.annotations.into(),
            };
            info!(
                "added ({}) {}/{}",
                self.metadata.lock().len(),
                pod_meta.namespace,
                pod_meta.name
            );
            self.metadata
                .lock()
                .insert(real, Metadata { pod_meta, symlink });
        }
        Ok(())
    }

    fn remove_metadata(&self, symlink: &PathBuf) {
        let mut metadata = self.metadata.lock();

        let mut keys_to_remove = Vec::new();
        for (key, meta) in metadata.iter() {
            if meta.symlink == *symlink {
                keys_to_remove.push(key.clone());
            }
        }

        keys_to_remove.iter().for_each(|k| {
            metadata.remove(k).map(|meta| {
                info!(
                    "removed ({}) {}/{}",
                    metadata.len(),
                    meta.pod_meta.namespace,
                    meta.pod_meta.name
                )
            });
        });
    }

    fn process_inotify(&self) {
        let mut buff = [0u8; 8_192];
        loop {
            let events = match self.inotify.lock().read_events_blocking(&mut buff) {
                Ok(v) => v,
                Err(_) => {
                    continue;
                }
            };

            for event in events {
                if event.mask.contains(EventMask::CREATE) {
                    if let Err(e) = self.update_metadata(event_name_to_symlink(event.name)) {
                        error!("error updating k8s metadata: {}", e)
                    }
                } else if event.mask.contains(EventMask::DELETE) {
                    self.remove_metadata(&event_name_to_symlink(event.name));
                }
            }
        }
    }

    fn process_poll(&self) {
        let resource = Api::v1Pod(self.api.clone());

        let mut inf = Informer::new(resource.clone())
            .init()
            .expect("k8s informer failed");
        if let Ok(node) = env::var("NODE_NAME") {
            inf = inf.fields(&format!("spec.nodeName={}", node));
        }

        loop {
            if let Err(e) = inf.poll() {
                error!("error polling api server: {}", e);
                continue;
            };

            while let Some(event) = inf.pop() {
                match event {
                    WatchEvent::Modified(pod) => {
                        let name = pod.metadata.name;
                        let namespace = pod.metadata.namespace.unwrap_or("default".into());
                        for (_, meta) in self.metadata.lock().iter_mut() {
                            if meta.pod_meta.name == name && meta.pod_meta.namespace == namespace {
                                info!("updated {}/{}", namespace, name);
                                meta.pod_meta.annotations = pod.metadata.annotations.clone().into();
                                meta.pod_meta.labels = pod.metadata.labels.clone().into();
                            }
                        }
                    }
                    WatchEvent::Error(e) => error!("api server watch error: {}", e),
                    _ => {}
                }
            }
        }
    }
}

impl Middleware for K8s {
    fn run(&self) {
        //FIXME: integrate into env config
        let poll = env::var("LOGDNA_K8S_POLL")
            .ok()
            .and_then(|var| bool::from_str(&var).ok())
            .unwrap_or(false);
        if poll {
            scope(|s| {
                s.spawn(|_| self.process_inotify());
                s.spawn(|_| self.process_poll());
            })
            .expect("K8s::middleware::run()")
        } else {
            self.process_inotify()
        }
    }

    fn process(&self, mut line: LineBuilder) -> Status {
        if let Some(ref file) = line.file {
            if let Some(real) = PathBuf::from(file).parent().map(|p| p.to_path_buf()) {
                if let Some(meta) = self.metadata.lock().get(&real) {
                    if let Some(file) = meta.symlink.to_str() {
                        line = line.file(file);
                    }
                    line = line.labels(meta.pod_meta.labels.clone());
                    line = line.annotations(meta.pod_meta.annotations.clone());
                }
            }
        }
        Status::Ok(line)
    }
}

fn event_name_to_symlink(name: Option<&OsStr>) -> PathBuf {
    PathBuf::from("/var/log/containers/").join(name.unwrap_or_default())
}

fn canonicalize(symlink: &PathBuf) -> io::Result<Option<PathBuf>> {
    symlink
        .canonicalize()
        .map(|p| p.parent().map(|p| p.to_path_buf()))
}

fn create_inotify() -> io::Result<Inotify> {
    let mut inotify = Inotify::init()?;
    inotify.add_watch(
        "/var/log/containers/",
        WatchMask::CREATE | WatchMask::DELETE | WatchMask::DONT_FOLLOW,
    )?;
    Ok(inotify)
}

fn parse_container_path(path: &PathBuf) -> Option<(String, String)> {
    let captures = K8S_REG.captures(path.to_str()?)?;
    Some((
        captures.get(1)?.as_str().into(),
        captures.get(2)?.as_str().into(),
    ))
}

struct PodMetadata {
    name: String,
    namespace: String,
    labels: KeyValueMap,
    annotations: KeyValueMap,
}

struct Metadata {
    pod_meta: PodMetadata,
    symlink: PathBuf,
}
