mod error;
use crate::journalctl::error::JournalCtlError;
use bytes::{Buf, BytesMut};

use http::types::body::LineBuilder;

use combine::{
    error::{ParseError, StreamError},
    many1, none_of, one_of,
    parser::{
        byte::{alpha_num, num::le_u64},
        choice::choice,
        combinator::{any_partial_state, AnyPartialState},
        range::{recognize, take},
    },
    skip_many, skip_many1,
    stream::{easy, PartialStream, RangeStream, StreamErrorFor},
    token, Parser,
};

use futures::{Stream, StreamExt};
use log::{info, trace, warn};
use tokio_util::codec::{Decoder, FramedRead};

use std::convert::TryInto;
use std::process::Stdio;

const JOURNALCTL_CMD: &str = "journalctl";
const KEY_MESSAGE: &str = "MESSAGE";
const KEY_SYSTEMD_UNIT: &str = "_SYSTEMD_UNIT";
const KEY_SYSLOG_IDENTIFIER: &str = "SYSLOG_IDENTIFIER";
const KEY_CONTAINER_NAME: &str = "CONTAINER_NAME";
const DEFAULT_APP: &str = "UNKNOWN_SYSTEMD_APP";

#[derive(Default)]
pub struct JournaldExportDecoder {
    state: AnyPartialState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FieldValue {
    Bytes(Vec<u8>),
    Utf8(String),
}

impl FieldValue {
    pub fn to_string_lossy(&self) -> String {
        match self {
            FieldValue::Bytes(b) => String::from_utf8_lossy(b).to_string(),
            FieldValue::Utf8(s) => s.clone(),
        }
    }
}

type JournalRecord = std::collections::HashMap<String, FieldValue>;

// The actual parser for the journald export format
fn decode_parser<'a, Input>(
) -> impl Parser<Input, Output = JournalRecord, PartialState = AnyPartialState> + 'a
where
    Input: RangeStream<Token = u8, Range = &'a [u8]> + 'a,
    // Necessary due to rust-lang/rust#24159
    Input::Error: ParseError<Input::Token, Input::Range, Input::Position>,
{
    /*
    Two journal entries that follow each other are separated by a double newline.

     Journal fields consisting only of valid non-control UTF-8 codepoints are serialized as they are:
         field name, followed by '=', followed by field data, followed by a newline as separator to the next field.
     Other journal fields are serialized in a special binary safe way:
         field name, followed by newline, followed by a binary 64bit little endian size value, followed by the binary field data,
         followed by a newline as separator to the next field.
     Entry metadata that is not actually a field is serialized like it was a field, but beginning with two underscores.
     More specifically, __CURSOR=, __REALTIME_TIMESTAMP=, __MONOTONIC_TIMESTAMP= are introduced this way.
     The order in which fields appear in an entry is undefined and might be different for each entry that is serialized.

     And that's it.
     */

    any_partial_state(
        (
            many1::<std::collections::HashMap<_, _>, _, _>(
                (
                    // Get the key
                    recognize(skip_many1(choice((alpha_num(), token(b'_'))))).and_then(
                        |bytes: &[u8]| {
                            std::str::from_utf8(bytes)
                                .map(|s| s.to_string())
                                .map_err(StreamErrorFor::<Input>::other)
                        },
                    ),
                    // Get the Value
                    one_of(b"=\n"[..].iter().copied()).then_partial(|&mut c| {
                        // If the k/v separator is a = it's just utf8
                        if c == b'=' {
                            recognize(skip_many(none_of(b"\n".iter().copied())))
                                .and_then(|bytes: &[u8]| {
                                    std::str::from_utf8(bytes)
                                        .map(|s| FieldValue::Utf8(s.to_string()))
                                        .map_err(StreamErrorFor::<Input>::other)
                                })
                                .left()
                        } else {
                            // else it must be a newline, therefor treat it as bytes
                            le_u64()
                                .then_partial(|vlen: &mut u64| {
                                    let vlen: usize = (*vlen).try_into().unwrap();
                                    take(vlen)
                                        .map(|bytes: &[u8]| FieldValue::Bytes(bytes.to_owned()))
                                })
                                .right()
                        }
                    }),
                    // lines within an entry are line separated
                    token(b'\n'),
                )
                    .map(|(key, value, _)| (key, value)),
            ),
            // entries are line separated
            token(b'\n'),
        )
            .map(|(kvs, _)| kvs),
    )
}

