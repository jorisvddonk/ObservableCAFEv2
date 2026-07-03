use anyhow::{Context, Result};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub chunk_id: String,
    pub purpose: String,
    pub iat: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp: Option<u64>,
}

/// Load or generate the JWT signing key.
pub fn load_key(data_dir: &Path) -> Result<Vec<u8>> {
    let key_path = data_dir.join("cafe-binary-store.key");
    if key_path.exists() {
        Ok(std::fs::read(&key_path).context("failed to read JWT key file")?)
    } else {
        std::fs::create_dir_all(data_dir).context("failed to create data dir")?;
        let key: Vec<u8> = (0..64).map(|_| rand::random::<u8>()).collect();
        std::fs::write(&key_path, &key).context("failed to write JWT key file")?;
        Ok(key)
    }
}

pub fn sign_write(chunk_id: &str, ttl_secs: u64, key: &[u8]) -> Result<String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let claims = Claims {
        chunk_id: chunk_id.to_string(),
        purpose: "write".into(),
        iat: now,
        exp: Some(now + ttl_secs),
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(key),
    )
    .context("failed to sign write JWT")
}

pub fn sign_read(chunk_id: &str, key: &[u8]) -> Result<String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let claims = Claims {
        chunk_id: chunk_id.to_string(),
        purpose: "read".into(),
        iat: now,
        exp: None,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(key),
    )
    .context("failed to sign read JWT")
}

pub fn verify(token: &str, key: &[u8]) -> Result<Claims> {
    let mut validation = Validation::default();
    validation.validate_exp = false;
    validation.leeway = 0;
    // Remove required claims that read tokens don't have
    validation.required_spec_claims.clear();
    let claims = decode::<Claims>(token, &DecodingKey::from_secret(key), &validation)
        .map(|d| d.claims)
        .context("invalid JWT")?;
    // If the token has an exp claim, check it (read tokens skip this)
    if let Some(exp) = claims.exp {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now >= exp {
            anyhow::bail!("token expired");
        }
    }
    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> Vec<u8> {
        b"this-is-a-32-byte-test-key-for-jwt!!".to_vec()
    }

    #[test]
    fn sign_and_verify_write_token() {
        let key = test_key();
        let token = sign_write("chunk-abc", 3600, &key).unwrap();
        let claims = verify(&token, &key).unwrap();
        assert_eq!(claims.chunk_id, "chunk-abc");
        assert_eq!(claims.purpose, "write");
        assert!(claims.exp.is_some());
    }

    #[test]
    fn sign_and_verify_read_token() {
        let key = test_key();
        let token = sign_read("chunk-abc", &key).unwrap();
        let claims = verify(&token, &key).unwrap();
        assert_eq!(claims.chunk_id, "chunk-abc");
        assert_eq!(claims.purpose, "read");
        assert!(claims.exp.is_none());
    }

    #[test]
    fn expired_token_fails() {
        let key = test_key();
        // Sign with 0-second TTL — should be expired immediately
        let token = sign_write("chunk-abc", 0, &key).unwrap();
        // Sleep 1ms to ensure expiry
        std::thread::sleep(std::time::Duration::from_millis(1));
        let result = verify(&token, &key);
        assert!(result.is_err());
    }

    #[test]
    fn wrong_key_fails() {
        let key = test_key();
        let wrong = b"a-different-32-byte-key-for-testing!!".to_vec();
        let token = sign_write("chunk-abc", 3600, &key).unwrap();
        let result = verify(&token, &wrong);
        assert!(result.is_err());
    }

    #[test]
    fn different_chunk_id_rejected() {
        let key = test_key();
        let token = sign_write("chunk-abc", 3600, &key).unwrap();
        let claims = verify(&token, &key).unwrap();
        assert_ne!(claims.chunk_id, "chunk-xyz");
    }

    #[test]
    fn write_ttl_matches_exp() {
        let key = test_key();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let ttl = 3600u64;
        let token = sign_write("chunk-abc", ttl, &key).unwrap();
        let claims = verify(&token, &key).unwrap();
        let exp = claims.exp.unwrap();
        assert!(exp > now);
        assert!(exp <= now + ttl + 1); // allow 1s clock skew
    }
}
