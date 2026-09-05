//! Pure, deterministic secret/PII detection and marker substitution.
//!
//! Every detector is a compiled regex plus a post-validation step that can
//! reject a candidate (Luhn for card numbers, ISO 7064 mod-97 for IBANs,
//! boundary checks for both) or trim it (emails). A rejected candidate does
//! not skip the span it covered: scanning resumes one character past the
//! candidate's start, so a valid secret embedded in a rejected greedy match
//! is still found. Detection is anchored to curated patterns — no entropy
//! heuristics — so results are deterministic and false positives stay rare.
//!
//! Markers use an alphabet (`[REDACTED:<kind>]`) that no detector can
//! re-match, and a position immediately adjacent to an existing marker is
//! treated as a blocked word boundary (the marker stands in for whatever
//! character enforced the boundary before it was redacted), making
//! [`Detectors::redact`] idempotent.

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

/// Every marker this crate can emit, for the marker-adjacency boundary rule.
const MARKERS: [&str; 6] = [
    "[REDACTED:email]",
    "[REDACTED:api-key]",
    "[REDACTED:jwt]",
    "[REDACTED:private-key]",
    "[REDACTED:card]",
    "[REDACTED:iban]",
];

/// Post-validation applied to a raw regex candidate. Receives the candidate
/// text and the characters surrounding its byte range (`prev`/`next`,
/// `None` at the text edges; a character adjacent to an existing redaction
/// marker is reported as a synthetic alphanumeric so boundary rules keep
/// holding after a neighbouring secret was redacted). Returns the number of
/// candidate bytes to keep — usually the full length, shorter to trim a
/// over-greedy match — or `None` to reject the candidate entirely.
type Validate = fn(candidate: &str, prev: Option<char>, next: Option<char>) -> Option<usize>;

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
/// alternation prefers it). The header-only fallback also consumes any
/// directly following base64 lines, so a block whose `END` line was
/// truncated does not leave its key material behind in cleartext — only
/// whole newline-led base64 lines are consumed, never same-line prose.
const PRIVATE_KEY_PATTERN: &str = "-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----\
    (?s:.*?)-----END [A-Z0-9 ]*PRIVATE KEY-----\
    |-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----(?:\\r?\\n[A-Za-z0-9+/=]+)*";

/// 13–24 characters of digits with optional space/dash separators, starting
/// and ending on a digit. Length and separator uniformity are enforced by
/// [`validate_card`], Luhn decides the rest.
const CARD_PATTERN: &str = "[0-9][0-9 -]{11,22}[0-9]";

