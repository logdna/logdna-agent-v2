use std::fmt::{self, Display, Formatter};

#[derive(Debug)]
pub enum JournalCtlError {
    RecordMissingField(String),
}

impl Display for JournalCtlError {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        match self {
            JournalCtlError::RecordMissingField(field) => {
                write!(f, "missing journald field {field}")
            }
        }
    }
}
