// Session tag validation (ADR-117).
//
// Tags are free-form non-empty strings without whitespace. Validation happens
// at every API entry point (bus client messages and the HTTP API) so the
// invariant holds regardless of which surface set the tags.

use crate::error::CafeError;

/// Maximum tag length in characters. Keeps tags bounded for storage and UI.
pub const MAX_TAG_LEN: usize = 64;

/// Validate a single session tag.
///
/// A tag is valid when it is non-empty, contains no whitespace and no control
/// characters, and is at most [`MAX_TAG_LEN`] characters long.
pub fn validate_tag(tag: &str) -> Result<(), CafeError> {
    if tag.is_empty() {
        return Err(CafeError::InvalidTags("tag must not be empty".into()));
    }
    if tag.chars().any(char::is_whitespace) {
        return Err(CafeError::InvalidTags(format!(
            "tag must not contain whitespace: {tag:?}"
        )));
    }
    if tag.chars().any(char::is_control) {
        return Err(CafeError::InvalidTags(format!(
            "tag must not contain control characters: {tag:?}"
        )));
    }
    if tag.chars().count() > MAX_TAG_LEN {
        return Err(CafeError::InvalidTags(format!(
            "tag must be at most {MAX_TAG_LEN} characters: {tag:?}"
        )));
    }
    Ok(())
}

/// Validate every tag in a slice, failing on the first invalid tag.
pub fn validate_tags(tags: &[String]) -> Result<(), CafeError> {
    for tag in tags {
        validate_tag(tag)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_simple_tags() {
        for tag in ["work", "urgent", "background", "a-b_c.d", "tag123"] {
            assert!(validate_tag(tag).is_ok(), "expected {tag:?} to be valid");
        }
    }

    #[test]
    fn accepts_unicode_without_whitespace() {
        assert!(validate_tag("werkstatt").is_ok());
        assert!(validate_tag("日本語").is_ok());
    }

    #[test]
    fn rejects_empty() {
        assert!(matches!(validate_tag(""), Err(CafeError::InvalidTags(_))));
    }

    #[test]
    fn rejects_whitespace() {
        for tag in [
            "a b",
            " lead",
            "trail ",
            "tab\there",
            "new\nline",
            " " as &str,
        ] {
            assert!(
                matches!(validate_tag(tag), Err(CafeError::InvalidTags(_))),
                "expected {tag:?} to be rejected"
            );
        }
    }

    #[test]
    fn rejects_control_characters() {
        assert!(validate_tag("nul\0byte").is_err());
    }

    #[test]
    fn rejects_overlong_tags() {
        let long = "a".repeat(MAX_TAG_LEN + 1);
        assert!(validate_tag(&long).is_err());
        let at_limit = "a".repeat(MAX_TAG_LEN);
        assert!(validate_tag(&at_limit).is_ok());
    }

    #[test]
    fn validate_tags_accepts_empty_slice() {
        assert!(validate_tags(&[]).is_ok());
    }

    #[test]
    fn validate_tags_reports_first_invalid() {
        let tags = vec!["ok".to_string(), "not ok".to_string()];
        assert!(validate_tags(&tags).is_err());
    }

    // ── Property-based tests ──

    use proptest::prelude::*;

    proptest! {
        #[test]
        fn any_string_with_whitespace_is_rejected(s in ".*") {
            prop_assume!(s.chars().any(char::is_whitespace));
            prop_assert!(validate_tag(&s).is_err());
        }

        #[test]
        fn any_control_char_string_is_rejected(s in ".*") {
            prop_assume!(s.chars().any(char::is_control));
            prop_assert!(validate_tag(&s).is_err());
        }

        #[test]
        fn valid_tags_are_always_accepted(s in "[^\\s\\p{Cc}]{1,64}") {
            prop_assert!(validate_tag(&s).is_ok());
        }

        #[test]
        fn validity_is_consistent(tags in prop::collection::vec(".*", 0..10)) {
            let expected = tags.iter().all(|t| validate_tag(t).is_ok());
            prop_assert_eq!(validate_tags(&tags).is_ok(), expected);
        }
    }
}
