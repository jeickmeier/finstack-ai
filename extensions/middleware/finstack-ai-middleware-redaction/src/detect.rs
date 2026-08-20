//! Pure, deterministic secret/PII detection and marker substitution.
//!
//! Every detector is a compiled regex plus an optional post-validation step
//! (Luhn for card numbers, ISO 7064 mod-97 for IBANs, digit-boundary checks
//! for both). Detection is anchored to curated patterns — no entropy
//! heuristics — so results are deterministic and false positives stay rare.
//! Markers use an alphabet (`[REDACTED:<kind>]`) that no detector can
//! re-match, making [`Detectors::redact`] idempotent.

use std::collections::BTreeSet;

use regex::Regex;

use crate::{RedactionConfig, RedactionError};

/// Marker kind for email addresses.
const KIND_EMAIL: &str = "email";
/// Marker kind for vendor API keys and tokens.
const KIND_API_KEY: &str = "api-key";
/// Marker kind for JSON Web Tokens.
const KIND_JWT: &str = "jwt";
/// Marker kind for PEM private-key blocks.
const KIND_PRIVATE_KEY: &str = "private-key";
/// Marker kind for Luhn-valid payment-card numbers.
const KIND_CARD: &str = "card";
/// Marker kind for mod-97-valid IBANs.
const KIND_IBAN: &str = "iban";

/// Post-validation applied to a raw regex candidate before it becomes a
/// match. Receives the candidate text and its byte range's surrounding
/// characters (`prev`/`next`, `None` at the text edges).
type Validate = fn(candidate: &str, prev: Option<char>, next: Option<char>) -> bool;

struct Detector {
    regex: Regex,
    kind: &'static str,
    validate: Validate,
}

/// One validated, resolved match.
struct Match {
    start: usize,
    end: usize,
    kind: &'static str,
}

/// Compiled detector set. Built once at middleware construction; running a
/// detector never fails after that.
pub(crate) struct Detectors {
    detectors: Vec<Detector>,
}

impl std::fmt::Debug for Detectors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Detectors")
            .field("count", &self.detectors.len())
            .finish()
    }
}

const EMAIL_PATTERN: &str = "[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\\.[A-Za-z]{2,}";

/// Known vendor key/token prefixes, one alternation: OpenAI/Anthropic `sk-`,
/// GitHub classic + fine-grained, Slack, AWS access-key ids, Google `AIza`,
/// Stripe live/test keys.
const API_KEY_PATTERN: &str = "sk-[A-Za-z0-9_-]{16,}\
    |gh[pousr]_[A-Za-z0-9]{36,}\
    |github_pat_[A-Za-z0-9_]{22,}\
    |xox[abprs]-[A-Za-z0-9-]{10,}\
    |(?:AKIA|ASIA|ABIA|ACCA)[A-Z0-9]{16,}\
    |AIza[A-Za-z0-9_-]{35,}\
    |(?:sk|pk|rk)_(?:live|test)_[A-Za-z0-9]{10,}";

const JWT_PATTERN: &str = "eyJ[A-Za-z0-9_-]{4,}\\.eyJ[A-Za-z0-9_-]{4,}\\.[A-Za-z0-9_-]{4,}";

/// A whole PEM block when the end marker is present (leftmost-first
/// alternation prefers it), else the header alone.
const PRIVATE_KEY_PATTERN: &str = "-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----\
    (?s:.*?)-----END [A-Z0-9 ]*PRIVATE KEY-----\
    |-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----";

/// 13–24 characters of digits with optional space/dash separators, starting
/// and ending on a digit. Length and separator uniformity are enforced by
/// [`validate_card`], Luhn decides the rest.
const CARD_PATTERN: &str = "[0-9][0-9 -]{11,22}[0-9]";

const IBAN_PATTERN: &str = "[A-Z]{2}[0-9]{2}[A-Z0-9]{11,30}";

impl Detectors {
    /// Compile the detectors enabled by `config`.
    ///
    /// # Errors
    ///
    /// Rejects an all-disabled detector set or (unreachably for the
    /// checked-in patterns) a pattern that fails to compile.
    pub(crate) fn try_new(config: RedactionConfig) -> Result<Self, RedactionError> {
        if !config.detect_emails && !config.detect_api_keys && !config.detect_account_numbers {
            return Err(RedactionError::Configuration {
                reason: "all_detectors_disabled",
            });
        }
        let mut detectors = Vec::new();
        if config.detect_api_keys {
            // Longer, more specific structures first so equal-start overlaps
            // resolve to the most specific kind.
            detectors.push(detector(PRIVATE_KEY_PATTERN, KIND_PRIVATE_KEY, validate_any)?);
            detectors.push(detector(JWT_PATTERN, KIND_JWT, validate_any)?);
            detectors.push(detector(API_KEY_PATTERN, KIND_API_KEY, validate_any)?);
        }
        if config.detect_account_numbers {
            detectors.push(detector(IBAN_PATTERN, KIND_IBAN, validate_iban)?);
            detectors.push(detector(CARD_PATTERN, KIND_CARD, validate_card)?);
        }
        if config.detect_emails {
            detectors.push(detector(EMAIL_PATTERN, KIND_EMAIL, validate_any)?);
        }
        Ok(Self { detectors })
    }

