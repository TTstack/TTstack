//! Shared authentication utilities.

/// Validate a nonempty bearer key before constructing a client or starting a service.
/// Diagnostics deliberately exclude the supplied secret.
pub fn parse_api_key(key: &str) -> Result<String, String> {
    if key.is_empty() || !key.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("API key must be nonempty printable ASCII without whitespace".into());
    }
    Ok(key.to_owned())
}

/// Compare equal-length keys without a data-dependent early exit.
/// Key lengths are not hidden.
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_fail_closed_without_echoing_secrets() {
        for key in ["", " ", "secret\nheader", "non-ascii-密钥"] {
            let error = parse_api_key(key).unwrap_err();
            assert!(!error.contains("secret"));
        }
        assert_eq!(parse_api_key("tt-valid-key").unwrap(), "tt-valid-key");
    }
    #[test]
    fn equal_strings() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(constant_time_eq("", ""));
        assert!(constant_time_eq(
            "tt-0123456789abcdef",
            "tt-0123456789abcdef"
        ));
    }

    #[test]
    fn different_strings() {
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "ab"));
        assert!(!constant_time_eq("", "a"));
    }
}
