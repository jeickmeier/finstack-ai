//! Hand-rolled AWS Signature Version 4 (`SigV4`) request signing.
//!
//! All functions here are pure: no I/O, no clock reads beyond
//! [`UtcStamp::now`]. This keeps the signer testable against AWS's published
//! known-answer vectors without any network access.

use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// UTC timestamp formatted for `SigV4` (`date` = `YYYYMMDD`, `datetime` =
/// `YYYYMMDDTHHMMSSZ`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UtcStamp {
    /// `YYYYMMDD`, used in the credential scope.
    pub date: String,
    /// `YYYYMMDDTHHMMSSZ`, used as the `x-amz-date` header/query value.
    pub datetime: String,
}

impl UtcStamp {
    /// Capture the current wall-clock time.
    ///
    /// Falls back to the Unix epoch if the system clock is set before it.
    #[must_use]
    pub fn now() -> Self {
        let unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());
        Self::from_unix_secs(unix_secs)
    }

    /// Build a stamp from a Unix timestamp (seconds since epoch, UTC).
    #[must_use]
    pub fn from_unix_secs(unix_secs: u64) -> Self {
        let days = unix_secs / 86_400;
        let remainder = unix_secs % 86_400;
        let (year, month, day) = civil_from_days(i64_from_u64(days));
        let hour = remainder / 3_600;
        let minute = (remainder % 3_600) / 60;
        let second = remainder % 60;
        let date = format!("{year:04}{month:02}{day:02}");
        let datetime = format!("{date}T{hour:02}{minute:02}{second:02}Z");
        Self { date, datetime }
    }
}

fn i64_from_u64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Civil (year, month, day) from a day count since 1970-01-01, per Howard
/// Hinnant's `civil_from_days` algorithm
/// (<https://howardhinnant.github.io/date_algorithms.html>).
fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    #[expect(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        reason = "doy/mp are non-negative and bounded to [1, 31] by construction (day-of-year and month-index ranges above)"
    )]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    #[expect(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        reason = "mp is non-negative and bounded to [1, 12] by construction (month-index range above)"
    )]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day)
}

/// Credentials, region, and timestamp needed to compute a `SigV4` signature.
#[derive(Clone)]
pub struct SigningParams<'a> {
    /// AWS access key id (public identifier).
    pub access_key_id: &'a str,
    /// AWS secret access key (never logged).
    pub secret_key: &'a str,
    /// AWS region, e.g. `us-east-1`.
    pub region: &'a str,
    /// AWS service, e.g. `s3`.
    pub service: &'a str,
    /// Request timestamp.
    pub timestamp: UtcStamp,
}

impl std::fmt::Debug for SigningParams<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SigningParams")
            .field("access_key_id", &self.access_key_id)
            .field("secret_key", &"[REDACTED]")
            .field("region", &self.region)
            .field("service", &self.service)
            .field("timestamp", &self.timestamp)
            .finish()
    }
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).unwrap_or_else(|_| unreachable_hmac());
    mac.update(data);
    mac.finalize().into_bytes().into()
}

// HMAC accepts any key length; this helper avoids a `Result::unwrap` at the
// call site while keeping the `#[deny(clippy::panic)]` crate lint honest
// about why a panic here can never actually happen.
#[expect(
    clippy::panic,
    reason = "HMAC-SHA256 accepts any key length per RFC 2104; this branch is unreachable"
)]
fn unreachable_hmac() -> Hmac<Sha256> {
    panic!("HMAC-SHA256 key initialization cannot fail for any key length")
}

fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> [u8; 32] {
    let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn sha256_hex(data: &[u8]) -> String {
    to_hex(&Sha256::digest(data))
}

/// URI-encode one path or query component per RFC 3986, as required by
/// `SigV4` canonicalization. Unreserved characters (`A-Za-z0-9-_.~`) pass
/// through unescaped; `/` passes through only when `encode_slash` is false
/// (used for path components, never for query values).
#[must_use]
pub fn uri_encode(input: &str, encode_slash: bool) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b'/' if !encode_slash => out.push('/'),
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

/// One canonical, sorted, lower-cased header set derived from the fixed
/// `SigV4` headers plus caller-supplied passthrough headers.
fn canonical_header_set(
    host: &str,
    x_amz_date: &str,
    payload_sha256_hex: &str,
    extra_headers: &[(String, String)],
) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = vec![
        ("host".to_owned(), host.to_owned()),
        (
            "x-amz-content-sha256".to_owned(),
            payload_sha256_hex.to_owned(),
        ),
        ("x-amz-date".to_owned(), x_amz_date.to_owned()),
    ];
    for (name, value) in extra_headers {
        headers.push((name.to_ascii_lowercase(), value.trim().to_owned()));
    }
    headers.sort_by(|left, right| left.0.cmp(&right.0));
    headers
}

fn canonical_request(
    method: &str,
    url_path: &str,
    canonical_query: &str,
    canonical_headers: &str,
    signed_headers: &str,
    payload_hash: &str,
) -> String {
    format!(
        "{method}\n{}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}",
        uri_encode(url_path, false)
    )
}

fn string_to_sign(timestamp: &UtcStamp, credential_scope: &str, canonical_request: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256\n{}\n{credential_scope}\n{}",
        timestamp.datetime,
        sha256_hex(canonical_request.as_bytes())
    )
}

