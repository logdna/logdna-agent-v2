use std::fmt::{self, Display, Formatter};

#[derive(Debug)]
pub enum TailerError {
    RecordMissingField(String),
}

impl Display for TailerError {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        match self {
            TailerError::RecordMissingField(field) => {
                write!(f, "missing journald field {}", field)
            }
        }
    }
}
