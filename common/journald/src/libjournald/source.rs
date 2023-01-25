use crate::libjournald::stream::{Path, Stream};
use futures::stream::{select_all, SelectAll, Stream as FutureStream};
use http::types::body::LineBuilder;
use std::path::PathBuf;
use tracing::{debug, info, warn};

pub fn create_source(paths: &[PathBuf]) -> impl FutureStream<Item = LineBuilder> {
    let mut journal_files: Vec<PathBuf> = Vec::new();
    let mut journal_directories: Vec<PathBuf> = Vec::new();
    for path in paths {
        if path.is_dir() {
            journal_directories.push(path.to_path_buf());
        } else if path.is_file() {
            journal_files.push(path.to_path_buf());
        } else {
            warn!("journald path {:?} does not exist", path);
            continue;
        }

        info!("monitoring journald path {:?}", path);
    }

    debug!("initialising journald streams");
    let mut streams: Vec<Stream> = journal_directories
        .into_iter()
        .map(|dir| Stream::new(Path::Directory(dir)))
        .collect();
    if !journal_files.is_empty() {
        streams.push(Stream::new(Path::Files(journal_files)));
    }

    let combined_stream: SelectAll<<Vec<Stream> as IntoIterator>::Item> = select_all(streams);
    combined_stream
}
