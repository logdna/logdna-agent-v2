use std::fmt::{self, Display, Formatter};
use systemd::Error;

#[derive(Debug)]
pub enum JournalError {
    BadRead(Error),
    RecordMissingField(String),
}

impl Display for JournalError {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        match self {
            JournalError::BadRead(e) => write!(f, "failed to read journald {}", e),
            JournalError::RecordMissingField(field) => {
                write!(f, "missing journald field {}", field)
            }
        }
    }
}
