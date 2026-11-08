use aes::cipher::{BlockEncrypt, KeyInit};
use aes::Aes128;
use rand::{rngs::OsRng, RngCore};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Key([u8; 16]);

impl Key {
    pub fn parse(value: &str) -> Result<Self, CryptoError> {
        if value.len() != 16 || !value.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(CryptoError::InvalidKey);
        }
        let mut key = [0; 16];
        key.copy_from_slice(value.as_bytes());
        Ok(Self(key))
    }

    /// Only the platform persistence layer should export a key. No Debug/Serialize.
    pub fn expose(&self) -> &str {
        std::str::from_utf8(&self.0).expect("validated ASCII")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("invalid protocol key")]
    InvalidKey,
    #[error("secure random source unavailable")]
    RandomUnavailable,
    #[error("invalid random prefix")]
    InvalidPrefix,
}

pub fn encrypt(password: &str, key: &Key) -> Result<Zeroizing<String>, CryptoError> {
    if password.len() == 32 && password.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Ok(Zeroizing::new(password.to_owned()));
    }
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut random = Zeroizing::new([0u8; 4]);
    OsRng
        .try_fill_bytes(random.as_mut())
        .map_err(|_| CryptoError::RandomUnavailable)?;
    let prefix = random.map(|b| ALPHABET[(b & 63) as usize]);
    encrypt_with_prefix(password, key, prefix)
}

/// Deterministic protocol oracle; production uses `encrypt` and OS randomness.
pub fn encrypt_with_prefix(
    password: &str,
    key: &Key,
    prefix: [u8; 4],
) -> Result<Zeroizing<String>, CryptoError> {
    if password.len() == 32 && password.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Ok(Zeroizing::new(password.to_owned()));
    }
    if !prefix
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || b"+/".contains(b))
    {
        return Err(CryptoError::InvalidPrefix);
    }
    let mut plaintext = Zeroizing::new(Vec::with_capacity(password.len() + 20));
    plaintext.extend_from_slice(&prefix);
    plaintext.extend_from_slice(password.as_bytes());
    let padded_len = plaintext.len().div_ceil(16) * 16;
    plaintext.resize(padded_len, 0);
    let cipher = Aes128::new_from_slice(&key.0).expect("16 byte key");
    let mut output = Zeroizing::new(String::with_capacity(plaintext.len() * 2));
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for block in plaintext.chunks_exact_mut(16) {
        cipher.encrypt_block(block.into());
        for byte in block {
            output.push(HEX[(*byte >> 4) as usize] as char);
            output.push(HEX[(*byte & 15) as usize] as char);
        }
    }
    Ok(output)
}