/// Country code + check digits, then 11–30 more alphanumerics with optional
/// single space/dash separators — covering both the compact form and the
/// printed blocks-of-four grouping. [`validate_iban`] strips the separators
/// before the mod-97 check.
const IBAN_PATTERN: &str = "[A-Z]{2}[0-9]{2}(?:[ -]?[A-Z0-9]){11,30}";

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
            detectors.push(detector(
                PRIVATE_KEY_PATTERN,
                KIND_PRIVATE_KEY,
                validate_any,
            )?);
            detectors.push(detector(JWT_PATTERN, KIND_JWT, validate_any)?);
            detectors.push(detector(API_KEY_PATTERN, KIND_API_KEY, validate_any)?);
        }
        if config.detect_account_numbers {
            detectors.push(detector(IBAN_PATTERN, KIND_IBAN, validate_iban)?);
            detectors.push(detector(CARD_PATTERN, KIND_CARD, validate_card)?);
        }
        if config.detect_emails {
            detectors.push(detector(EMAIL_PATTERN, KIND_EMAIL, validate_email)?);
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

    /// The distinct detector kinds present in `text` (after overlap
    /// resolution) and the resolved match count. Both are safe to name in
    /// error descriptors — kinds carry nothing from the matched text.
    pub(crate) fn findings(&self, text: &str) -> (BTreeSet<&'static str>, usize) {
        let matches = self.scan(text);
        let kinds = matches.iter().map(|entry| entry.kind).collect();
        (kinds, matches.len())
    }

    /// Collect validated matches from every detector, resolve overlaps by
    /// (earliest start, then longest, then detector registration order),
    /// and return them in text order.
    ///
    /// A candidate the validator rejects only advances the search by one
    /// character past the candidate's start — never past its end — so a
    /// secret embedded inside a rejected greedy candidate (e.g. a valid
    /// card preceded by a run of reference digits) is still found.
    fn scan(&self, text: &str) -> Vec<Match> {
        let mut candidates: Vec<(usize, usize, usize, &'static str)> = Vec::new();
        for (order, detector) in self.detectors.iter().enumerate() {
            let mut position = 0_usize;
            while position <= text.len() {
                let Some(found) = detector.regex.find_at(text, position) else {
                    break;
                };
                let prev = effective_prev(text, found.start());
                let next = effective_next(text, found.end());
                match (detector.validate)(found.as_str(), prev, next) {
                    Some(keep) if keep > 0 => {
                        let end = found.start() + keep;
                        candidates.push((found.start(), end, order, detector.kind));
                        position = end;
                    }
                    _ => {
                        position =
                            found.start() + found.as_str().chars().next().map_or(1, char::len_utf8);
                    }
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

/// The character before byte `start`, with an existing redaction marker
/// reported as a synthetic alphanumeric: the marker stands in for whatever
/// character enforced a word boundary before it was redacted, so
/// boundary-sensitive validators keep rejecting on the second pass exactly
/// as they did on the first — the idempotence invariant.
fn effective_prev(text: &str, start: usize) -> Option<char> {
    let prefix = text.get(..start)?;
    if MARKERS.iter().any(|marker| prefix.ends_with(marker)) {
        return Some('A');
    }
    prefix.chars().next_back()
}

/// The character at byte `end`, with a following redaction marker reported
/// as a synthetic alphanumeric (see [`effective_prev`]).
fn effective_next(text: &str, end: usize) -> Option<char> {
    let suffix = text.get(end..)?;
    if suffix.starts_with("[REDACTED:") {
        return Some('A');
    }
    suffix.chars().next()
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

#[allow(clippy::unnecessary_wraps, reason = "Validate fn-pointer contract")]
fn validate_any(candidate: &str, _prev: Option<char>, _next: Option<char>) -> Option<usize> {
    Some(candidate.len())
}

/// Trim over-greedy email matches: a sentence-ending period with no
/// following space ("a@b.com.See the runbook") makes the domain class
/// swallow the next word as a bogus final label. Drop trailing labels that
/// start with an uppercase letter — real TLDs in prose are lowercase —
/// while the remaining domain still contains a dot.
fn validate_email(candidate: &str, _prev: Option<char>, _next: Option<char>) -> Option<usize> {
    let mut keep = candidate.len();
    loop {
        let kept = candidate.get(..keep)?;
        let at = kept.find('@')?;
        let domain = kept.get(at + 1..)?;
        let Some(dot) = domain.rfind('.') else {
            return Some(keep);
        };
        let last_label_upper = domain
            .get(dot + 1..)
            .and_then(|label| label.chars().next())
            .is_some_and(|c| c.is_ascii_uppercase());
        let rest_has_dot = domain.get(..dot).is_some_and(|rest| rest.contains('.'));
        if last_label_upper && rest_has_dot {
            keep = at + 1 + dot;
        } else {
            return Some(keep);
        }
    }
}

/// A card candidate must not extend a longer digit run, must use one uniform
/// separator (or none), must carry 13–19 digits, and must pass Luhn.
fn validate_card(candidate: &str, prev: Option<char>, next: Option<char>) -> Option<usize> {
    if prev.is_some_and(|c| c.is_ascii_digit()) || next.is_some_and(|c| c.is_ascii_digit()) {
        return None;
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
                Some(_) => return None,
            }
        }
    }
    ((13..=19).contains(&digits.len()) && luhn(&digits)).then_some(candidate.len())
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
/// check over its rearranged, letter-expanded form, ignoring the space or
/// dash separators of the printed blocks-of-four grouping.
fn validate_iban(candidate: &str, prev: Option<char>, next: Option<char>) -> Option<usize> {
    if prev.is_some_and(|c| c.is_ascii_alphanumeric())
        || next.is_some_and(|c| c.is_ascii_alphanumeric())
    {
        return None;
    }
    let compact: Vec<char> = candidate
        .chars()
        .filter(|c| *c != ' ' && *c != '-')
        .collect();
    if compact.len() < 4 {
        return None;
    }
    let (head, tail) = compact.split_at(4);
    let mut remainder = 0_u32;
    for character in tail.iter().chain(head.iter()) {
        if let Some(digit) = character.to_digit(10) {
            remainder = (remainder * 10 + digit) % 97;
        } else if character.is_ascii_uppercase() {
            let value = u32::from(*character) - u32::from('A') + 10;
            remainder = (remainder * 100 + value) % 97;
        } else {
            return None;
        }
    }
    (remainder == 1).then_some(candidate.len())
}
