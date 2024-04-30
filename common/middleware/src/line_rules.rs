use crate::{Middleware, MiddlewareError, Status};
use http::types::body::LineBufferMut;
use regex::bytes::{Regex, RegexSet};
use std::cmp;
use thiserror::Error;

static REDACT_BYTES: &[u8] = b"[REDACTED]";

pub struct LineRules {
    exclusion: RegexSet,
    inclusion: RegexSet,
    redact: Vec<Regex>,
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
        // Use a normal foreach to bubble up parsing errors
        let mut redact_vec = Vec::with_capacity(redact.len());
        for s in redact.iter() {
            redact_vec.push(Regex::new(s).map_err(LineRulesError::RegexError)?);
        }

        Ok(LineRules {
            exclusion: RegexSet::new(exclusion).map_err(LineRulesError::RegexError)?,
            inclusion: RegexSet::new(inclusion).map_err(LineRulesError::RegexError)?,
            redact: redact_vec,
        })
    }

    /// Applies inclusion and exclusion rules and replaces the redacted values.
    fn process_line<'a>(
        &self,
        line: &'a mut dyn LineBufferMut,
    ) -> Status<&'a mut dyn LineBufferMut> {
        let value = line.get_line_buffer().unwrap();

        // If it doesn't match any inclusion rule -> skip
        if !self.inclusion.is_empty() && !self.inclusion.is_match(value) {
            return Status::Skip;
        }

        // If any exclusion rule matches -> skip
        if self.exclusion.is_match(value) {
            return Status::Skip;
        }

        if !self.redact.is_empty() {
            return self.redact(value.to_owned(), line);
        }

        Status::Ok(line)
    }

    fn redact<'a>(
        &self,
        value: Vec<u8>,
        line: &'a mut dyn LineBufferMut,
    ) -> Status<&'a mut dyn LineBufferMut> {
        let mut matches: Vec<(usize, usize)> = vec![];
        for r in self.redact.iter() {
            for m in r.find_iter(&value) {
                let mut overlapping_match = None;
                let mut insert_index = None;
                for (i, existing) in matches.iter().enumerate() {
                    let overlaps =
                        // Overlaps when it starts between an existing match
                        (m.start() >= existing.0 && m.start() <= existing.1)
                        // or it starts before an existing match
                        // and ends after the existing match end
                        || (m.start() <= existing.0 && m.end() >= existing.0);

                    if overlaps {
                        overlapping_match = Some((
                            i,
                            cmp::min(existing.0, m.start()),
                            cmp::max(existing.1, m.end()),
                        ));
                        // Order is guaranteed so there's no need to continue processing
                        break;
                    }

                    if m.start() < existing.0 {
                        insert_index = Some(i);
                        // Order is guaranteed so there's no need to continue processing
                        break;
                    }
                }

                if let Some(item) = overlapping_match {
                    // Replace existing
                    matches[item.0] = (item.1, item.2);
                } else if let Some(index) = insert_index {
                    // Insert at position and shift all elements after it to the right
                    matches.insert(index, (m.start(), m.end()));
                } else {
                    // Append
                    matches.push((m.start(), m.end()));
                }
            }
        }

        if matches.is_empty() {
            return Status::Ok(line);
        }

        let mut redacted = Vec::with_capacity(value.len());
        let mut index = 0;
        for item in matches {
            redacted.extend_from_slice(&value[index..item.0]);
            redacted.extend_from_slice(REDACT_BYTES);
            index = item.1;
        }

        if index < value.len() {
            redacted.extend_from_slice(&value[index..]);
        }

        #[allow(clippy::question_mark)]
        if line.set_line_buffer(redacted).is_err() {
            return Status::Skip;
        }

        Status::Ok(line)
    }
}

impl Middleware for LineRules {
    fn run(&self) {}

