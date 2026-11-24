//! The MyJDownloader protocol's cryptography: secrets derived from the account, HMAC-signed
//! queries, and AES-128-CBC payloads whose IV and key are the two halves of a 32-byte token.

use aes::Aes128;
use base64::{Engine, engine::general_purpose::STANDARD};
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::error::Error;

/// A 32-byte secret or token: the first half is the IV, the second the AES key.
pub type Token = [u8; 32];

pub fn sha256(parts: &[&[u8]]) -> Token {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update(part);
    }
    hash.finalize().into()
}

/// The account secret for a domain (`server` signs account calls, `device` encrypts device calls).
/// Only the derived secrets are kept, never the password.
pub fn account_secret(email: &str, password: &str, domain: &str) -> Token {
    sha256(&[
        email.to_lowercase().as_bytes(),
        password.as_bytes(),
        domain.to_lowercase().as_bytes(),
    ])
}

/// A session's token: the previous token (or secret) hashed with the session token's bytes.
pub fn session_token(previous: &Token, session: &str) -> Result<Token, Error> {
    let session = hex::decode(session).map_err(|_| Error::Protocol("session token is not hex"))?;
    Ok(sha256(&[previous, &session]))
}

/// Lower-case hex HMAC-SHA256 of a query, as the server expects in `signature=`.
pub fn sign(key: &Token, query: &str) -> String {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(query.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn halves(token: &Token) -> ([u8; 16], [u8; 16]) {
    let mut iv = [0u8; 16];
    let mut key = [0u8; 16];
    iv.copy_from_slice(&token[..16]);
    key.copy_from_slice(&token[16..]);
    (iv, key)
}

pub fn encrypt(token: &Token, plain: &str) -> String {
    let (iv, key) = halves(token);
    let cipher = cbc::Encryptor::<Aes128>::new(&key.into(), &iv.into());
    STANDARD.encode(cipher.encrypt_padded_vec_mut::<Pkcs7>(plain.as_bytes()))
}

pub fn decrypt(token: &Token, encoded: &str) -> Result<String, Error> {
    let (iv, key) = halves(token);
    let data = STANDARD
        .decode(encoded.trim())
        .map_err(|_| Error::Protocol("response is not base64"))?;
    let cipher = cbc::Decryptor::<Aes128>::new(&key.into(), &iv.into());
    let plain = cipher
        .decrypt_padded_vec_mut::<Pkcs7>(&data)
        .map_err(|_| Error::Protocol("response cannot be decrypted (wrong password?)"))?;
    String::from_utf8(plain).map_err(|_| Error::Protocol("response is not UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_secrets_from_the_lower_case_email() {
        assert_eq!(
            account_secret("Ada@Example.org", "pw", "server"),
            account_secret("ada@example.org", "pw", "SERVER"),
        );
        assert_ne!(
            account_secret("ada@example.org", "pw", "server"),
            account_secret("ada@example.org", "pw", "device"),
        );
        // sha256("a" "b" "c") is sha256("abc").
        assert_eq!(
            hex::encode(account_secret("a", "b", "c")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn chains_session_tokens() {
        let secret = [7u8; 32];
        let token = session_token(&secret, "00ff").unwrap();
        assert_eq!(token, sha256(&[&secret, &[0x00, 0xff]]));
        assert!(session_token(&secret, "zz").is_err());
    }

    #[test]
    fn signs_queries_with_hmac_sha256() {
        // RFC 4231 test case 2, with the key padded into our 32-byte token type.
        let mut key = [0u8; 32];
        key[..4].copy_from_slice(b"Jefe");
        let signature = sign(&key, "what do ya want for nothing?");
        assert_eq!(signature.len(), 64);
        assert_ne!(signature, sign(&key, "what do ya want for something?"));
    }

    #[test]
    fn encrypts_and_decrypts_round_trip() {
        let token = sha256(&[b"token"]);
        let sealed = encrypt(&token, r#"{"rid":1,"data":"héllo"}"#);
        assert_eq!(
            decrypt(&token, &sealed).unwrap(),
            r#"{"rid":1,"data":"héllo"}"#
        );
        assert!(decrypt(&sha256(&[b"other"]), &sealed).is_err());
        assert!(decrypt(&token, "not base64!").is_err());
    }
}
