use std::fmt::{Display, Formatter};
use std::{fmt, io};

#[derive(Debug)]
pub enum ConfigError {
    MissingField(&'static str),
    MissingFieldOrEnvVar(&'static str, Vec<String>),
    Io(io::Error),
    Serde(serde_yaml::Error),
    Template(http::types::error::TemplateError),
    Glob(globber::Error),
    Regex(pcre2::Error),
    NotADirectory(fs::cache::DirPathBufError),
    Lookback(fs::tail::ParseLookbackError),
}

impl Display for ConfigError {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        match self {
            ConfigError::MissingField(field) => write!(f, "{} is a required field", field),
            ConfigError::MissingFieldOrEnvVar(field, vars) => {
                let vars = vars.join(",");
                write!(
                    f,
                    "{} is missing either set it in the config or set one of the env vars {} ",
                    field, vars
                )
            }
            ConfigError::Io(e) => write!(f, "{}", e),
            ConfigError::Serde(e) => write!(f, "{}", e),
            ConfigError::Template(e) => write!(f, "{}", e),
            ConfigError::Glob(e) => write!(f, "{}", e),
            ConfigError::Regex(e) => write!(f, "{}", e),
            ConfigError::NotADirectory(e) => write!(f, "{}", e),
            ConfigError::Lookback(e) => write!(f, "{}", e),
        }
    }
}

impl From<io::Error> for ConfigError {
    fn from(e: io::Error) -> Self {
        ConfigError::Io(e)
    }
}

impl From<serde_yaml::Error> for ConfigError {
    fn from(e: serde_yaml::Error) -> Self {
        ConfigError::Serde(e)
    }
}

impl From<http::types::error::TemplateError> for ConfigError {
    fn from(e: http::types::error::TemplateError) -> Self {
        ConfigError::Template(e)
    }
}

impl From<globber::Error> for ConfigError {
    fn from(e: globber::Error) -> Self {
        ConfigError::Glob(e)
    }
}

impl From<pcre2::Error> for ConfigError {
    fn from(e: pcre2::Error) -> Self {
        ConfigError::Regex(e)
    }
}

impl From<fs::cache::DirPathBufError> for ConfigError {
    fn from(e: fs::cache::DirPathBufError) -> Self {
        ConfigError::NotADirectory(e)
    }
}

impl From<fs::tail::ParseLookbackError> for ConfigError {
    fn from(e: fs::tail::ParseLookbackError) -> Self {
        ConfigError::Lookback(e)
    }
}