fn credential_scope(params: &SigningParams<'_>) -> String {
    format!(
        "{}/{}/{}/aws4_request",
        params.timestamp.date, params.region, params.service
    )
}

/// Sign a request, returning `authorization`, `x-amz-date`, and
/// `x-amz-content-sha256` headers followed by the passthrough
/// `extra_headers` unchanged (in the order given).
#[must_use]
pub fn sign_headers(
    params: &SigningParams<'_>,
    method: &str,
    url_path: &str,
    canonical_query: &str,
    host: &str,
    payload_sha256_hex: &str,
    extra_headers: &[(String, String)],
) -> Vec<(String, String)> {
    let headers = canonical_header_set(
        host,
        &params.timestamp.datetime,
        payload_sha256_hex,
        extra_headers,
    );
    let canonical_headers = headers
        .iter()
        .fold(String::new(), |mut acc, (name, value)| {
            let _ = writeln!(acc, "{name}:{value}");
            acc
        });
    let signed_headers = headers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(";");

    let request = canonical_request(
        method,
        url_path,
        canonical_query,
        &canonical_headers,
        &signed_headers,
        payload_sha256_hex,
    );
    let scope = credential_scope(params);
    let to_sign = string_to_sign(&params.timestamp, &scope, &request);
    let key = signing_key(
        params.secret_key,
        &params.timestamp.date,
        params.region,
        params.service,
    );
    let signature = to_hex(&hmac_sha256(&key, to_sign.as_bytes()));

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope},SignedHeaders={signed_headers},Signature={signature}",
        params.access_key_id
    );

    let mut result = vec![
        ("authorization".to_owned(), authorization),
        ("x-amz-date".to_owned(), params.timestamp.datetime.clone()),
        (
            "x-amz-content-sha256".to_owned(),
            payload_sha256_hex.to_owned(),
        ),
    ];
    result.extend(extra_headers.iter().cloned());
    result
}

#[cfg(test)]
mod tests {
    use super::{SigningParams, UtcStamp, sign_headers};

    const ACCESS_KEY_ID: &str = "AKIAIOSFODNN7EXAMPLE";
    const SECRET_KEY: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
    const EMPTY_BODY_SHA256: &str =
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn params() -> SigningParams<'static> {
        SigningParams {
            access_key_id: ACCESS_KEY_ID,
            secret_key: SECRET_KEY,
            region: "us-east-1",
            service: "s3",
            timestamp: UtcStamp::from_unix_secs(1_369_353_600),
        }
    }

    // AWS documented example: GET https://examplebucket.s3.amazonaws.com/test.txt
    // with access key AKIAIOSFODNN7EXAMPLE, secret
    // wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY, region us-east-1, service s3,
    // timestamp 20130524T000000Z, Range: bytes=0-9, empty-body payload hash
    // e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855.
    //
    // AWS's canonical request for this example (from the SigV4 documentation):
    //   GET
    //   /test.txt
    //   <empty>
    //   host:examplebucket.s3.amazonaws.com
    //   range:bytes=0-9
    //   x-amz-content-sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
    //   x-amz-date:20130524T000000Z
    //   <empty>
    //   host;range;x-amz-content-sha256;x-amz-date
    //   e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
    //
    // Expected signature:
    // f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41
    #[test]
    fn sigv4_header_signing_matches_the_aws_known_answer() {
        let extra_headers = vec![("range".to_owned(), "bytes=0-9".to_owned())];
        let headers = sign_headers(
            &params(),
            "GET",
            "/test.txt",
            "",
            "examplebucket.s3.amazonaws.com",
            EMPTY_BODY_SHA256,
            &extra_headers,
        );
        let authorization = headers
            .iter()
            .find(|(name, _)| name == "authorization")
            .map(|(_, value)| value.as_str())
            .expect("authorization header must be present");
        assert_eq!(
            authorization,
            "AWS4-HMAC-SHA256 \
             Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request,\
             SignedHeaders=host;range;x-amz-content-sha256;x-amz-date,\
             Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
        assert!(
            headers
                .iter()
                .any(|(name, value)| name == "x-amz-date" && value == "20130524T000000Z")
        );
        assert!(
            headers
                .iter()
                .any(|(name, value)| name == "x-amz-content-sha256" && value == EMPTY_BODY_SHA256)
        );
        assert!(
            headers
                .iter()
                .any(|(name, value)| name == "range" && value == "bytes=0-9")
        );
    }

    #[test]
    fn utc_stamp_formats_a_known_epoch() {
        // 1369353600 == 2013-05-24T00:00:00Z
        let stamp = UtcStamp::from_unix_secs(1_369_353_600);
        assert_eq!(stamp.date, "20130524");
        assert_eq!(stamp.datetime, "20130524T000000Z");
    }

    #[test]
    fn uri_encode_preserves_slash_only_when_requested() {
        assert_eq!(super::uri_encode("/a b/c", false), "/a%20b/c");
        assert_eq!(super::uri_encode("/a b/c", true), "%2Fa%20b%2Fc");
    }

    #[test]
    fn signing_params_debug_never_leaks_the_secret() {
        let rendered = format!("{:?}", params());
        assert!(
            rendered.contains("[REDACTED]"),
            "Debug output must redact secret_key: {rendered}"
        );
        assert!(
            !rendered.contains(SECRET_KEY),
            "Debug output must never contain the raw secret: {rendered}"
        );
    }
}
