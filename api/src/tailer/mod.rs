use std::process::Stdio;
use std::sync::Arc;

use bytes::{Buf, BytesMut};
use combine::parser::byte::crlf;
use combine::{
    error::{ParseError, StreamError},
    none_of,
    parser::{
        combinator::{any_partial_state, AnyPartialState},
        range::recognize,
    },
    skip_many,
    stream::{easy, PartialStream, RangeStream, StreamErrorFor},
    Parser,
};
use futures::{Stream, StreamExt};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::ChildStdin;
use tokio::sync::Mutex;
use tokio_util::codec::{Decoder, FramedRead};
use tracing::{error, info, trace, warn};
#[cfg(windows)]
use win32job::Job;

use http::types::body::LineBuilder;

use crate::tailer::error::TailerError;

mod error;

pub struct TailerApiDecoder {
    state: AnyPartialState,
    _stdin: ChildStdin,
    #[cfg(windows)]
    _job: Job,
}

type TailerRecord = String;

// The actual parser for the Tailer API format
fn decode_parser<'a, Input>(
) -> impl Parser<Input, Output = TailerRecord, PartialState = AnyPartialState> + 'a
where
    Input: RangeStream<Token = u8, Range = &'a [u8]> + 'a,
    Input::Error: ParseError<Input::Token, Input::Range, Input::Position>,
{
    /*
    Tailer API log lines are 'utf-8' encoded and separated by a single newline.
    */
    any_partial_state(
        recognize((skip_many(none_of(b"\r\n".iter().copied())), crlf())).and_then(
            |bytes: &[u8]| {
                std::str::from_utf8(bytes)
                    .map(|s| s.to_string())
                    .map_err(StreamErrorFor::<Input>::other)
            },
        ),
    )
}

// tokenizer - extract lines
fn find_next_record<'a, Input>(
) -> impl Parser<Input, Output = (), PartialState = AnyPartialState> + 'a
where
    Input: RangeStream<Token = u8, Range = &'a [u8]> + 'a,
    Input::Error: ParseError<Input::Token, Input::Range, Input::Position>,
{
    any_partial_state((skip_many(none_of(b"\r\n".iter().copied())), crlf()).map(|_| ()))
}

impl TailerApiDecoder {
    fn process_default_record(record: &TailerRecord) -> Result<Option<LineBuilder>, TailerError> {
        Ok(Some(
            LineBuilder::new().line(record.trim()).file("winevt_tailer"),
        ))
    }
}

impl Decoder for TailerApiDecoder {
    type Item = TailerRecord;
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

pub fn create_tailer_source(
    exe_path: &str,
    args: Vec<&str>,
    shutdown_tx: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
) -> Result<impl Stream<Item = LineBuilder>, std::io::Error> {
    #[cfg(windows)]
    let tailer_job = {
        let job = Job::create().unwrap();
        let mut info = job.query_extended_limit_info().unwrap();
        info.limit_kill_on_job_close();
        job.set_extended_limit_info(&mut info).unwrap();
        job
    };

    info!("Starting API Tailer: [{}] {:?}", exe_path, args);

    let exe_path_str = exe_path.to_string();
    let mut tailer_process = tokio::process::Command::new(exe_path_str.clone())
        .args(args)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;

    #[cfg(windows)]
    tailer_job
        .assign_process(tailer_process.raw_handle().unwrap())
        .expect("Failed to assign tailer process to job.");

    let tailer_stdout = tailer_process.stdout.take().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::Other, "Can't get tailer stdout handle")
    })?;

    let tailer_stderr = tailer_process.stderr.take().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::Other, "Can't get tailer stderr handle")
    })?;

    let tailer_stdin = tailer_process.stdin.take().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::Other, "Can't get tailer stdin handle")
    })?;

    tokio::spawn(async move {
        let status = tailer_process
            .wait()
            .await
            .expect("Failed to wait for tailer process");
        error!(
            "api tailer source process [{}] exited with status: {}",
            exe_path_str, status
        );
        let mut reader = BufReader::new(tailer_stderr).lines();
        while let Some(line) = reader.next_line().await.unwrap_or(None) {
            error!("api tailer source [stderr]: {}", line); // print lines from stderr to agent log
        }
        error!("Exiting agent...");
        shutdown_tx.lock().await.take().unwrap().send(()).unwrap();
    });

    let decoder = TailerApiDecoder {
        state: AnyPartialState::default(),
        _stdin: tailer_stdin,
        #[cfg(windows)]
        _job: tailer_job,
    };

    info!("Listening to API Tailer");
    Ok(
        FramedRead::new(tailer_stdout, decoder).filter_map(|r| async move {
            match r {
                Ok(record) => match TailerApiDecoder::process_default_record(&record) {
                    Ok(r) => {
                        trace!("Received a record from tailer");
                        r
                    }
                    Err(e) => {
                        warn!("Encountered error in tailer record: {}", e);
                        None
                    }
                },
                Err(e) => {
                    warn!("Encountered error while parsing tailer output: {}", e);
                    None
                }
            }
        }),
    )
}