    fn process<'a>(&self, line: &'a mut dyn LineBufferMut) -> Status<&'a mut dyn LineBufferMut> {
        if self.exclusion.is_empty() && self.inclusion.is_empty() && self.redact.is_empty() {
            // Avoid unnecessary allocations when no rules were defined
            return Status::Ok(line);
        }

        match line.get_line_buffer() {
            None => Status::Skip,
            Some(_) => self.process_line(line),
        }
    }

    fn validate<'a>(
        &self,
        line: &'a dyn LineBufferMut,
    ) -> Result<&'a dyn LineBufferMut, MiddlewareError> {
        Ok(line)
    }

    fn name(&self) -> &'static str {
        std::any::type_name::<LineRules>()
    }
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
                    assert_eq!(
                        std::str::from_utf8(l.get_line_buffer().unwrap()).unwrap(),
                        $expected
                    );
                }
                Status::Skip => panic!("should not have been skipped"),
            }
        };
    }

    #[test]
    fn should_exclude_lines() {
        let exclusion = [s!("DEBUG"), s!("(?i:TRACE)")];
        let p = LineRules::new(&exclusion, &[], &[]).unwrap();
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
        let inclusion = [s!("WARN"), s!("(?i:error)")];
        let p = LineRules::new(&[], &inclusion, &[]).unwrap();

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
        let inclusion = [s!("WARN"), s!("(?i:error)")];
        let exclusion = [s!("VERBOSE")];
        let p = LineRules::new(&exclusion, &inclusion, &[]).unwrap();

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
        let redact = [
            s!(r"\S+@\S+\.\S+"),
            s!("(?i:SENSITIVE)"),
            s!(r"\d{1,2}-\d{1,2}-\d{4}"),
        ];
        let p = LineRules::new(&[], &[], &redact).unwrap();
        redact_match!(p, "Hello INFO not redacted", "Hello INFO not redacted");
        redact_match!(p, "my sensitive information", "my [REDACTED] information");
        redact_match!(p, "Sensitive sentence", "[REDACTED] sentence");
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
    fn should_support_overlapping_redactions() {
        let redact = [
            s!(r"(?i:SI)"),
            s!(r"(?i:SUPERSENSITIVE)"),
            s!(r"(?i:SENSITIVE)"),
            s!(r"(?i:TIV)"),
            s!(r"(?i:S\w+E)"),
            s!(r"(?i:SENSITIVE information)"),
        ];
        let p = LineRules::new(&[], &[], &redact).unwrap();
        redact_match!(p, "Hello INFO not redacted", "Hello INFO not redacted");
        redact_match!(
            p,
            "my SENSITIVE information, in a sentence",
            "my [REDACTED], in a [REDACTED]"
        );
        redact_match!(
            p,
            "Si, this is sensitive, supersensitive and sensible.",
            "[REDACTED], this is [REDACTED], [REDACTED] and [REDACTED]."
        );
    }

    #[test]
    fn should_support_redactions_changing_length() {
        let redact = [s!(r"(?:123)"), s!(r"(?:def)")];
        let p = LineRules::new(&[], &[], &redact).unwrap();
        redact_match!(p, "Hello INFO not redacted", "Hello INFO not redacted");
        redact_match!(
            p,
            "count def 123 hello hello hello hello 123 def def",
            "count [REDACTED] [REDACTED] hello hello hello hello [REDACTED] [REDACTED] [REDACTED]"
        );
    }

    #[test]
    fn should_support_unordered_redactions() {
        let redact = [s!(r"(?i:AB)"), s!(r"(?i:CD)")];
        let p = LineRules::new(&[], &[], &redact).unwrap();
        redact_match!(p, "AB CD", "[REDACTED] [REDACTED]");
        redact_match!(
            p,
            "first: CD second: AB",
            "first: [REDACTED] second: [REDACTED]"
        );
        redact_match!(
            p,
            "CD 1 CD 2 AB 3 CD",
            "[REDACTED] 1 [REDACTED] 2 [REDACTED] 3 [REDACTED]"
        );
    }

    #[test]
    fn should_support_unordered_overlapping_redactions() {
        let redact = [s!(r"(?i:AB)"), s!(r"(?i:CD)"), s!(r"\w{2}"), s!(r"\w{3}")];
        let p = LineRules::new(&[], &[], &redact).unwrap();
        redact_match!(
            p,
            "CD 1 CDA 2 AB 3 CD",
            "[REDACTED] 1 [REDACTED] 2 [REDACTED] 3 [REDACTED]"
        );
    }

    #[test]
    fn should_apply_rules_and_redact_lines() {
        let redact = [s!("(?i:SENSITIVE)")];
        let p = LineRules::new(&[s!("DEBUG")], &[s!("WARN"), s!("ERROR")], &redact).unwrap();
        redact_match!(p, "Hello WARN not redacted", "Hello WARN not redacted");
        redact_match!(
            p,
            "Hello ERROR SENSITIVE information",
            "Hello ERROR [REDACTED] information"
        );
        is_match!(p, "ERROR DEBUG SENSITIVE", Status::Skip);
    }
}
