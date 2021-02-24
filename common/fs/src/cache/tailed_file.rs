use http::types::body::LineBuilder;
use metrics::Metrics;

use futures::Stream;

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct TailedFile {
    reader: std::io::BufReader<File>,
    offset: u64,
}

impl TailedFile {
    pub(crate) fn new(path: &Path) -> Result<Self, std::io::Error> {
        Ok(Self {
            reader: BufReader::new(OpenOptions::new().read(true).open(path)?),
            offset: 0,
        })
    }

    pub(crate) fn seek(&mut self, offset: u64) -> Result<(), std::io::Error> {
        if let Err(e) = self.reader.seek(SeekFrom::Start(offset)) {
            error!("error seeking {:?}", e);
            return Err(e);
        }
        self.offset = offset;
        Ok(())
    }
    // tail a file for new line(s)
    pub(crate) fn tail(&mut self, paths: &[PathBuf]) -> Option<impl Stream<Item = LineBuilder>> {
        // get the file len
        let len = match self.reader.get_ref().metadata().map(|m| m.len()) {
            Ok(v) => v,
            Err(e) => {
                error!("unable to stat {:?}: {:?}", &paths[0], e);
                return None;
            }
        };

        // if we are at the end of the file there's no work to do
        if self.offset == len {
            return None;
        }

        // if the offset is greater than the file's len
        // it's very likely a truncation occurred
        if self.offset > len {
            info!(
                "{:?} was truncated from {} to {}",
                &paths[0], self.offset, len
            );
            // Reset offset back to the start... ish?
            // TODO: Work out the purpose of the 8192 something to do with lookback? That seems wrong.
            self.offset = if len < 8192 { 0 } else { len };
            // seek to the offset, this creates the "tailing" effect
            if let Err(e) = self.reader.get_mut().seek(SeekFrom::Start(self.offset)) {
                error!("error seeking {:?}", e);
                return None;
            }
        }

        let mut line_groups = Vec::new();

        loop {
            let mut raw_line = Vec::new();
            // read until a new line returning the line length
            let line_len = match self.reader.read_until(b'\n', &mut raw_line) {
                Ok(v) => v as u64,
                Err(e) => {
                    error!("error reading from file {:?}: {:?}", &paths[0], e);
                    break;
                }
            };
            // try to parse the raw data as utf8
            // if that fails replace invalid chars with blank chars
            // see String::from_utf8_lossy docs
            let mut line = String::from_utf8(raw_line)
                .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).to_string());
            // if the line doesn't end with a new line we might have read in the middle of a write
            // so we return in this case
            if !line.ends_with('\n') {
                Metrics::fs().increment_partial_reads();
                break;
            }
            // remove the trailing new line
            line.pop();
            // increment the offset
            self.offset += line_len;
            // send the line upstream, safe to unwrap
            debug!("tailer sendings lines for {:?}", paths);
            line_groups.extend(paths.iter().map(|path| {
                Metrics::fs().increment_lines();
                Metrics::fs().add_bytes(line_len);
                LineBuilder::new()
                    .line(line.clone())
                    .file(path.to_str().unwrap_or("").to_string())
            }));
        }

        if line_groups.is_empty() {
            None
        } else {
            Some(futures::stream::iter(line_groups))
        }
    }
}
