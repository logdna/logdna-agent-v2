use std::fmt::{self, Display, Formatter};

#[derive(Debug)]
pub enum TailerError {
    InvalidJSON(String),
}

impl Display for TailerError {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        match self {
            TailerError::InvalidJSON(error) => {
                write!(f, "Invalid JSON: {}", error)
            }
        }
    }
}
