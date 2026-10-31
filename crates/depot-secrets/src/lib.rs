//! Authenticated encryption and external key-file handling for project secrets.

use std::{fs, path::Path};

use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use thiserror::Error;

const KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 24;

/// Encrypted value and unique nonce safe to persist together in SQLite.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncryptedValue {
    pub ciphertext: Vec<u8>,
    pub nonce: [u8; NONCE_BYTES],
}

/// In-memory encryption boundary initialized from a host-owned master key.
#[derive(Clone)]
pub struct SecretCipher {
    cipher: XChaCha20Poly1305,
}

impl std::fmt::Debug for SecretCipher {
    /// Omits key material from debug output while retaining useful type context.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecretCipher")
            .finish_non_exhaustive()
    }
}

/// Key loading, encoding, and authenticated-encryption failures.
#[derive(Debug, Error)]
pub enum SecretError {
    #[error("could not read the secret master key: {0}")]
    KeyRead(#[source] std::io::Error),
    #[error("the secret master key must contain exactly 64 hexadecimal characters")]
    InvalidKey,
    #[error("secret encryption failed")]
    Encrypt,
    #[error("secret authentication or decryption failed")]
    Decrypt,
}

impl SecretCipher {
    /// Loads one 256-bit hexadecimal master key from an operator-owned file.
    pub fn load(path: &Path) -> Result<Self, SecretError> {
        let encoded = fs::read_to_string(path).map_err(SecretError::KeyRead)?;
        let key = decode_key(encoded.trim())?;
        let cipher =
            XChaCha20Poly1305::new_from_slice(&key).map_err(|_| SecretError::InvalidKey)?;
        Ok(Self { cipher })
    }

    /// Encrypts a value while binding it to project and variable identity as AAD.
    pub fn encrypt(&self, plaintext: &str, context: &[u8]) -> Result<EncryptedValue, SecretError> {
        let nonce = rand::random::<[u8; NONCE_BYTES]>();
        let nonce_value = XNonce::try_from(nonce.as_slice()).map_err(|_| SecretError::Encrypt)?;
        let ciphertext = self
            .cipher
            .encrypt(
                &nonce_value,
                Payload {
                    msg: plaintext.as_bytes(),
                    aad: context,
                },
            )
            .map_err(|_| SecretError::Encrypt)?;
        Ok(EncryptedValue { ciphertext, nonce })
    }

    /// Authenticates the stored context before returning decrypted UTF-8 text.
    pub fn decrypt(
        &self,
        ciphertext: &[u8],
        nonce: &[u8],
        context: &[u8],
    ) -> Result<String, SecretError> {
        if nonce.len() != NONCE_BYTES {
            return Err(SecretError::Decrypt);
        }
        let nonce_value = XNonce::try_from(nonce).map_err(|_| SecretError::Decrypt)?;
        let plaintext = self
            .cipher
            .decrypt(
                &nonce_value,
                Payload {
                    msg: ciphertext,
                    aad: context,
                },
            )
            .map_err(|_| SecretError::Decrypt)?;
        String::from_utf8(plaintext).map_err(|_| SecretError::Decrypt)
    }
}

/// Generates a fresh 256-bit key encoded for a restricted text file.
pub fn generate_key_hex() -> String {
    encode_hex(&rand::random::<[u8; KEY_BYTES]>())
}

/// Decodes an exact lowercase or uppercase hexadecimal master key.
fn decode_key(encoded: &str) -> Result<[u8; KEY_BYTES], SecretError> {
    if encoded.len() != KEY_BYTES * 2 {
        return Err(SecretError::InvalidKey);
    }
    let mut key = [0_u8; KEY_BYTES];
    for (index, pair) in encoded.as_bytes().chunks_exact(2).enumerate() {
        let high = decode_nibble(pair[0]).ok_or(SecretError::InvalidKey)?;
        let low = decode_nibble(pair[1]).ok_or(SecretError::InvalidKey)?;
        key[index] = (high << 4) | low;
    }
    Ok(key)
}

/// Converts one ASCII hexadecimal digit to its four-bit value.
fn decode_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

/// Encodes key bytes without exposing them to formatting implementations.
fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::{SecretCipher, SecretError, decode_key, generate_key_hex};
    use chacha20poly1305::{XChaCha20Poly1305, aead::KeyInit};

    /// Proves round trips require the same authenticated project/name context.
    #[test]
    fn encrypted_values_are_context_bound_and_randomized() {
        let key = decode_key(&generate_key_hex()).expect("generated key decodes");
        let cipher = SecretCipher {
            cipher: XChaCha20Poly1305::new_from_slice(&key).expect("key length is valid"),
        };
        let first = cipher.encrypt("secret", b"project:NAME").expect("encrypts");
        let second = cipher.encrypt("secret", b"project:NAME").expect("encrypts");
        assert_ne!(first, second);
        assert_eq!(
            cipher
                .decrypt(&first.ciphertext, &first.nonce, b"project:NAME")
                .expect("decrypts"),
            "secret"
        );
        assert!(matches!(
            cipher.decrypt(&first.ciphertext, &first.nonce, b"other:NAME"),
            Err(SecretError::Decrypt)
        ));
    }
}
