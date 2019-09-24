use std::{fmt, io};
use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum ConfigError {
    MissingField(&'static str),
    MissingEnvVar(Vec<String>),
    Io(io::Error),
    Serde(serde_yaml::Error),
    Template(http::types::error::TemplateError),
    Glob(globber::Error),
    Regex(regex::Error),
}

impl Display for ConfigError {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        match self {
            ConfigError::MissingField(field) => write!(f, "{} is a required field", field),
            ConfigError::MissingEnvVar(vars) => {
                let vars = vars.join(" or ");
                write!(f, "one of {} needs to be set ", vars)
            },
            ConfigError::Io(e) => write!(f, "{}", e),
            ConfigError::Serde(e) => write!(f, "{}", e),
            ConfigError::Template(e) => write!(f, "{}", e),
            ConfigError::Glob(e) => write!(f, "{}", e),
            ConfigError::Regex(e) => write!(f, "{}", e),
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

impl From<regex::Error> for ConfigError {
    fn from(e: regex::Error) -> Self {
        ConfigError::Regex(e)
    }
}