// The actual parser for the journald export format
fn find_next_record<'a, Input>(
) -> impl Parser<Input, Output = (), PartialState = AnyPartialState> + 'a
where
    Input: RangeStream<Token = u8, Range = &'a [u8]> + 'a,
    // Necessary due to rust-lang/rust#24159
    Input::Error: ParseError<Input::Token, Input::Range, Input::Position>,
{
    any_partial_state(
        (
            skip_many(none_of(b"\n".iter().copied())),
            // entries are line separated
            token(b'\n'),
            token(b'\n'),
        )
            .map(|_| ()),
    )
}

impl JournaldExportDecoder {
    fn process_default_record(
        record: &JournalRecord,
    ) -> Result<Option<LineBuilder>, JournalCtlError> {
        let message = match record.get(KEY_MESSAGE) {
            Some(message) => message,
            None => {
                warn!("unable to get message of journald record");
                return Err(JournalCtlError::RecordMissingField(KEY_MESSAGE.into()));
            }
        };

        let default_app = String::from(DEFAULT_APP);
        let app = record
            .get(KEY_CONTAINER_NAME)
            .map(FieldValue::to_string_lossy)
            .or_else(|| {
                record
                    .get(KEY_SYSTEMD_UNIT)
                    .map(FieldValue::to_string_lossy)
            })
            .or_else(|| {
                record
                    .get(KEY_SYSLOG_IDENTIFIER)
                    .map(FieldValue::to_string_lossy)
            })
            .unwrap_or(default_app);

        //Metrics::journald().add_bytes(message.len());
        Ok(Some(
            LineBuilder::new().line(message.to_string_lossy()).file(app),
        ))
    }
}

impl Decoder for JournaldExportDecoder {
    type Item = JournalRecord;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let decode_result = combine::stream::decode(
            decode_parser(),
            // PartialStream lets the parser know that more input should be
            // expected if end of input is unexpectedly reached
            &mut easy::Stream(PartialStream(&src[..])),
            &mut self.state,
        );

        let (opt, removed_len) = match decode_result {
            Ok((opt, removed_len)) => (opt, removed_len),
            Err(e) => {
                let mut range_len = 0;
                let err = e
                    .map_range(|r| {
                        range_len = r.len();
                        std::str::from_utf8(r)
                            .ok()
                            .map_or_else(|| format!("{:?}", r), |s| s.to_string())
                    })
                    .map_position(|p| p.translate_position(&src[..]));

                warn!(
                    "{}\nError parsing record: `{}`",
                    err,
                    String::from_utf8_lossy(src)
                );

                // step over error range
                src.advance(range_len);

                // Search for the start of the next record
                let mut search_state = AnyPartialState::default();
                let search_result = combine::stream::decode(
                    find_next_record(),
                    &mut easy::Stream(PartialStream(&src[..])),
                    &mut search_state,
                );

                let (_, removed_len) = search_result.map_err(|err| {
                    let err = err
                        .map_range(|r| {
                            std::str::from_utf8(r)
                                .ok()
                                .map_or_else(|| format!("{:?}", r), |s| s.to_string())
                        })
                        .map_position(|p| p.translate_position(&src[..]));
                    format!(
                        "{}\nError scanning for next record in input: `{}`",
                        err,
                        String::from_utf8_lossy(src)
                    )
                })?;

                (None, removed_len)
            }
        };

