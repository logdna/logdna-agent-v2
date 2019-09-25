use std::path::PathBuf;

quick_error! {
    #[derive(Debug)]
    pub enum WatchError {
        Io(err: std::io::Error) {
            from()
            display("I/O error: {}", err)
        }
        PathNonUtf8(path: PathBuf) {
            display("{:?} was not a valid utf8 path", path)
        }
        Duplicate
    }
}
