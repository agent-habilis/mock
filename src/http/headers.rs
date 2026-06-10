use crate::error::MockerError;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub(crate) type Headers = HashMap<String, Value>;

/// Hop-by-hop headers (RFC 9110 §7.6.1) that an intermediary must not forward.
const HOP_BY_HOP_HEADERS: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Drop hop-by-hop headers — plus any header named in the `Connection` token
/// list and `content-length` — from a forwarded request/response header list.
///
/// The body is re-framed by hyper from an owned `Full<Bytes>`, so forwarding the
/// origin's `content-length` / `transfer-encoding` verbatim would produce
/// conflicting framing (and a 502 when the declared length disagrees with the
/// re-sent body). Matching is case-insensitive.
pub(crate) fn strip_hop_by_hop(headers: &[(String, String)]) -> Vec<(String, String)> {
    let mut drop_tokens: HashSet<String> = HOP_BY_HOP_HEADERS
        .iter()
        .map(|h| (*h).to_string())
        .collect();
    drop_tokens.insert("content-length".to_string());

    for (name, value) in headers {
        if name.eq_ignore_ascii_case("connection") {
            for token in value.split(',') {
                let token = token.trim().to_ascii_lowercase();
                if !token.is_empty() {
                    drop_tokens.insert(token);
                }
            }
        }
    }

    headers
        .iter()
        .filter(|(name, _)| !drop_tokens.contains(&name.to_ascii_lowercase()))
        .cloned()
        .collect()
}

/// Replace values of secret keys with `[REDACTED]`.
pub(crate) fn redact_headers(headers: &Headers, secrets: &Headers) -> Headers {
    let mut result = headers.clone();
    for key in secrets.keys() {
        if result.contains_key(key) {
            result.insert(key.clone(), Value::String("[REDACTED]".to_string()));
        }
    }
    result
}

/// Restore redacted values from the secrets map.
/// Returns `SecretNotFoundError` if a redacted key is missing from secrets.
pub(crate) fn unredact_headers(
    headers: &Headers,
    secrets: &Headers,
) -> Result<Headers, MockerError> {
    let mut result = headers.clone();
    for (key, value) in &mut result {
        if *value == Value::String("[REDACTED]".to_string()) {
            if let Some(secret_value) = secrets.get(key) {
                *value = secret_value.clone();
            } else {
                return Err(MockerError::SecretNotFoundError { key: key.clone() });
            }
        }
    }
    Ok(result)
}

/// Sanitize request headers: redact secrets and remove content-length.
pub(crate) fn sanitize_request_headers(headers: &Headers, secrets: &Headers) -> Headers {
    let mut result = redact_headers(headers, secrets);
    result.remove("content-length");
    result
}

