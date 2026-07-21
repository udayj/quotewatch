use crate::integrations::gmail::StoredToken;
use base64::Engine;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use thiserror::Error;

const TOKEN_KEY_ENV: &str = "OAUTH_TOKEN_ENCRYPTION_KEY";

#[derive(Debug, Error)]
pub enum TokenStoreError {
    #[error("invalid {TOKEN_KEY_ENV}: expected base64-encoded 32-byte key")]
    InvalidKey,

    #[error("failed to serialize OAuth token: {0}")]
    Serialize(#[from] serde_json::Error),

    #[error("failed to decode encrypted OAuth token field: {0}")]
    Base64(#[from] base64::DecodeError),

    #[error("invalid encrypted OAuth token nonce")]
    InvalidNonce,

    #[error("failed to encrypt OAuth token")]
    Encrypt,

    #[error("failed to decrypt OAuth token")]
    Decrypt,
}

pub struct EncryptedToken {
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Clone)]
pub struct TokenCipher {
    cipher: ChaCha20Poly1305,
}

impl TokenCipher {
    pub fn from_env() -> Result<Option<Self>, TokenStoreError> {
        let Ok(raw) = std::env::var(TOKEN_KEY_ENV) else {
            return Ok(None);
        };
        let raw = raw.trim();
        if raw.is_empty() {
            return Ok(None);
        }
        Self::from_base64(raw).map(Some)
    }

    fn from_base64(raw: &str) -> Result<Self, TokenStoreError> {
        let key = base64::engine::general_purpose::STANDARD.decode(raw)?;
        let key: [u8; 32] = key.try_into().map_err(|_| TokenStoreError::InvalidKey)?;
        Ok(Self {
            cipher: ChaCha20Poly1305::new(&key.into()),
        })
    }

    pub fn encrypt_token(&self, token: &StoredToken) -> Result<EncryptedToken, TokenStoreError> {
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        // Storage flow:
        // StoredToken struct -> JSON bytes -> encrypted bytes -> base64 text in DB.
        // Reading reverses that:
        // base64 text from DB -> encrypted bytes -> JSON bytes -> StoredToken struct.
        // Base64 is only an encoding layer so arbitrary encrypted bytes can live
        // safely in text columns and environment-style values.
        let raw = serde_json::to_vec(token)?;
        let ciphertext = self
            .cipher
            .encrypt(&nonce, raw.as_slice())
            .map_err(|_| TokenStoreError::Encrypt)?;
        Ok(EncryptedToken {
            nonce: base64::engine::general_purpose::STANDARD.encode(nonce),
            ciphertext: base64::engine::general_purpose::STANDARD.encode(ciphertext),
        })
    }

    pub fn decrypt_token(
        &self,
        nonce: &str,
        ciphertext: &str,
    ) -> Result<StoredToken, TokenStoreError> {
        let nonce = base64::engine::general_purpose::STANDARD.decode(nonce)?;
        let nonce: [u8; 12] = nonce
            .try_into()
            .map_err(|_| TokenStoreError::InvalidNonce)?;
        let ciphertext = base64::engine::general_purpose::STANDARD.decode(ciphertext)?;
        let raw = self
            .cipher
            .decrypt(Nonce::from_slice(&nonce), ciphertext.as_slice())
            .map_err(|_| TokenStoreError::Decrypt)?;
        serde_json::from_slice(&raw).map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_cipher_roundtrips_token_json() {
        let key = base64::engine::general_purpose::STANDARD.encode([7_u8; 32]);
        let cipher = TokenCipher::from_base64(&key).unwrap();
        let token = StoredToken {
            access_token: "access".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: 123,
            scopes: vec!["scope".to_string()],
        };

        let encrypted = cipher.encrypt_token(&token).unwrap();
        let decrypted = cipher
            .decrypt_token(&encrypted.nonce, &encrypted.ciphertext)
            .unwrap();

        assert_eq!(decrypted.access_token, token.access_token);
        assert_eq!(decrypted.refresh_token, token.refresh_token);
        assert_eq!(decrypted.expires_at, token.expires_at);
        assert_eq!(decrypted.scopes, token.scopes);
    }

    #[test]
    fn token_cipher_rejects_tampered_ciphertext() {
        let key = base64::engine::general_purpose::STANDARD.encode([7_u8; 32]);
        let cipher = TokenCipher::from_base64(&key).unwrap();
        let token = StoredToken {
            access_token: "access".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: 123,
            scopes: vec!["scope".to_string()],
        };

        let encrypted = cipher.encrypt_token(&token).unwrap();
        let mut ciphertext = base64::engine::general_purpose::STANDARD
            .decode(&encrypted.ciphertext)
            .unwrap();
        ciphertext[0] ^= 1;
        let tampered = base64::engine::general_purpose::STANDARD.encode(ciphertext);

        assert!(matches!(
            cipher.decrypt_token(&encrypted.nonce, &tampered),
            Err(TokenStoreError::Decrypt)
        ));
    }

    #[test]
    fn token_cipher_rejects_invalid_nonce_length() {
        let key = base64::engine::general_purpose::STANDARD.encode([7_u8; 32]);
        let cipher = TokenCipher::from_base64(&key).unwrap();
        let nonce = base64::engine::general_purpose::STANDARD.encode([1_u8; 8]);
        let ciphertext = base64::engine::general_purpose::STANDARD.encode([2_u8; 32]);

        assert!(matches!(
            cipher.decrypt_token(&nonce, &ciphertext),
            Err(TokenStoreError::InvalidNonce)
        ));
    }
}
