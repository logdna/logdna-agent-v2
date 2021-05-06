# Common regex patterns for log redaction and exclusion

You can use `LOGDNA_REDACT_REGEX` environment variable to redact parts of the log line, replacing matches with
[REDACTED]. Additionally, you can use `LOGDNA_LINE_EXCLUSION_REGEX` variable to define entire lines to exclude.

Note that:

- It's recommended to avoid producing logs with PII.
- All regular expressions use [Perl-style syntax][regex-syntax] with case sensitivity by default. You can disable
case-sensitivity using the `i` flag in a non-capturing group, e.g., `(?i:my_case_insensitive_regex)`.
- You can use comma to separate multiple regex in an environment variable value, e.g., `(?:first),(?:second)`. If
you need to match the comma character in a pattern, use the unicode character reference: `\u002C`.

Here is a summary of regex patterns commonly used for log line redaction and exclusion to add to your agent DaemonSet
yaml file.

## Email addresses

Redact email addresses according to the RFC 5322 regex specification.

```yaml
    - env:
        - name: LOGDNA_REDACT_REGEX
          value: (?i:[a-z0-9!#$%&'*+/=?^_`{|}~-]+(?:\.[a-z0-9!#$%&'*+/=?^_`{|}~-]+)*|"(?:[\x01-\x08\x0b\x0c\x0e-\x1f\x21\x23-\x5b\x5d-\x7f]|\\[\x01-\x09\x0b\x0c\x0e-\x7f])*")@(?:(?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+[a-z0-9](?:[a-z0-9-]*[a-z0-9])?|\[(?:(?:(2(5[0-5]|[0-4][0-9])|1[0-9][0-9]|[1-9]?[0-9]))\.){3}(?:(2(5[0-5]|[0-4][0-9])|1[0-9][0-9]|[1-9]?[0-9])|[a-z0-9-]*[a-z0-9]:(?:[\x01-\x08\x0b\x0c\x0e-\x1f\x21-\x5a\x53-\x7f]|\\[\x01-\x09\x0b\x0c\x0e-\x7f])+)\])
```

## Credit card numbers

Redact Visa, MasterCard, American Express, Diners Club, Discover, and JCB credit card numbers.

```yaml
    - env:
        - name: LOGDNA_REDACT_REGEX
          value: (?:4[0-9]{12}(?:[0-9]{3})?|[25][1-7][0-9]{14}|6(?:011|5[0-9][0-9])[0-9]{12}|3[47][0-9]{13}|3(?:0[0-5]|[68][0-9])[0-9]{11}|(?:2131|1800|35\d{3})\d{11})
```

## Social Security numbers

Redact Social Security Numbers with dashes, spaces or all the digits together.

```yaml
    - env:
        - name: LOGDNA_REDACT_REGEX
          value: (?:\d{3}[- ]?\d{2}[- ]?\d{4})
```

## Class A IP Address

Redact [Class A IP addresses][ip-classes].

```yaml
    - env:
        - name: LOGDNA_REDACT_REGEX
          value: (?:(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.){3}(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)
```

## US and Canada phone numbers

Redact phone numbers as formatted in US & Canada.

```yaml
    - env:
        - name: LOGDNA_REDACT_REGEX
          value: (?:(\+(?:\d{1}|\d{2})\s)?\(?\d{3}\)?[\s.-]?\d{3}[\s.-]?\d{4})
```

## Exclude noisy messages or sensitive information

Exclude entire log lines with noisy messages or sensitive information.

```yaml
    - env:
        - name: LOGDNA_LINE_EXCLUSION_REGEX
          value: (?i:received request),(?:my sensitive info)
```

[ip-classes]: https://en.wikipedia.org/wiki/Classful_network#Introduction_of_address_classes
[regex-syntax]: https://docs.rs/regex/1.4.5/regex/#syntax
