use std::fmt::{self, Display, Formatter};

#[allow(dead_code)] // TODO
#[derive(Debug)]
pub enum TailerError {
    InvalidJSON(String),
}

impl Display for TailerError {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        match self {
            TailerError::InvalidJSON(error) => {
                write!(f, "Invalid JSON: {error}")
            }
        }
    }
}

#[test]
fn test_root_lvl_find_valid_path() {
    let ex = TailerError::InvalidJSON("abc".into());
    assert!(ex.to_string().contains("Invalid JSON:"))
}
