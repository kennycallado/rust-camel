//! Pattern compiler and formatter for the `${date:...}` Simple sub-language.
//!
//! Supports a compile-time-validated subset of Java `SimpleDateFormat`:
//! `y`, `M`, `d`, `H`, `m`, `s`, `S`, `X` and `'literal'`. All formatting is
//! against the UTC wall clock; `X`/`XX`/`XXX` render `Z` for the zero offset.

use std::time::UNIX_EPOCH;

use camel_language_api::LanguageError;
use chrono::Datelike;
use chrono::Timelike;

pub(crate) const DEFAULT_PATTERN: &str = "yyyy-MM-dd'T'HH:mm:ss.SSSX";

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DatePattern {
    segments: Vec<DateSegment>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DateSegment {
    Year(usize),
    Month,
    Day,
    Hour,
    Minute,
    Second,
    Millis(usize),
    Offset,
    Literal(String),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DateSource {
    Now,
    Header(String),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DateOutput {
    Formatted(DatePattern),
    EpochMillis,
}

impl DatePattern {
    pub(crate) fn compile(pattern: &str) -> Result<DatePattern, LanguageError> {
        if pattern.is_empty() {
            return Err(parse_error(pattern, "empty date pattern"));
        }

        let chars: Vec<char> = pattern.chars().collect();
        let mut segments = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let current = chars[i];
            if current == '\'' {
                // A doubled quote is a standalone literal apostrophe and does
                // not open quoted mode; only a single quote starts a literal.
                if i + 1 < chars.len() && chars[i + 1] == '\'' {
                    segments.push(DateSegment::Literal("'".to_string()));
                    i += 2;
                    continue;
                }
                let (literal, next) = quoted_literal(pattern, &chars, i)?;
                segments.push(DateSegment::Literal(literal));
                i = next;
                continue;
            }
            if current.is_ascii_alphabetic() {
                let mut count = 0;
                while i + count < chars.len() && chars[i + count] == current {
                    count += 1;
                }
                segments.push(letter_segment(pattern, current, count)?);
                i += count;
                continue;
            }

            // Consecutive non-letter characters form one literal run.
            let mut literal = String::new();
            while i < chars.len() && !chars[i].is_ascii_alphabetic() && chars[i] != '\'' {
                literal.push(chars[i]);
                i += 1;
            }
            segments.push(DateSegment::Literal(literal));
        }
        Ok(DatePattern { segments })
    }
}

/// Consume one `'...'` literal starting at `start`, returning the decoded text
/// and the index after the closing quote. `''` inside the body is a literal
/// quote.
fn quoted_literal(
    pattern: &str,
    chars: &[char],
    start: usize,
) -> Result<(String, usize), LanguageError> {
    let mut literal = String::new();
    let mut i = start + 1;
    loop {
        if i >= chars.len() {
            return Err(parse_error(pattern, "unterminated quote in date pattern"));
        }
        if chars[i] == '\'' {
            if i + 1 < chars.len() && chars[i + 1] == '\'' {
                literal.push('\'');
                i += 2;
            } else {
                return Ok((literal, i + 1));
            }
        } else {
            literal.push(chars[i]);
            i += 1;
        }
    }
}

fn letter_segment(pattern: &str, letter: char, count: usize) -> Result<DateSegment, LanguageError> {
    let letters = letter.to_string().repeat(count);
    match letter {
        'y' => {
            if count > 4 {
                return Err(parse_error(
                    pattern,
                    format!("year pattern `{letters}` supports at most 4 letters"),
                ));
            }
            Ok(DateSegment::Year(count))
        }
        'M' => fixed_double_letter(pattern, letter, count, DateSegment::Month),
        'd' => fixed_double_letter(pattern, letter, count, DateSegment::Day),
        'H' => fixed_double_letter(pattern, letter, count, DateSegment::Hour),
        'm' => fixed_double_letter(pattern, letter, count, DateSegment::Minute),
        's' => fixed_double_letter(pattern, letter, count, DateSegment::Second),
        'S' => {
            if count > 3 {
                return Err(parse_error(
                    pattern,
                    format!("millisecond pattern `{letters}` supports at most 3 letters"),
                ));
            }
            Ok(DateSegment::Millis(count))
        }
        'X' => {
            if count > 3 {
                return Err(parse_error(
                    pattern,
                    format!("offset pattern `{letters}` supports at most 3 letters"),
                ));
            }
            Ok(DateSegment::Offset)
        }
        other => Err(parse_error(
            pattern,
            format!(
                "unsupported date pattern letter `{other}` \
                 (supported: y M d H m s S X and 'literal')"
            ),
        )),
    }
}

fn fixed_double_letter(
    pattern: &str,
    letter: char,
    count: usize,
    segment: DateSegment,
) -> Result<DateSegment, LanguageError> {
    if count == 2 {
        Ok(segment)
    } else {
        Err(parse_error(
            pattern,
            format!(
                "date pattern field `{}` requires exactly 2 letters",
                letter.to_string().repeat(count)
            ),
        ))
    }
}

pub(crate) fn format_epoch_ms(ms: i64, pattern: &DatePattern) -> Result<String, LanguageError> {
    let instant = chrono::DateTime::from_timestamp_millis(ms)
        .ok_or_else(|| LanguageError::EvalError("timestamp out of range".into()))?;
    let mut out = String::new();
    for segment in &pattern.segments {
        match segment {
            DateSegment::Year(count) => {
                let year = instant.year();
                if *count == 2 {
                    out.push_str(&format!("{:02}", year.rem_euclid(100)));
                } else {
                    out.push_str(&format!("{year:0width$}", width = count));
                }
            }
            DateSegment::Month => out.push_str(&format!("{:02}", instant.month())),
            DateSegment::Day => out.push_str(&format!("{:02}", instant.day())),
            DateSegment::Hour => out.push_str(&format!("{:02}", instant.hour())),
            DateSegment::Minute => out.push_str(&format!("{:02}", instant.minute())),
            DateSegment::Second => out.push_str(&format!("{:02}", instant.second())),
            DateSegment::Millis(count) => {
                out.push_str(&format!(
                    "{:0width$}",
                    instant.timestamp_subsec_millis(),
                    width = *count
                ));
            }
            DateSegment::Offset => out.push('Z'),
            DateSegment::Literal(text) => out.push_str(text),
        }
    }
    Ok(out)
}

/// Current wall-clock time as milliseconds since the UNIX epoch.
///
/// Saturates at `i64::MAX` if the elapsed time does not fit. Returns `0` if
/// the system clock is before the UNIX epoch (documented fallback); never
/// panics.
pub(crate) fn now_epoch_ms() -> i64 {
    match std::time::SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_millis()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

fn parse_error(expr: &str, reason: impl Into<String>) -> LanguageError {
    LanguageError::ParseError {
        expr: expr.to_string(),
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use camel_language_api::LanguageError;

    #[test]
    fn default_pattern_formats_fixed_instant() {
        let pattern =
            super::DatePattern::compile(super::DEFAULT_PATTERN).expect("default pattern compiles");
        assert_eq!(
            super::format_epoch_ms(1_700_000_123_456, &pattern).expect("format"),
            "2023-11-14T22:15:23.456Z"
        );
    }

    #[test]
    fn pattern_matrix_formats_fixed_instant() {
        let cases = [
            ("yyyy-MM-dd", "2023-11-14"),
            ("HH:mm:ss", "22:15:23"),
            ("yyyy-MM-dd HH:mm:ss.SSS", "2023-11-14 22:15:23.456"),
            ("yy", "23"),
            ("HH:mm:ssXXX", "22:15:23Z"),
            ("'T'HH", "T22"),
        ];
        for (pattern, expected) in cases {
            let compiled = super::DatePattern::compile(pattern).expect("pattern compiles");
            assert_eq!(
                super::format_epoch_ms(1_700_000_123_456, &compiled).expect("format"),
                expected,
                "pattern {pattern}"
            );
        }
    }

    #[test]
    fn millis_width_java_minimum() {
        for (pattern, expected) in [("S", "45"), ("SS", "45"), ("SSS", "045")] {
            let compiled = super::DatePattern::compile(pattern).expect("pattern compiles");
            assert_eq!(
                super::format_epoch_ms(1_700_000_123_045, &compiled).expect("format"),
                expected,
                "pattern {pattern}"
            );
        }
        let compiled = super::DatePattern::compile("SS").expect("pattern compiles");
        assert_eq!(
            super::format_epoch_ms(1_700_000_123_004, &compiled).expect("format"),
            "04"
        );
    }

    #[test]
    fn unsupported_letter_is_compile_error() {
        for (pattern, letter) in [("yyyy-MM-dd G", 'G'), ("z", 'z'), ("a", 'a')] {
            match super::DatePattern::compile(pattern) {
                Err(LanguageError::ParseError { reason, .. }) => {
                    assert!(
                        reason.contains("unsupported"),
                        "must say unsupported: {reason}"
                    );
                    assert!(
                        reason.contains(letter),
                        "must name the letter {letter}: {reason}"
                    );
                }
                other => panic!("pattern {pattern} must fail with ParseError, got {other:?}"),
            }
        }
    }

    #[test]
    fn unterminated_quote_is_compile_error() {
        let err =
            super::DatePattern::compile("yyyy-MM-dd'THH").expect_err("quote must be rejected");
        assert!(
            err.to_string().contains("unterminated"),
            "must mention unterminated: {err}"
        );
    }

    #[test]
    fn literal_quote_escape() {
        let pattern = super::DatePattern::compile("'T''HH'").expect("pattern compiles");
        assert_eq!(super::format_epoch_ms(0, &pattern).expect("format"), "T'HH");

        // A doubled quote outside quoted text is a standalone apostrophe.
        let pattern = super::DatePattern::compile("''HH").expect("pattern compiles");
        assert_eq!(
            super::format_epoch_ms(1_700_000_123_456, &pattern).expect("format"),
            "'22"
        );
    }

    #[test]
    fn non_ascii_renders_literally() {
        let pattern = super::DatePattern::compile("yyyy'é'dd").expect("pattern compiles");
        assert_eq!(
            super::format_epoch_ms(1_700_000_123_456, &pattern).expect("format"),
            "2023é14"
        );

        let pattern = super::DatePattern::compile("é").expect("non-ASCII literal compiles");
        assert_eq!(super::format_epoch_ms(0, &pattern).expect("format"), "é");
    }

    #[test]
    fn empty_pattern_is_compile_error() {
        assert!(matches!(
            super::DatePattern::compile(""),
            Err(LanguageError::ParseError { .. })
        ));
    }

    #[test]
    fn pre_epoch_instant_formats() {
        let pattern =
            super::DatePattern::compile("yyyy-MM-dd'T'HH:mm:ss.SSSX").expect("pattern compiles");
        assert_eq!(
            super::format_epoch_ms(-1000, &pattern).expect("format"),
            "1969-12-31T23:59:59.000Z"
        );
    }

    #[test]
    fn millis_rejects_count_over_three() {
        assert!(super::DatePattern::compile("SSSS").is_err());
    }

    #[test]
    fn two_digit_fields_reject_other_counts() {
        assert!(super::DatePattern::compile("MMM").is_err());
    }
}
