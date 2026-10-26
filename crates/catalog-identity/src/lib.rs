//! Identifier and digest syntax shared by every catalog document.
/// Validate a stable catalog, component or content identity.
pub fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 128
        || !id.as_bytes()[0].is_ascii_alphanumeric()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        || id == "."
        || id == ".."
    {
        return Err("identity must be a bounded, safe identifier".into());
    }
    Ok(())
}

/// Validate lowercase SHA-256 digest syntax.
pub fn validate_sha256(digest: &str) -> Result<(), String> {
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("identity requires a lowercase SHA-256 digest".into());
    }
    Ok(())
}

/// Validate the lowercase SHA-256 digest of a content identity.
pub fn validate_content_sha256(digest: &str) -> Result<(), String> {
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("content requires a lowercase SHA-256".into());
    }
    Ok(())
}