/// Sanitize response headers: redact secrets, remove content-encoding and content-length.
pub(crate) fn sanitize_response_headers(headers: &Headers, secrets: &Headers) -> Headers {
    let mut result = redact_headers(headers, secrets);
    result.remove("content-encoding");
    result.remove("content-length");
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(crate::proptest_support::config())]
        /// redact then unredact with the same secrets restores the originals.
        #[test]
        fn prop_redact_unredact_round_trips(
            pairs in prop::collection::vec(("[a-z][a-z0-9-]{0,15}", "[a-zA-Z0-9]{0,30}"), 0..8),
        ) {
            let headers: Headers = pairs
                .into_iter()
                .map(|(name, value)| (name, Value::String(value)))
                .collect();
            let secrets = headers.clone();
            let redacted = redact_headers(&headers, &secrets);
            let restored = unredact_headers(&redacted, &secrets).unwrap();
            prop_assert_eq!(restored, headers);
        }

        /// strip_hop_by_hop never keeps a hop-by-hop header or content-length.
        #[test]
        fn prop_strip_hop_by_hop_drops_all(
            pairs in prop::collection::vec(("[a-zA-Z][a-zA-Z0-9-]{0,20}", "[ -~]{0,20}"), 0..12),
        ) {
            for (name, _) in strip_hop_by_hop(&pairs) {
                let lower = name.to_ascii_lowercase();
                prop_assert!(!HOP_BY_HOP_HEADERS.contains(&lower.as_str()));
                prop_assert_ne!(lower, "content-length");
            }
        }
    }

    fn make_headers(pairs: &[(&str, &str)]) -> Headers {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
            .collect()
    }

    #[test]
    fn test_strip_hop_by_hop() {
        let headers = vec![
            ("connection".to_string(), "keep-alive, x-custom".to_string()),
            ("keep-alive".to_string(), "timeout=5".to_string()),
            ("transfer-encoding".to_string(), "chunked".to_string()),
            ("content-length".to_string(), "10".to_string()),
            ("x-custom".to_string(), "drop-me".to_string()),
            ("content-type".to_string(), "text/plain".to_string()),
            ("content-encoding".to_string(), "gzip".to_string()),
        ];
        let kept = strip_hop_by_hop(&headers);
        let names: Vec<&str> = kept.iter().map(|(name, _)| name.as_str()).collect();
        // hop-by-hop, content-length, and the Connection-listed `x-custom` are
        // dropped; content-encoding is preserved (the mock-write path needs it).
        assert_eq!(names, vec!["content-type", "content-encoding"]);
    }

    #[test]
    fn test_redact_headers() {
        let headers = make_headers(&[
            ("authorization", "Bearer token123"),
            ("accept", "text/html"),
        ]);
        let secrets = make_headers(&[("authorization", "Bearer token123")]);
        let result = redact_headers(&headers, &secrets);
        assert_eq!(
            result["authorization"],
            Value::String("[REDACTED]".to_string())
        );
        assert_eq!(result["accept"], Value::String("text/html".to_string()));
    }

    #[test]
    fn test_redact_headers_no_match() {
        let headers = make_headers(&[("accept", "text/html")]);
        let secrets = make_headers(&[("authorization", "Bearer token123")]);
        let result = redact_headers(&headers, &secrets);
        assert_eq!(result.len(), 1);
        assert_eq!(result["accept"], Value::String("text/html".to_string()));
    }

    #[test]
    fn test_redact_headers_empty_secrets() {
        let headers = make_headers(&[("accept", "text/html")]);
        let secrets = Headers::new();
        let result = redact_headers(&headers, &secrets);
        assert_eq!(result, headers);
    }

    #[test]
    fn test_unredact_headers() {
        let headers = make_headers(&[("authorization", "[REDACTED]"), ("accept", "text/html")]);
        let secrets = make_headers(&[("authorization", "Bearer token123")]);
        let result = unredact_headers(&headers, &secrets).unwrap();
        assert_eq!(
            result["authorization"],
            Value::String("Bearer token123".to_string())
        );
        assert_eq!(result["accept"], Value::String("text/html".to_string()));
    }

    #[test]
    fn test_unredact_headers_missing_secret() {
        let headers = make_headers(&[("authorization", "[REDACTED]")]);
        let secrets = Headers::new();
        let result = unredact_headers(&headers, &secrets);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            MockerError::SecretNotFoundError { .. }
        ));
    }

    #[test]
    fn test_sanitize_request_headers() {
        let headers = make_headers(&[
            ("authorization", "Bearer token"),
            ("content-length", "42"),
            ("accept", "text/html"),
        ]);
        let secrets = make_headers(&[("authorization", "Bearer token")]);
        let result = sanitize_request_headers(&headers, &secrets);
        assert_eq!(
            result["authorization"],
            Value::String("[REDACTED]".to_string())
        );
        assert!(!result.contains_key("content-length"));
        assert_eq!(result["accept"], Value::String("text/html".to_string()));
    }

    #[test]
    fn test_sanitize_response_headers() {
        let headers = make_headers(&[
            ("authorization", "Bearer token"),
            ("content-length", "42"),
            ("content-encoding", "gzip"),
            ("x-custom", "value"),
        ]);
        let secrets = make_headers(&[("authorization", "Bearer token")]);
        let result = sanitize_response_headers(&headers, &secrets);
        assert_eq!(
            result["authorization"],
            Value::String("[REDACTED]".to_string())
        );
        assert!(!result.contains_key("content-length"));
        assert!(!result.contains_key("content-encoding"));
        assert_eq!(result["x-custom"], Value::String("value".to_string()));
    }

    #[test]
    fn test_roundtrip_redact_unredact() {
        let headers = make_headers(&[
            ("authorization", "Bearer secret"),
            ("x-api-key", "key123"),
            ("accept", "application/json"),
        ]);
        let secrets = make_headers(&[("authorization", "Bearer secret"), ("x-api-key", "key123")]);
        let redacted = redact_headers(&headers, &secrets);
        assert_eq!(
            redacted["authorization"],
            Value::String("[REDACTED]".to_string())
        );
        assert_eq!(
            redacted["x-api-key"],
            Value::String("[REDACTED]".to_string())
        );
        let unredacted = unredact_headers(&redacted, &secrets).unwrap();
        assert_eq!(unredacted, headers);
    }
}
