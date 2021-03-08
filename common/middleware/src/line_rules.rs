use crate::{Middleware, Status};
use http::types::body::LineMetaMut;
use regex::bytes::Regex as BytesRegex;
use regex::Regex as TextRegex;
use thiserror::Error;

static REDACTED_TEXT: &str = "[REDACTED]";
static REDACT_BYTES: &[u8] = &REDACTED_TEXT.as_bytes();

pub struct LineRules {
    exclusion_text: Vec<TextRegex>,
    exclusion_bytes: Vec<BytesRegex>,
    inclusion_text: Vec<TextRegex>,
    inclusion_bytes: Vec<BytesRegex>,
    redact_text: Vec<TextRegex>,
    redact_bytes: Vec<BytesRegex>,
}

#[derive(Clone, Debug, Error)]
pub enum LineRulesError {
    #[error(transparent)]
    RegexError(regex::Error),
}

impl LineRules {
    pub fn new(
        exclusion: &[String],
        inclusion: &[String],
        redact: &[String],
    ) -> Result<LineRules, LineRulesError> {
        Ok(LineRules {
            exclusion_text: map_to_regex(exclusion)?,
            inclusion_text: map_to_regex(inclusion)?,
            redact_text: map_to_regex(redact)?,
            // At this time, regex values has been validated, it's safe to unwrap()
            exclusion_bytes: exclusion
                .iter()
                .map(|s| BytesRegex::new(s).unwrap())
                .collect(),
            inclusion_bytes: inclusion
                .iter()
                .map(|s| BytesRegex::new(s).unwrap())
                .collect(),
            redact_bytes: redact.iter().map(|s| BytesRegex::new(s).unwrap()).collect(),
        })
    }

    /// Applies inclusion and exclusion rules and replaces the redacted values
    fn process_text<'a>(&self, line: &'a mut dyn LineMetaMut) -> Status<&'a mut dyn LineMetaMut> {
        let value = line.get_line().0.unwrap();

        // If it doesn't match any inclusion rule -> skip
        if !self.inclusion_text.is_empty() && !self.inclusion_text.iter().any(|r| r.is_match(value))
        {
            return Status::Skip;
        }

        // If any exclusion rule matches -> skip
        if self.exclusion_text.iter().any(|r| r.is_match(value)) {
            return Status::Skip;
        }

        if !self.redact_text.is_empty() {
            let mut v = value.to_string();
            for r in self.redact_text.iter() {
                v = r.replace_all(&v, REDACTED_TEXT).to_string();
            }

            if line.set_line_text(v).is_err() {
                return Status::Skip;
            }
        }

        Status::Ok(line)
    }

    /// Applies inclusion and exclusion rules and replaces the redacted values
    fn process_bytes<'a>(&self, line: &'a mut dyn LineMetaMut) -> Status<&'a mut dyn LineMetaMut> {
        let value = line.get_line().1.unwrap();

        // If it doesn't match any inclusion rule -> skip
        if !self.inclusion_bytes.is_empty()
            && !self.inclusion_bytes.iter().any(|r| r.is_match(value))
        {
            return Status::Skip;
        }

        // If any exclusion rule matches -> skip
        if self.exclusion_bytes.iter().any(|r| r.is_match(value)) {
            return Status::Skip;
        }

        if !self.redact_bytes.is_empty() {
            let mut v = value.to_owned();
            for r in self.redact_bytes.iter() {
                v = r.replace_all(&v, REDACT_BYTES).to_vec();
            }

            if line.set_line_buffer(v).is_err() {
                return Status::Skip;
            }
        }

        Status::Ok(line)
    }
}

impl Middleware for LineRules {
    fn run(&self) {}

    fn process<'a>(&self, line: &'a mut dyn LineMetaMut) -> Status<&'a mut dyn LineMetaMut> {
        if self.exclusion_text.is_empty()
            && self.inclusion_text.is_empty()
            && self.redact_text.is_empty()
        {
            // Avoid unnecessary allocations when no rules were defined
            return Status::Ok(line);
        }

        match line.get_line() {
            (Some(_), None) => self.process_text(line),
            (None, Some(_)) => self.process_bytes(line),
            (None, None) => Status::Skip,
            (Some(_), Some(_)) => panic!("line represented in both bytes and text"),
        }
    }
}

