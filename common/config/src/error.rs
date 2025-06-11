use std::fmt::{Display, Formatter};
use std::{fmt, io};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    MultipleErrors(Vec<ConfigError>),
    MissingField(&'static str),
    MissingFieldOrEnvVar(&'static str, &'static str),
    Io(io::Error),
    Serde(serde_yaml::Error),
    SerdeProperties(java_properties::PropertiesError),
    PropertyInvalid(String),
    Template(http::types::error::TemplateError),
    Glob(glob::PatternError),
    Regex(types::rule::RuleError),
    NotADirectory(types::dir_path::DirPathBufError),
    Lookback(types::lookback::ParseLookbackError),
    Truncate(types::truncate::ParseTruncateError),
}

impl Display for ConfigError {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        match self {
            ConfigError::MultipleErrors(vector) => write!(f, "{:?}", vector),
            ConfigError::MissingField(field) => write!(f, "{} is a required field", field),
            ConfigError::MissingFieldOrEnvVar(field, vars) => {
                write!(
                    f,
                    "{} is missing, use command line arguments, env var ({}) or \
                    the config file to set it",
                    field, vars
                )
            }
            ConfigError::Io(e) => write!(f, "{}", e),
            ConfigError::Serde(e) => write!(f, "{}", e),
            ConfigError::SerdeProperties(e) => write!(f, "{}", e),
            ConfigError::PropertyInvalid(e) => write!(f, "{}", e),
            ConfigError::Template(e) => write!(f, "{}", e),
            ConfigError::Glob(e) => write!(f, "{}", e),
            ConfigError::Regex(e) => write!(f, "{}", e),
            ConfigError::NotADirectory(e) => write!(f, "{}", e),
            ConfigError::Lookback(e) => write!(f, "{}", e),
            ConfigError::Truncate(e) => write!(f, "{}", e),
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

impl From<glob::PatternError> for ConfigError {
    fn from(e: glob::PatternError) -> Self {
        ConfigError::Glob(e)
    }
}

impl From<types::rule::RuleError> for ConfigError {
    fn from(e: types::rule::RuleError) -> Self {
        ConfigError::Regex(e)
    }
}

impl From<types::dir_path::DirPathBufError> for ConfigError {
    fn from(e: types::dir_path::DirPathBufError) -> Self {
        ConfigError::NotADirectory(e)
    }
}

impl From<types::lookback::ParseLookbackError> for ConfigError {
    fn from(e: types::lookback::ParseLookbackError) -> Self {
        ConfigError::Lookback(e)
    }
}

impl From<types::truncate::ParseTruncateError> for ConfigError {
    fn from(e: types::truncate::ParseTruncateError) -> Self {
        ConfigError::Truncate(e)
    }
}
