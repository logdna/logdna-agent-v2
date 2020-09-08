extern crate log;

#[cfg(use_systemd)]
pub mod source {
    #[macro_use]
    use std::time::UNIX_EPOCH;

    use systemd::journal::{Journal, JournalFiles, JournalRecord, JournalSeek};

    use http::types::body::LineBuilder;

    use chrono::{Local, TimeZone};

    use source::Source;

    const KEY_TRANSPORT: &str = "_TRANSPORT";
    const KEY_HOSTNAME: &str = "_HOSTNAME";
    const KEY_COMM: &str = "_COMM";
    const KEY_PID: &str = "_PID";
    const KEY_MESSAGE: &str = "MESSAGE";

    const TRANSPORT_AUDIT: &str = "audit";
    const TRANSPORT_DRIVER: &str = "driver";
    const TRANSPORT_SYSLOG: &str = "syslog";
    const TRANSPORT_JOURNAL: &str = "journal";
    const TRANSPORT_STDOUT: &str = "stdout";
    const TRANSPORT_KERNEL: &str = "kernel";

    enum RecordStatus {
        Line(String),
        BadLine,
        NoLines,
    }

    pub struct JournaldSource {
        reader: Journal,
    }

    impl JournaldSource {
        pub fn new() -> JournaldSource {
            let mut reader = Journal::open(JournalFiles::All, false, false)
                .expect("Could not open journald reader");
            reader
                .seek(JournalSeek::Tail)
                .expect("Could not seek to tail of journald logs");

            JournaldSource { reader }
        }

        fn process_next_record(&mut self) -> RecordStatus {
            let record = match self.reader.next_record() {
                Ok(Some(record)) => record,
                Ok(None) => return RecordStatus::NoLines,
                Err(e) => panic!("Unable to read next record from journald: {}", e),
            };

            let timestamp = match self.reader.timestamp() {
                Ok(timestamp) => Local
                    .timestamp(
                        timestamp.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64,
                        0,
                    )
                    .format("%b %d %H:%M:%S")
                    .to_string(),
                Err(e) => {
                    warn!(
                        "Unable to read timestamp associated with journald record: {}",
                        e
                    );
                    Local::now().format("%b %d %H:%M:%S").to_string()
                }
            };

            match record.get(KEY_TRANSPORT) {
                Some(transport) => match transport.as_ref() {
                    TRANSPORT_AUDIT => self.process_audit_record(&record, timestamp),
                    TRANSPORT_DRIVER | TRANSPORT_SYSLOG | TRANSPORT_JOURNAL | TRANSPORT_STDOUT => {
                        self.process_default_record(&record, transport, timestamp)
                    }
                    TRANSPORT_KERNEL => self.process_kernel_record(&record, timestamp),
                    _ => {
                        warn!(
                            "Got unexpected transport for journald record: {}",
                            transport
                        );
                        RecordStatus::BadLine
                    }
                },
                None => {
                    warn!("Unable to get transport of journald record");
                    RecordStatus::BadLine
                }
            }
        }

        fn process_audit_record(&self, record: &JournalRecord, timestamp: String) -> RecordStatus {
            let hostname = match record.get(KEY_HOSTNAME) {
                Some(hostname) => hostname,
                None => {
                    warn!("Unable to get hostname of journald audit record");
                    return RecordStatus::BadLine;
                }
            };

            let pid = match record.get(KEY_PID) {
                Some(pid) => pid,
                None => {
                    warn!("Unable to get pid of journald audit record");
                    return RecordStatus::BadLine;
                }
            };

            let message = match record.get(KEY_MESSAGE) {
                Some(message) => message,
                None => {
                    warn!("Unable to get message of journald audit record");
                    return RecordStatus::BadLine;
                }
            };

            RecordStatus::Line(format!(
                "{} {} audit[{}]: {}",
                timestamp, hostname, pid, message
            ))
        }

        fn process_default_record(
            &self,
            record: &JournalRecord,
            record_type: &String,
            timestamp: String,
        ) -> RecordStatus {
            let hostname = match record.get(KEY_HOSTNAME) {
                Some(hostname) => hostname,
                None => {
                    warn!("Unable to get hostname of journald {} record", record_type);
                    return RecordStatus::BadLine;
                }
            };

            let comm = match record.get(KEY_COMM) {
                Some(comm) => comm,
                None => {
                    warn!("Unable to get comm of journald {} record", record_type);
                    return RecordStatus::BadLine;
                }
            };

            let pid = match record.get(KEY_PID) {
                Some(pid) => pid,
                None => {
                    warn!("Unable to get pid of journald {} record", record_type);
                    return RecordStatus::BadLine;
                }
            };

            let message = match record.get(KEY_MESSAGE) {
                Some(message) => message,
                None => {
                    warn!("Unable to get message of journald {} record", record_type);
                    return RecordStatus::BadLine;
                }
            };

            RecordStatus::Line(format!(
                "{} {} {}[{}]: {}",
                timestamp, hostname, comm, pid, message
            ))
        }

        fn process_kernel_record(&self, record: &JournalRecord, timestamp: String) -> RecordStatus {
            let hostname = match record.get(KEY_HOSTNAME) {
                Some(hostname) => hostname,
                None => {
                    warn!("Unable to get hostname of journald kernel record");
                    return RecordStatus::BadLine;
                }
            };

            let message = match record.get(KEY_MESSAGE) {
                Some(message) => message,
                None => {
                    warn!("Unable to get message of journald kernel record");
                    return RecordStatus::BadLine;
                }
            };

            RecordStatus::Line(format!("{} {} kernel: {}", timestamp, hostname, message))
        }
    }

    impl<'a> Source<'a> for JournaldSource {
        fn drain(&mut self, callback: &mut Box<dyn FnMut(Vec<LineBuilder>) + 'a>) {
            loop {
                match self.process_next_record() {
                    RecordStatus::Line(line) => callback(vec![LineBuilder::new().line(line)]),
                    RecordStatus::BadLine => continue,
                    RecordStatus::NoLines => break,
                };
            }
        }
    }
}