        // Advance by the accepted parse length
        src.advance(removed_len);

        match opt {
            // We did not have enough input and we require that the caller of supply more bytes
            None => Ok(None),
            Some(output) => Ok(Some(output)),
        }
    }
}

pub fn create_journalctl_source() -> Result<impl Stream<Item = LineBuilder>, std::io::Error> {
    let mut journalctl_process = tokio::process::Command::new(JOURNALCTL_CMD)
        // The current boot
        .arg("-b")
        // follow
        .arg("-f")
        // set export format
        .arg("-o")
        .arg("export")
        .stdout(Stdio::piped())
        .spawn()?;

    let decoder = JournaldExportDecoder::default();

    let journalctl_stdout = journalctl_process.stdout.take().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            "cannot get journalctl stdout hanlde",
        )
    })?;

    info!("Listening to journalctl");
    Ok(
        FramedRead::new(journalctl_stdout, decoder).filter_map(|r| async move {
            match r {
                Ok(record) => match JournaldExportDecoder::process_default_record(&record) {
                    Ok(r) => {
                        trace!("received a record from journalctl");
                        r
                    }
                    Err(e) => {
                        warn!("Encountered error in journald record: {}", e);
                        None
                    }
                },
                Err(e) => {
                    warn!("Encountered error while parsing journalctl output: {}", e);
                    None
                }
            }
        }),
    )
}

#[cfg(test)]
mod test {
    use super::JournaldExportDecoder;
    use futures::prelude::*;
    use partial_io::{PartialAsyncRead, PartialOp};
    use std::io::Cursor;
    use tokio_util::codec::FramedRead;