fn map_to_regex(rules: &[String]) -> Result<Vec<TextRegex>, LineRulesError> {
    let mut result = Vec::with_capacity(rules.len());
    // Use a normal foreach to bubble up parsing errors
    for s in rules.iter() {
        result.push(TextRegex::new(s).map_err(LineRulesError::RegexError)?);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::types::body::LineBuilder;

    macro_rules! s {
        ($val: expr) => {
            $val.to_string()
        };
    }

    macro_rules! is_match {
        ($p: ident, $line: expr, $status: pat) => {
            assert!(matches!(
                $p.process(&mut LineBuilder::new().line($line)),
                $status
            ))
        };
    }

    macro_rules! redact_match {
        ($p: ident, $line: expr, $expected: expr) => {
            match $p.process(&mut LineBuilder::new().line($line)) {
                Status::Ok(l) => {
                    assert_eq!(l.get_line().0.unwrap(), $expected);
                }
                Status::Skip => panic!("should not have been skipped"),
            }
        };
    }

    #[test]
    fn should_exclude_lines() {
        let exclusion = &vec![s!("DEBUG"), s!("(?i:TRACE)")];
        let p = LineRules::new(exclusion, &[], &[]).unwrap();
        // Not containing any excluded value
        is_match!(p, "Hello INFO something", Status::Ok(_));

        // Excluded values
        is_match!(p, "DEBUG something", Status::Skip);
        is_match!(p, "DEBUG", Status::Skip);
        // Case sensitive by default
        is_match!(p, "debug", Status::Ok(_));
        // Case-insensitive for "TRACE" matches
        is_match!(p, "a TRACE value", Status::Skip);
        is_match!(p, "a trace value", Status::Skip);
    }

    #[test]
    fn should_filter_by_inclusion() {
        // Only include lines with "WARN" (case sensitive) and "error" (case insensitive)
        let inclusion = &vec![s!("WARN"), s!("(?i:error)")];
        let p = LineRules::new(&[], inclusion, &[]).unwrap();

        // Should match
        is_match!(p, "WARN something", Status::Ok(_));
        is_match!(p, "a WARN", Status::Ok(_));
        is_match!(p, "an error", Status::Ok(_));
        is_match!(p, "ERROR", Status::Ok(_));
        is_match!(p, "Hello Error message", Status::Ok(_));

        // Should not match
        is_match!(p, "warn", Status::Skip);
        is_match!(p, "A line", Status::Skip);
        is_match!(p, "line", Status::Skip);
        is_match!(p, "", Status::Skip);
        is_match!(p, "err", Status::Skip);
    }

    #[test]
    fn should_use_exclusion_last() {
        // Only include lines with "WARN" and "error"
        // And exclude messages with the text "VERBOSE"
        let inclusion = &vec![s!("WARN"), s!("(?i:error)")];
        let exclusion = &vec![s!("VERBOSE")];
        let p = LineRules::new(exclusion, inclusion, &[]).unwrap();

        // Should match
        is_match!(p, "WARN something", Status::Ok(_));
        is_match!(p, "an error", Status::Ok(_));

        // Should not match
        is_match!(p, "VERBOSE error", Status::Skip);
        is_match!(p, "ERROR VERBOSE", Status::Skip);
        is_match!(p, "WARN VERBOSE", Status::Skip);
        is_match!(p, "A line", Status::Skip);
    }

    #[test]
    fn should_redact_lines() {
        let redact = &vec![
            s!(r"\S+@\S+\.\S+"),
            s!("(?i:SENSITIVE)"),
            s!(r"\d{1,2}-\d{1,2}-\d{4}"),
        ];
        let p = LineRules::new(&[], &[], redact).unwrap();
        redact_match!(p, "Hello INFO not redacted", "Hello INFO not redacted");
        redact_match!(p, "my sensitive information", "my [REDACTED] information");
        redact_match!(
            p,
            "my email is support@logdna.com",
            "my email is [REDACTED]"
        );
        redact_match!(
            p,
            "my date of birth is 01-02-1985",
            "my date of birth is [REDACTED]"
        );
    }

    #[test]
    fn should_apply_rules_and_redact_lines() {
        let redact = &vec![s!("(?i:SENSITIVE)")];
        let p = LineRules::new(&[s!("DEBUG")], &[s!("WARN"), s!("ERROR")], redact).unwrap();
        redact_match!(p, "Hello WARN not redacted", "Hello WARN not redacted");
        redact_match!(
            p,
            "Hello ERROR SENSITIVE information",
            "Hello ERROR [REDACTED] information"
        );
        is_match!(p, "ERROR DEBUG SENSITIVE", Status::Skip);
    }
}
