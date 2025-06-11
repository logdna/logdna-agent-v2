use std::fmt;

use thiserror::Error;

#[derive(Clone, Copy, std::fmt::Debug, Eq, PartialEq, Default)]
pub enum Truncate {
    Start,
    #[default]
    SmallFiles,
    Tail,
}

#[derive(Error, Debug)]
pub enum ParseTruncateError {
    #[error("Unknown truncate strategy: {0}")]
    Unknown(String),
}

impl std::str::FromStr for Truncate {
    type Err = ParseTruncateError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s
            .to_lowercase()
            .split_whitespace()
            .collect::<String>()
            .as_str()
        {
            "start" => Ok(Truncate::Start),
            "smallfiles" => Ok(Truncate::SmallFiles),
            "tail" => Ok(Truncate::Tail),
            _ => Err(ParseTruncateError::Unknown(s.into())),
        }
    }
}

impl fmt::Display for Truncate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Truncate::Start => "start",
                Truncate::SmallFiles => "smallfiles",
                Truncate::Tail => "tail",
            }
        )
    }
}
