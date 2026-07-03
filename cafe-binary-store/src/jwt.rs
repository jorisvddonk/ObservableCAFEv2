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
    validation.validate_exp = true;
    // Allow both write and read tokens — caller checks purpose
    decode::<Claims>(token, &DecodingKey::from_secret(key), &validation)
        .map(|d| d.claims)
        .context("invalid JWT")
}