    /// Replace every detected span with its `[REDACTED:<kind>]` marker.
    ///
    /// Returns `None` when nothing matched, so callers can distinguish
    /// "unchanged" without comparing strings.
    pub(crate) fn redact(&self, text: &str) -> Option<String> {
        let matches = self.scan(text);
        if matches.is_empty() {
            return None;
        }
        let mut output = String::with_capacity(text.len());
        let mut cursor = 0_usize;
        for entry in &matches {
            output.push_str(text.get(cursor..entry.start).unwrap_or_default());
            output.push_str("[REDACTED:");
            output.push_str(entry.kind);
            output.push(']');
            cursor = entry.end;
        }
        output.push_str(text.get(cursor..).unwrap_or_default());
        Some(output)
    }

    /// The distinct detector kinds present in `text`, after overlap
    /// resolution. Safe to name in error descriptors — kinds carry nothing
    /// from the matched text.
    pub(crate) fn kinds_in(&self, text: &str) -> BTreeSet<&'static str> {
        self.scan(text).iter().map(|entry| entry.kind).collect()
    }

    /// Collect validated matches from every detector, resolve overlaps by
    /// (earliest start, then longest, then detector registration order),
    /// and return them in text order.
    fn scan(&self, text: &str) -> Vec<Match> {
        let mut candidates: Vec<(usize, usize, usize, &'static str)> = Vec::new();
        for (order, detector) in self.detectors.iter().enumerate() {
            for found in detector.regex.find_iter(text) {
                let prev = text.get(..found.start()).and_then(|s| s.chars().next_back());
                let next = text.get(found.end()..).and_then(|s| s.chars().next());
                if (detector.validate)(found.as_str(), prev, next) {
                    candidates.push((found.start(), found.end(), order, detector.kind));
                }
            }
        }
        candidates.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| a.2.cmp(&b.2))
        });
        let mut resolved: Vec<Match> = Vec::with_capacity(candidates.len());
        let mut last_end = 0_usize;
        for (start, end, _, kind) in candidates {
            if start >= last_end {
                resolved.push(Match { start, end, kind });
                last_end = end;
            }
        }
        resolved
    }
}

fn detector(
    pattern: &str,
    kind: &'static str,
    validate: Validate,
) -> Result<Detector, RedactionError> {
    Ok(Detector {
        regex: Regex::new(pattern).map_err(|_| RedactionError::Configuration {
            reason: "detector_pattern_invalid",
        })?,
        kind,
        validate,
    })
}

fn validate_any(_candidate: &str, _prev: Option<char>, _next: Option<char>) -> bool {
    true
}

/// A card candidate must not extend a longer digit run, must use one uniform
/// separator (or none), must carry 13–19 digits, and must pass Luhn.
fn validate_card(candidate: &str, prev: Option<char>, next: Option<char>) -> bool {
    if prev.is_some_and(|c| c.is_ascii_digit()) || next.is_some_and(|c| c.is_ascii_digit()) {
        return false;
    }
    let mut separator: Option<char> = None;
    let mut digits: Vec<u8> = Vec::with_capacity(candidate.len());
    for character in candidate.chars() {
        if let Some(digit) = character.to_digit(10) {
            digits.push(u8::try_from(digit).unwrap_or_default());
        } else {
            match separator {
                None => separator = Some(character),
                Some(existing) if existing == character => {}
                Some(_) => return false,
            }
        }
    }
    (13..=19).contains(&digits.len()) && luhn(&digits)
}

fn luhn(digits: &[u8]) -> bool {
    let mut sum = 0_u32;
    for (index, digit) in digits.iter().rev().enumerate() {
        let mut value = u32::from(*digit);
        if index % 2 == 1 {
            value *= 2;
            if value > 9 {
                value -= 9;
            }
        }
        sum += value;
    }
    sum.is_multiple_of(10)
}

/// An IBAN candidate must be word-isolated and pass the ISO 7064 mod-97
/// check over its rearranged, letter-expanded form.
fn validate_iban(candidate: &str, prev: Option<char>, next: Option<char>) -> bool {
    if prev.is_some_and(|c| c.is_ascii_alphanumeric())
        || next.is_some_and(|c| c.is_ascii_alphanumeric())
    {
        return false;
    }
    let Some((head, tail)) = candidate
        .char_indices()
        .nth(4)
        .map(|(index, _)| candidate.split_at(index))
    else {
        return false;
    };
    let mut remainder = 0_u32;
    for character in tail.chars().chain(head.chars()) {
        if let Some(digit) = character.to_digit(10) {
            remainder = (remainder * 10 + digit) % 97;
        } else if character.is_ascii_uppercase() {
            let value = u32::from(character) - u32::from('A') + 10;
            remainder = (remainder * 100 + value) % 97;
        } else {
            return false;
        }
    }
    remainder == 1
}
