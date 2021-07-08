use std::fmt::Debug;
use std::path::Path;

use core::fmt;
use glob::{Pattern, PatternError};
use pcre2::{bytes::Regex, Error as RegexError};

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
/// A trait for implementing a rule, see GlobRule/RegexRule for an example
pub trait Rule {
    /// Takes a value and returns true or false based on if it matches
    fn matches(&self, value: &Path) -> bool;
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum RuleDef {
    RegexRule(Regex),
    GlobRule(Pattern),
}

impl RuleDef {
    /// Creates a new RegexRule from a pattern
    pub fn regex_rule<'a, T: Into<&'a str>>(pattern: T) -> Result<Self, RuleError> {
        Ok(Self::RegexRule(
            Regex::new(pattern.into()).map_err(RuleError::Regex)?,
        ))
    }

    /// Creates a new GlobRule from a pattern
    pub fn glob_rule<'a, T: Into<&'a str>>(pattern: T) -> Result<Self, RuleError> {
        Ok(Self::GlobRule(
            Pattern::new(pattern.into()).map_err(RuleError::Pattern)?,
        ))
    }
}

impl Rule for RuleDef {
    fn matches(&self, value: &Path) -> bool {
        match self {
            Self::RegexRule(re) => re.is_match(value.as_os_str().as_bytes()).unwrap_or(false),
            Self::GlobRule(p) => p.matches(&value.to_string_lossy()),
        }
    }
}

/// Used for representing matches on Rules
#[derive(PartialEq)]
pub enum Status {
    /// Failed due to not being included
    NotIncluded,
    /// Was included but matched an exclusion rule therefor it did not pass
    Excluded,
    /// Passed
    Ok,
}

#[derive(std::fmt::Debug, thiserror::Error)]
pub enum RuleError {
    #[error("{0}")]
    Regex(RegexError),
    #[error("{0}")]
    Pattern(PatternError),
}

impl Status {
    /// Converts a status into a bool, returning true if the status is ok and false otherwise
    pub fn is_ok(&self) -> bool {
        matches!(self, Status::Ok)
    }
}

/// Holds both exclusion and inclusion rules
#[derive(Default, Debug, Clone)]
pub struct Rules {
    inclusion: Vec<RuleDef>,
    exclusion: Vec<RuleDef>,
}

impl Rules {
    /// Constructs an empty instance of Rules
    pub fn new() -> Self {
        Self {
            inclusion: Vec::new(),
            exclusion: Vec::new(),
        }
    }
    /// Check if value is included (matches at least one inclusion rule)
    pub fn included(&self, value: &Path) -> Status {
        for rule in &self.inclusion {
            if rule.matches(value) {
                return Status::Ok;
            }
        }
        Status::NotIncluded
    }
    /// Check if value is excluded (matches none of the exclusion rules)
    pub fn excluded(&self, value: &Path) -> Status {
        for rule in &self.exclusion {
            if rule.matches(value) {
                return Status::Excluded;
            }
        }
        Status::Ok
    }
    /// Returns true if the value is included but not excluded
    pub fn passes(&self, value: &Path) -> Status {
        if self.included(value) == Status::NotIncluded {
            return Status::NotIncluded;
        }

        self.excluded(value)
    }
    /// Adds an inclusion rule
    pub fn add_inclusion(&mut self, rule: RuleDef) {
        self.inclusion.push(rule)
    }
    /// Adds an exclusion rule
    pub fn add_exclusion(&mut self, rule: RuleDef) {
        self.exclusion.push(rule)
    }
    /// Appends all rules from another instance of rules
    pub fn add_all<T: Into<Rules>>(&mut self, rules: T) {
        let mut rules = rules.into();
        self.exclusion.append(&mut rules.exclusion);
        self.inclusion.append(&mut rules.inclusion);
    }
    /// Getter for inclusion list
    pub fn inclusion_list(&self) -> &Vec<RuleDef> {
        &self.inclusion
    }
    /// Getter for exclusion list
    pub fn exclusion_list(&self) -> &Vec<RuleDef> {
        &self.exclusion
    }
}
