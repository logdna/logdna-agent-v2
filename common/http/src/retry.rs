use std::fs::{create_dir_all, read_dir, remove_file, File, OpenOptions};
use std::io::Read;
use std::str::FromStr;
use std::thread::sleep;
use std::time::Duration;

use chrono::prelude::Utc;
use crossbeam::{bounded, scope, Receiver, Sender};
use uuid::Uuid;

use crate::types::body::IngestBody;
use metrics::Metrics;

quick_error! {
    #[derive(Debug)]
    pub enum Error {
        Io(e: std::io::Error) {
            from()
        }
        Serde(e: serde_json::Error){
            from()
        }
        Recv(e: crossbeam::RecvError){
            from()
        }
        Send(e: crossbeam::SendError<IngestBody>){
            from()
        }
        NonUTF8(path: std::path::PathBuf){
            display("{:?} is not valid utf8", path)
        }
        InvalidFileName(s: std::string::String){
            display("{} is not a valid file name", s)
        }
    }
}

pub struct Retry {
    retry_sender: Sender<IngestBody>,
    retry_receiver: Receiver<IngestBody>,
    body_sender: Sender<IngestBody>,
}

impl Retry {
    pub fn new() -> Retry {
        let (s, r) = bounded(0);
        let (temp, _) = bounded(0);
        Retry {
            retry_sender: s,
            retry_receiver: r,
            body_sender: temp,
        }
    }

    pub fn sender(&self) -> Sender<IngestBody> {
        self.retry_sender.clone()
    }

    pub fn run(mut self, body_sender: Sender<IngestBody>) {
        self.body_sender = body_sender;

        create_dir_all("/tmp/logdna/").expect("can't create /tmp/logdna");
        scope(|s| {
            s.spawn(|_| self.handle_incoming());
            s.spawn(|_| self.handle_outgoing());
        })
        .expect("failed starting Retry")
    }

    fn handle_incoming(&self) {
        loop {
            if let Err(e) = self.poll_incoming() {
                error!("failed to write retry: {}", e)
            }
        }
    }

    fn poll_incoming(&self) -> Result<(), Error> {
        Metrics::http().increment_retries();
        let body = self.retry_receiver.recv()?;

        let file = OpenOptions::new().create(true).write(true).open(format!(
            "/tmp/logdna/{}_{}.retry",
            Utc::now().timestamp(),
            Uuid::new_v4().to_string()
        ))?;

        Ok(serde_json::to_writer(file, &body)?)
    }

    fn handle_outgoing(&self) {
        loop {
            if let Err(e) = self.poll_outgoing() {
                error!("failed to read retry: {}", e)
            }
            sleep(Duration::from_secs(15));
        }
    }

    fn poll_outgoing(&self) -> Result<(), Error> {
        let files = read_dir("/tmp/logdna/")?;

        for file in files {
            let path = file?.path();
            if path.is_dir() {
                continue;
            }

            let file_name = path
                .file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
                .ok_or_else(|| Error::NonUTF8(path.clone()))?;
            let timestamp: i64 = file_name
                .split('_')
                .map(|s| s.to_string())
                .collect::<Vec<String>>()
                .get(0)
                .and_then(|s| FromStr::from_str(s).ok())
                .ok_or_else(|| Error::InvalidFileName(file_name.clone()))?;

            if Utc::now().timestamp() - timestamp < 15 {
                continue;
            }

            let mut file = File::open(&path)?;
            let mut data = String::new();
            file.read_to_string(&mut data)?;
            remove_file(&path)?;
            let body = serde_json::from_str(&data)?;
            self.body_sender.send(body)?;
        }

        Ok(())
    }
}