    #[tokio::test]
    async fn test_parse() {
        let start = br#"__CURSOR=s=6c8ff0351a1d415b955bb9fc81e5977c;i=1;b=81fa1537eb764b46a8840509591ae13f;m=1a90dea5;t=5c35036fb8c27;x=2ef1c9f471275593
__REALTIME_TIMESTAMP=1622124170808359
__MONOTONIC_TIMESTAMP=445701797
_BOOT_ID=81fa1537eb764b46a8840509591ae13f
SYSLOG_FACILITY=3
SYSLOG_IDENTIFIER=systemd-journald
_TRANSPORT=driver
PRIORITY=6
MESSAGE_ID=f77379a8490b408bbe5f6940505a777b
MESSAGE=Journal started
_PID=22
_UID=0
_GID=0
_COMM=systemd-journal
_EXE=/lib/systemd/systemd-journald
_CMDLINE=/lib/systemd/systemd-journald
_CAP_EFFECTIVE=cb
_SYSTEMD_CGROUP=/system.slice/systemd-journald.service
_SYSTEMD_UNIT=systemd-journald.service
_SYSTEMD_SLICE=system.slice
_SYSTEMD_INVOCATION_ID=8dae8ecb15d44ed097aa35fca91e3f4b
_MACHINE_ID=7827e5b17eaab5cd32a0b75d063c7569
_HOSTNAME=07770dbe2bbb

++Dodgy=record

__CURSOR=s=bcce4fb8ffcb40e9a6e05eee8b7831bf;i=5ef603;b=ec25d6795f0645619ddac9afdef453ee;m=545242e7049;t=50f1202
__REALTIME_TIMESTAMP=1423944916375353
__MONOTONIC_TIMESTAMP=5794517905481
_BOOT_ID=ec25d6795f0645619ddac9afdef453ee
_TRANSPORT=journal
_UID=1001
_GID=1001
_CAP_EFFECTIVE=0
_SYSTEMD_OWNER_UID=1001
_SYSTEMD_SLICE=user-1001.slice
_MACHINE_ID=5833158886a8445e801d437313d25eff
_HOSTNAME=bupkis
_AUDIT_LOGINUID=1001
_SELINUX_CONTEXT=unconfined_u:unconfined_r:unconfined_t:s0-s0:c0.c1023
CODE_LINE=1
CODE_FUNC=<module>
SYSLOG_IDENTIFIER=python3
_COMM=python3
_EXE=/usr/bin/python3.4
GET_MD5=False
_AUDIT_SESSION=35898
_SYSTEMD_CGROUP=/user.slice/user-1001.slice/session-35898.scope
_SYSTEMD_SESSION=35898
_SYSTEMD_UNIT=session-35898.scope
MESSAGE
"#;

        // little endian u64 encoded 7
        let size = b"\x07\x00\x00\x00\x00\x00\x00\x00";
        let rest = br#"foo
bar
CODE_FILE=<string>
_PID=16853
_CMDLINE=python3 -c from systemd import journal; journal.send("foo\nbar")
_SOURCE_REALTIME_TIMESTAMP=1423944916372858

__CURSOR=s=6c8ff0351a1d415b955bb9fc81e5977c;i=2;b=81fa1537eb764b46a8840509591ae13f;m=1a90dec3;t=5c35036fb8c44;x=a15020c965bfba83
__REALTIME_TIMESTAMP=1622124170808388
__MONOTONIC_TIMESTAMP=445701827
_BOOT_ID=81fa1537eb764b46a8840509591ae13f
SYSLOG_FACILITY=3
SYSLOG_IDENTIFIER=systemd-journald
_TRANSPORT=driver
PRIORITY=6
_PID=22
_UID=0
_GID=0
_COMM=systemd-journal
_EXE=/lib/systemd/systemd-journald
_CMDLINE=/lib/systemd/systemd-journald
_CAP_EFFECTIVE=cb
_SYSTEMD_CGROUP=/system.slice/systemd-journald.service
_SYSTEMD_UNIT=systemd-journald.service
_SYSTEMD_SLICE=system.slice
_SYSTEMD_INVOCATION_ID=8dae8ecb15d44ed097aa35fca91e3f4b
_MACHINE_ID=7827e5b17eaab5cd32a0b75d063c7569
_HOSTNAME=07770dbe2bbb
MESSAGE_ID=ec387f577b844b8fa948f33cad9a75e6
MESSAGE=Runtime journal (/run/log/journal/7827e5b17eaab5cd32a0b75d063c7569) is 8.0M, max 599.0M, 591.0M free.
JOURNAL_NAME=Runtime journal
JOURNAL_PATH=/run/log/journal/7827e5b17eaab5cd32a0b75d063c7569
CURRENT_USE=8388608
CURRENT_USE_PRETTY=8.0M
MAX_USE=628174848
MAX_USE_PRETTY=599.0M
DISK_KEEP_FREE=942264320
DISK_KEEP_FREE_PRETTY=898.6M
DISK_AVAILABLE=6273347584
DISK_AVAILABLE_PRETTY=5.8G
LIMIT=628174848
LIMIT_PRETTY=599.0M
AVAILABLE=619786240
AVAILABLE_PRETTY=591.0M

__CURSOR=s=eeadeea305f04105b991977c8881441d;i=d;b=39786bb2d22a43998a3a89727660c38e;m=3344085e79;t=5c46771a983e9;x=f92528b85724d97c
__REALTIME_TIMESTAMP=1623323451163625
__MONOTONIC_TIMESTAMP=220184731257
_BOOT_ID=39786bb2d22a43998a3a89727660c38e
SYSLOG_FACILITY=3
PRIORITY=6
_UID=0
_GID=0
_MACHINE_ID=7827e5b17eaab5cd32a0b75d063c7569
_HOSTNAME=51fa132b5549
_TRANSPORT=journal
_CAP_EFFECTIVE=a80425fb
CODE_FILE=../src/core/job.c
SYSLOG_IDENTIFIER=systemd
JOB_TYPE=start
_PID=1
_COMM=systemd
_CMDLINE=/bin/systemd --system --unit=basic.target
_SYSTEMD_CGROUP=/init.scope
_SYSTEMD_UNIT=init.scope
_SYSTEMD_SLICE=-.slice
CODE_FUNC=job_log_done_status_message
JOB_RESULT=done
MESSAGE_ID=39f53479d3a045ac8e11786248231fbf
CODE_LINE=897
MESSAGE=Condition check resulted in Commit a transient machine-id on disk being skipped.
JOB_ID=23
UNIT=systemd-machine-id-commit.service
INVOCATION_ID=
_SOURCE_REALTIME_TIMESTAMP=1623323451155218

"#;

        let mut input = Vec::new();
        input.extend_from_slice(start);
        input.extend_from_slice(size);
        input.extend_from_slice(rest);

        // Partial io ops
        let seq = vec![
            PartialOp::Limited(20),
            PartialOp::Limited(1),
            PartialOp::Limited(2),
            PartialOp::Limited(3),
            PartialOp::Limited(20),
            PartialOp::Limited(200),
            PartialOp::Limited(20),
            PartialOp::Limited(20),
            PartialOp::Limited(20),
            PartialOp::Limited(200),
            PartialOp::Limited(20),
            PartialOp::Limited(20),
            PartialOp::Limited(200),
            PartialOp::Limited(20),
            PartialOp::Limited(200),
        ];

        let reader = &mut Cursor::new(input);
        // Using the `partial_io` crate we emulate the partial reads
        let partial_reader = PartialAsyncRead::new(reader, seq);

        let decoder = JournaldExportDecoder::default();

        let result = FramedRead::new(partial_reader, decoder).try_collect().await;

        assert!(result.as_ref().is_ok(), "{}", result.unwrap_err());
        let values: Vec<_> = result.unwrap();

        assert_eq!(values.len(), 4);

        assert_eq!(
            values[0].get("MESSAGE").unwrap().to_string_lossy(),
            "Journal started".to_string()
        );
        assert_eq!(
            values[1].get("MESSAGE").unwrap().to_string_lossy(),
            "foo\nbar".to_string()
        );
        assert_eq!(
            values[3].get("INVOCATION_ID").unwrap().to_string_lossy(),
            String::new()
        );
    }