#[cfg(test)]
mod test {
    use combine::stream::position;
    use combine::{none_of, parser::range::recognize, skip_many1, Parser};
    #[cfg(windows)]
    use futures::StreamExt;
    #[cfg(windows)]
    use std::sync::Arc;
    #[cfg(windows)]
    use tokio::sync::Mutex;
    #[cfg(windows)]
    use tracing_test::traced_test;

    #[tokio::test]
    async fn test_my_parse() {
        let _ = env_logger::Builder::from_default_env().try_init();
        let mut parser = recognize(skip_many1(none_of(b"\r\n".iter().copied())));
        let result = parser
            .parse(position::Stream::new(&b"123\r\n456\r\n789"[..]))
            .map(|(output, input)| (output, input.input));
        assert_eq!(
            "123",
            String::from_utf8_lossy(result.unwrap().0).to_string()
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    #[traced_test]
    async fn stream_gets_some_logs() {
        use super::create_tailer_source;
        use std::time::Duration;
        use tokio::time::{sleep, timeout};

        let (tailer_cmd, tailer_args) = if cfg!(windows) {
            ("cmd", vec!["/C", "echo line1 && echo line2 && pause"])
        } else {
            (
                "/usr/bin/env",
                vec!["bash", "-c", "echo line1 && echo line2 && sleep 1000"],
            )
        };

        let (shutdown_tx, _shutdown_rx) = tokio::sync::oneshot::channel();
        let shutdown_tx = Arc::new(Mutex::new(Some(shutdown_tx)));
        let mut stream =
            Box::pin(create_tailer_source(tailer_cmd, tailer_args, shutdown_tx).unwrap());

        sleep(Duration::from_millis(50)).await;

        let first_line = match timeout(Duration::from_millis(500), stream.next()).await {
            Err(e) => {
                panic!("unable to grab first batch of lines from stream: {:?}", e);
            }
            Ok(None) => {
                panic!("expected to get a line from Tailer stream");
            }
            Ok(Some(batch)) => batch,
        };
        assert!(first_line.line.is_some());
    }

    #[cfg(windows)]
    #[tokio::test]
    #[traced_test]
    async fn capture_stderr_from_tailer_on_exit() {
        use super::create_tailer_source;
        use std::time::Duration;
        use tokio::time::{sleep, timeout};

        let (tailer_cmd, tailer_args) = if cfg!(windows) {
            (
                "cmd",
                vec![
                    "/c",
                    "echo line1 && echo ===error message=== 1>&2 && waitfor /t 1 test && exit 1",
                ],
            )
        } else {
            (
                "/usr/bin/env",
                vec![
                    "bash",
                    "-c",
                    "echo line1 && echo ===error message=== 1>&2 && sleep 1 && exit 1",
                ],
            )
        };

        let (shutdown_tx, _shutdown_rx) = tokio::sync::oneshot::channel();
        let shutdown_tx = Arc::new(Mutex::new(Some(shutdown_tx)));
        let mut stream =
            Box::pin(create_tailer_source(tailer_cmd, tailer_args, shutdown_tx).unwrap());

        sleep(Duration::from_millis(3000)).await;

        match timeout(Duration::from_millis(500), stream.next()).await {
            Err(_) => {}
            Ok(None) => {}
            Ok(Some(_)) => {}
        };
        assert!(logs_contain("api::tailer: Exiting agent..."));
        assert!(logs_contain(
            "api tailer source [stderr]: ===error message==="
        ));
    }
}
