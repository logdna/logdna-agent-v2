use std::fmt;

use thiserror::Error;

#[derive(Clone, std::fmt::Debug, Eq, PartialEq, Default)]
pub enum Lookback {
    Start,
    SmallFiles,
    Tail,
    #[default]
    None,
}

#[derive(Error, Debug)]
pub enum ParseLookbackError {
    #[error("Unknown lookback strategy: {0}")]
    Unknown(String),
}

impl std::str::FromStr for Lookback {
    type Err = ParseLookbackError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s
            .to_lowercase()
            .split_whitespace()
            .collect::<String>()
            .as_str()
        {
            "start" => Ok(Lookback::Start),
            "smallfiles" => Ok(Lookback::SmallFiles),
            "tail" => Ok(Lookback::Tail),
            "none" => Ok(Lookback::None),
            _ => Err(ParseLookbackError::Unknown(s.into())),
        }
    }
}

impl fmt::Display for Lookback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Lookback::Start => "start",
                Lookback::SmallFiles => "smallfiles",
                Lookback::None => "none",
                Lookback::Tail => "tail",
            }
        )
    }
}
