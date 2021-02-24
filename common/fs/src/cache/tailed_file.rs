#![allow(clippy::await_holding_refcell_ref)]
use http::types::body::LineBuilder;
use metrics::Metrics;

use futures::io::AsyncBufRead;
use futures::task::{Context, Poll};
use futures::{ready, Stream, StreamExt};

use pin_project_lite::pin_project;

use std::cell::RefCell;
use std::convert::TryInto;
use std::fs::OpenOptions;
use std::io;
use std::mem;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::rc::Rc;
use tokio::io::{BufReader, SeekFrom};
use tokio_util::compat::{Compat, Tokio02AsyncReadCompatExt};

fn read_until_internal<R: AsyncBufRead + ?Sized>(
    mut reader: Pin<&mut R>,
    cx: &mut Context<'_>,
    byte: u8,
    buf: &mut Vec<u8>,
    read: &mut usize,
) -> Poll<io::Result<usize>> {
    loop {
        let (done, used) = {
            let available = ready!(reader.as_mut().poll_fill_buf(cx))?;
            if let Some(i) = memchr::memchr(byte, available) {
                buf.extend_from_slice(&available[..=i]);
                (true, i + 1)
            } else {
                buf.extend_from_slice(available);
                (false, available.len())
            }
        };
        reader.as_mut().consume(used);
        *read += used;
        if done || used == 0 {
            if used == 0 {
                Metrics::fs().increment_partial_reads();
            }
            return Poll::Ready(Ok(mem::replace(read, 0)));
        }
    }
}

fn read_line_lossy<R: AsyncBufRead + ?Sized>(
    reader: Pin<&mut R>,
    cx: &mut Context<'_>,
    bytes: &mut Vec<u8>,
    read: &mut usize,
) -> Poll<io::Result<(String, usize)>> {
    match ready!(read_until_internal(reader, cx, b'\n', bytes, read)) {
        Ok(count) => {
            println!("count: {}", count);
            debug_assert_eq!(*read, 0);
            let ret = String::from_utf8_lossy(bytes).to_string();
            bytes.clear();
            Poll::Ready(Ok((ret, count)))
        }
        Err(e) => Poll::Ready(Err(e)),
    }
}

pin_project! {
    #[derive(Debug)]
    #[must_use = "streams do nothing unless polled"]
    pub struct Lines {
        #[pin]
        reader: Rc<RefCell<TailedFileInner>>,
        bytes: Vec<u8>,
        read: usize,
    }
}

impl Lines {
    pub fn new(reader: Rc<RefCell<TailedFileInner>>) -> Self {
        Self {
            reader,
            bytes: Vec::new(),
            read: 0,
        }
    }
}

impl Stream for Lines {
    type Item = io::Result<String>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        let mut reader = this.reader.borrow_mut();

        println!("reader.offset before: {}", reader.offset);
        let pinned_reader = Pin::new(&mut reader.reader);
        let (mut s, n) = ready!(read_line_lossy(pinned_reader, cx, this.bytes, this.read))?;
        if n == 0 && s.is_empty() {
            return Poll::Ready(None);
        }

        let n: u64 = n.try_into().unwrap();
        if s.ends_with('\n') {
            s.pop();
            if s.ends_with('\r') {
                s.pop();
            }
        }

        reader.offset += n;

        println!("reader.offset after: {}", reader.offset);
        Metrics::fs().add_bytes(n);
        Poll::Ready(Some(Ok(s)))
    }
}

#[derive(Debug)]
pub struct TailedFileInner {
    reader: Compat<tokio::io::BufReader<tokio::fs::File>>,
    offset: u64,
}

#[derive(Debug, Clone)]
pub struct TailedFile {
    inner: Rc<RefCell<TailedFileInner>>,
}

impl TailedFile {
    pub(crate) fn new(path: &Path) -> Result<Self, std::io::Error> {
        Ok(Self {
            inner: Rc::new(RefCell::new(TailedFileInner {
                reader: BufReader::new(tokio::fs::File::from_std(
                    OpenOptions::new().read(true).open(path)?,
                ))
                .compat(),
                offset: 0,
            })),
        })
    }

    pub(crate) async fn seek(&mut self, offset: u64) -> Result<(), std::io::Error> {
        let mut inner = self.inner.borrow_mut();
        inner.offset = offset;
        inner
            .reader
            .get_mut()
            .get_mut()
            .seek(SeekFrom::Start(offset))
            .await?;
        Ok(())
    }

    // tail a file for new line(s)
    pub(crate) async fn tail(
        &mut self,
        paths: Vec<PathBuf>,
    ) -> Option<impl Stream<Item = LineBuilder>> {
        // get the file len
        {
            let mut inner = self.inner.borrow_mut();
            let len = match inner
                .reader
                .get_ref()
                .get_ref()
                .metadata()
                .await
                .map(|m| m.len())
            {
                Ok(v) => v,
                Err(e) => {
                    error!("unable to stat {:?}: {:?}", &paths[0], e);
                    return None;
                }
            };

            // if we are at the end of the file there's no work to do
            if inner.offset == len {
                return None;
            }

            // if the offset is greater than the file's len
            // it's very likely a truncation occurred
            if inner.offset > len {
                info!(
                    "{:?} was truncated from {} to {}",
                    &paths[0], inner.offset, len
                );
                // Reset offset back to the start... ish?
                // TODO: Work out the purpose of the 8192 something to do with lookback? That seems wrong.
                inner.offset = if len < 8192 { 0 } else { len };
                // seek to the offset, this creates the "tailing" effect
                let offset = inner.offset;
                if let Err(e) = inner
                    .reader
                    .get_mut()
                    .get_mut()
                    .seek(SeekFrom::Start(offset))
                    .await
                {
                    error!("error seeking {:?}", e);
                    return None;
                }
            }
        }

        Some(
            Lines::new(self.inner.clone())
                .filter_map({
                    let paths = paths.clone();
                    move |line_res| {
                        let paths = paths.clone();
                        async move {
                            let paths = paths.clone();
                            line_res.ok().map({
                                move |line| {
                                    debug!("tailer sendings lines for {:?}", paths);
                                    futures::stream::iter(paths.into_iter().map({
                                        move |path| {
                                            Metrics::fs().increment_lines();
                                            LineBuilder::new()
                                                .line(line.clone())
                                                .file(path.to_str().unwrap_or("").to_string())
                                        }
                                    }))
                                }
                            })
                        }
                    }
                })
                .flatten(),
        )
    }
}
