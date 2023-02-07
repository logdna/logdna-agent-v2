use std::process::Stdio;

use bytes::{Buf, BytesMut};
use combine::{error::{ParseError, StreamError}, none_of, parser::{
    combinator::{any_partial_state, AnyPartialState},
    range::{recognize},
}, Parser, skip_many, stream::{easy, PartialStream, RangeStream, StreamErrorFor}, token};
use futures::{Stream, StreamExt};
use log::{info, trace, warn};
use tokio::process::Child;
use tokio_util::codec::{Decoder, FramedRead};
use win32job::Job;

use http::types::body::LineBuilder;

use crate::tailer::error::TailerError;

mod error;

const TAILER_CMD: &str = "C:\\Program Files\\Mezmo\\winevt-tailer.exe";

pub struct TailerApiDecoder {
    state: AnyPartialState,
    child: Child,
    _job: Job,
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

type TailerRecord = String;

// The actual parser for the Tailer API format
fn decode_parser<'a, Input>() -> impl Parser<Input, Output=TailerRecord, PartialState=AnyPartialState> + 'a
    where
        Input: RangeStream<Token=u8, Range=&'a [u8]> + 'a,
        Input::Error: ParseError<Input::Token, Input::Range, Input::Position>,
{
    /*
    Tailer API log lines are 'etf-8' encoded and separated by a single newline.
    */
    any_partial_state(
        recognize((
            skip_many(none_of(b"\n".iter().copied())),
            // entries are line separated
            token(b'\n'),
        )).and_then(
            |bytes: &[u8]| {
                std::str::from_utf8(bytes)
                    .map(|s| s.to_string())
                    .map_err(StreamErrorFor::<Input>::other)
            },
        )
    )
}

// tokenizer - extract lines
fn find_next_record<'a, Input>() -> impl Parser<Input, Output=(), PartialState=AnyPartialState> + 'a
    where
        Input: RangeStream<Token=u8, Range=&'a [u8]> + 'a,
    // Necessary due to rust-lang/rust#24159
        Input::Error: ParseError<Input::Token, Input::Range, Input::Position>,
{
    any_partial_state(
        (
            skip_many(none_of(b"\n".iter().copied())),
            // entries are line separated
            token(b'\n'),
        )
            .map(|_| ()),
    )
}

impl TailerApiDecoder {
    fn process_default_record(
        record: &TailerRecord,
    ) -> Result<Option<LineBuilder>, TailerError> {
        eprintln!("LINE='{:?}'", record);
        Ok(Some(
            LineBuilder::new().line(record).file("winevt_tailer"),
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

pub fn create_tailer_source() -> Result<impl Stream<Item=LineBuilder>, std::io::Error> {
    let tailer_job = Job::create().unwrap();
    let mut info = tailer_job.query_extended_limit_info().unwrap();
    info.limit_kill_on_job_close();
    tailer_job.set_extended_limit_info(&mut info).unwrap();

    let tailer_process = tokio::process::Command::new(TAILER_CMD)
        .arg("-f")// follow
        .arg("-b")// lookback
        .arg("10")
        .stdout(Stdio::piped())
        .spawn()?;

    tailer_job.assign_process(tailer_process.raw_handle().unwrap()).unwrap();

    let mut decoder = TailerApiDecoder {
        state: AnyPartialState::default(),
        child: tailer_process,
        _job: tailer_job,
    };

    let tailer_stdout = decoder.child.stdout.take().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            "cannot get tailer stdout handle",
        )
    })?;

    info!("Listening to Tailer");
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

    use combine::{ none_of, parser::{
        range::{recognize},
    }, Parser, skip_many1};
    use combine::stream::position;

    #[tokio::test]
    async fn test_my_parse() {
        let mut parser = recognize(skip_many1(none_of(b"\n".iter().copied())));
        let result = parser.parse(position::Stream::new(&b"{abc\nABC"[..]))
            .map(|(output, input)| (output, input.input));
        eprintln!("DONE='{:?}'", result);
    }
}