    #[cfg(feature = "libjournald")]
    #[tokio::test]
    async fn stream_gets_new_logs() {
        use super::create_journalctl_source;
        use std::time::Duration;
        use systemd::journal;
        use tokio::time::{sleep, timeout};
        let _ = env_logger::Builder::from_default_env().try_init();
        journal::print(1, "Reader got the correct line 1!");
        sleep(Duration::from_millis(50)).await;
        let mut stream = Box::pin(create_journalctl_source().unwrap());
        sleep(Duration::from_millis(50)).await;
        journal::print(1, "Reader got the correct line 2!");

        let first_line = match timeout(Duration::from_millis(500), stream.next()).await {
            Err(e) => {
                panic!("unable to grab first batch of lines from stream: {:?}", e);
            }
            Ok(None) => {
                panic!("expected to get a line from journald stream");
            }
            Ok(Some(batch)) => batch,
        };

        assert!(first_line.line.is_some());
        if let Some(line_str) = &first_line.line {
            assert_eq!(line_str, "Reader got the correct line 1!");
        }

        let second_line = match timeout(Duration::from_millis(500), stream.next()).await {
            Err(e) => {
                panic!("unable to grab second batch of lines from stream: {:?}", e);
            }
            Ok(None) => {
                panic!("expected to get a line from journald stream");
            }
            Ok(Some(batch)) => batch,
        };

        assert!(second_line.line.is_some());
        if let Some(line_str) = &second_line.line {
            assert_eq!(line_str, "Reader got the correct line 2!");
        }

        match timeout(Duration::from_millis(50), stream.next()).await {
            Err(_) => {}
            _ => panic!("did not expect any more events from journald stream"),
        }
    }
}
