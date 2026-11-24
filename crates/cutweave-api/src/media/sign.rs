// SPDX-License-Identifier: GPL-3.0-or-later
//! Signed, expiring media URLs (WP-1I.03; arch §6, §18).
//!
//! An `<img src>` cannot send an `Authorization` header, and the UI token must never appear in
//! a URL. A client that holds a token therefore asks `POST /media/sign` for URLs that carry a
//! signature instead: `…?<params>&exp=<unix seconds>&sig=<hex>`, where `sig` is
//! HMAC-SHA256 (RFC 2104) over `<path>?<params>&exp=<unix seconds>` with a 256-bit key drawn
//! from the OS CSPRNG when the server starts. The key never leaves the engine process and dies
//! with it, so a restart invalidates every URL.
//!
//! - **Scope:** one resource. The signature covers the whole path and query, so changing any
//!   parameter (a width, a time, the expiry) breaks it.
//! - **Canonical form:** [`UrlSigner::sign`] re-encodes the query with
//!   `application/x-www-form-urlencoded` rules, so the URL it returns is one browsers send
//!   unchanged; verification compares the bytes the request carried.
//! - **Expiry:** checked after the signature, so a forged URL learns nothing about the clock.

use std::fmt;

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::creds::{TokenError, hex};

/// The query parameter holding the expiry (Unix seconds).
pub const EXPIRES_PARAM: &str = "exp";
/// The query parameter holding the signature; always the last one.
pub const SIGNATURE_PARAM: &str = "sig";
/// Longest path and query accepted for signing.
pub const MAX_SIGNED_LEN: usize = 2048;
/// The only path prefix that can be signed.
pub const MEDIA_PREFIX: &str = "/media/";
/// The signing endpoint itself, which a signed URL can never reach.
pub const SIGN_PATH: &str = "/media/sign";

const SIG_HEX_LEN: usize = 64;

/// Why a URL could not be signed, or a signed request was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SignError {
    /// The path is not a media route (or is `/media/sign`).
    #[error("only /media/... routes can be signed (not /media/sign)")]
    NotMedia,
    /// The path or query holds characters a browser would re-encode, or is too long.
    #[error(
        "the path must be ASCII letters, digits, '/', '-', '_', '.', '~', at most {MAX_SIGNED_LEN} characters"
    )]
    BadPath,
    /// The URL already carries `exp` or `sig`.
    #[error("the URL is already signed (it has `exp` or `sig`)")]
    AlreadySigned,
    /// The request carries no signature.
    #[error("no signature")]
    Missing,
    /// `sig` or `exp` is not in the expected form.
    #[error("malformed signature or expiry")]
    Malformed,
    /// The signature does not match the path and query.
    #[error("the signature does not match this URL")]
    BadSignature,
    /// The signature is valid but expired.
    #[error("the signed URL expired; ask /media/sign for a new one")]
    Expired,
}

/// HMAC-SHA256 (RFC 2104 with SHA-256, block size 64).
#[must_use]
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let inner = Sha256::new()
        .chain_update(ipad)
        .chain_update(message)
        .finalize();
    Sha256::new()
        .chain_update(opad)
        .chain_update(inner)
        .finalize()
        .into()
}

/// Signs and verifies media URLs with a per-launch key (see the module docs).
pub struct UrlSigner {
    key: [u8; 32],
}

impl fmt::Debug for UrlSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("UrlSigner(<key redacted>)")
    }
}

/// Whether `c` may appear in a signable path without a browser changing it.
const fn path_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'/' | b'-' | b'_' | b'.' | b'~')
}

impl UrlSigner {
    /// A signer with a fresh 256-bit key from the OS CSPRNG.
    pub fn generate() -> Result<UrlSigner, TokenError> {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).map_err(|_| TokenError::Random)?;
        Ok(UrlSigner { key })
    }

    /// A signer with a known key (tests).
    #[must_use]
    pub const fn with_key(key: [u8; 32]) -> UrlSigner {
        UrlSigner { key }
    }

    /// `path_and_query` (`/media/...?...`) signed until `expires` (Unix seconds): the query
    /// re-encoded canonically, then `exp` and `sig` appended.
    pub fn sign(&self, path_and_query: &str, expires: u64) -> Result<String, SignError> {
        if path_and_query.len() > MAX_SIGNED_LEN || path_and_query.contains('#') {
            return Err(SignError::BadPath);
        }
        let (path, query) = path_and_query
            .split_once('?')
            .unwrap_or((path_and_query, ""));
        if !path.starts_with(MEDIA_PREFIX) || path == SIGN_PATH || path.contains("..") {
            return Err(SignError::NotMedia);
        }
        if !path.bytes().all(path_char) {
            return Err(SignError::BadPath);
        }
        let mut canonical = form_urlencoded::Serializer::new(String::new());
        for (key, value) in form_urlencoded::parse(query.as_bytes()) {
            if key == EXPIRES_PARAM || key == SIGNATURE_PARAM {
                return Err(SignError::AlreadySigned);
            }
            canonical.append_pair(&key, &value);
        }
        canonical.append_pair(EXPIRES_PARAM, &expires.to_string());
        let message = format!("{path}?{}", canonical.finish());
        let sig = hex(&hmac_sha256(&self.key, message.as_bytes()));
        Ok(format!("{message}&{SIGNATURE_PARAM}={sig}"))
    }

    /// Checks a request's `path` and raw `query` at `now` (Unix seconds): the signature first,
    /// then the expiry.
    pub fn verify(&self, path: &str, query: Option<&str>, now: u64) -> Result<(), SignError> {
        let query = query.ok_or(SignError::Missing)?;
        let marker = format!("&{SIGNATURE_PARAM}=");
        let (rest, sig) = match query.rfind(&marker) {
            Some(i) => (&query[..i], &query[i + marker.len()..]),
            None => return Err(SignError::Missing),
        };
        if sig.len() != SIG_HEX_LEN || !sig.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(SignError::Malformed);
        }
        if !path.starts_with(MEDIA_PREFIX) || path == SIGN_PATH {
            return Err(SignError::NotMedia);
        }
        let expected = hex(&hmac_sha256(&self.key, format!("{path}?{rest}").as_bytes()));
        if !bool::from(expected.as_bytes().ct_eq(sig.as_bytes())) {
            return Err(SignError::BadSignature);
        }
        let mut expires = None;
        for (key, value) in form_urlencoded::parse(rest.as_bytes()) {
            if key == SIGNATURE_PARAM {
                return Err(SignError::Malformed);
            }
            if key == EXPIRES_PARAM {
                if expires.is_some() {
                    return Err(SignError::Malformed);
                }
                expires = Some(value.parse::<u64>().map_err(|_| SignError::Malformed)?);
            }
        }
        match expires {
            Some(exp) if now <= exp => Ok(()),
            Some(_) => Err(SignError::Expired),
            None => Err(SignError::Malformed),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// RFC 4231 test cases 1, 2, 4 and 6 (a key longer than the block).
    #[test]
    fn hmac_sha256_matches_rfc_4231() {
        let cases: [(Vec<u8>, Vec<u8>, &str); 4] = [
            (
                vec![0x0b; 20],
                b"Hi There".to_vec(),
                "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7",
            ),
            (
                b"Jefe".to_vec(),
                b"what do ya want for nothing?".to_vec(),
                "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
            ),
            (
                unhex("0102030405060708090a0b0c0d0e0f10111213141516171819"),
                vec![0xcd; 50],
                "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b",
            ),
            (
                vec![0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First".to_vec(),
                "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54",
            ),
        ];
        for (key, message, want) in cases {
            assert_eq!(hex(&hmac_sha256(&key, &message)), want);
        }
    }

    fn signer() -> UrlSigner {
        UrlSigner::with_key([7; 32])
    }

    #[test]
    fn a_signed_url_verifies_until_it_expires() {
        let url = signer()
            .sign("/media/med_1/thumb?t=0&w=320&hash=blake3-fast:ab", 1000)
            .unwrap();
        assert!(
            url.starts_with("/media/med_1/thumb?t=0&w=320&hash=blake3-fast%3Aab&exp=1000&sig="),
            "{url}"
        );
        let (path, query) = url.split_once('?').unwrap();
        assert_eq!(signer().verify(path, Some(query), 999), Ok(()));
        assert_eq!(signer().verify(path, Some(query), 1000), Ok(()));
        assert_eq!(
            signer().verify(path, Some(query), 1001),
            Err(SignError::Expired)
        );
        // Another launch's key.
        assert_eq!(
            UrlSigner::with_key([8; 32]).verify(path, Some(query), 999),
            Err(SignError::BadSignature)
        );
    }

    #[test]
    fn the_signature_covers_the_path_every_parameter_and_the_expiry() {
        let url = signer().sign("/media/med_1/thumb?t=0&w=320", 1000).unwrap();
        let (path, query) = url.split_once('?').unwrap();
        for (p, q) in [
            ("/media/med_2/thumb", query.to_owned()),
            (path, query.replace("w=320", "w=4096")),
            (path, query.replace("exp=1000", "exp=9999")),
            (path, query.replace("t=0&", "")),
            (path, format!("x=1&{query}")),
        ] {
            assert_eq!(
                signer().verify(p, Some(&q), 0),
                Err(SignError::BadSignature),
                "{p}?{q}"
            );
        }
        assert_eq!(signer().verify(path, None, 0), Err(SignError::Missing));
        assert_eq!(
            signer().verify(path, Some("t=0&w=320"), 0),
            Err(SignError::Missing)
        );
        assert_eq!(
            signer().verify(path, Some("sig=zz"), 0),
            Err(SignError::Missing)
        );
        assert_eq!(
            signer().verify(path, Some("t=0&sig=abc"), 0),
            Err(SignError::Malformed)
        );
    }

    #[test]
    fn only_media_routes_can_be_signed() {
        let s = signer();
        assert_eq!(s.sign("/rpc", 1), Err(SignError::NotMedia));
        assert_eq!(s.sign("/media/sign", 1), Err(SignError::NotMedia));
        assert_eq!(s.sign("/media/../rpc", 1), Err(SignError::NotMedia));
        assert_eq!(s.sign("/media/a b/thumb", 1), Err(SignError::BadPath));
        assert_eq!(s.sign("/media/x#frag", 1), Err(SignError::BadPath));
        assert_eq!(
            s.sign("/media/x/thumb?exp=5", 1),
            Err(SignError::AlreadySigned)
        );
        assert_eq!(
            s.sign(&format!("/media/{}", "a".repeat(MAX_SIGNED_LEN)), 1),
            Err(SignError::BadPath)
        );
        // A path without a query is fine.
        assert!(
            s.sign("/media/frame", 1)
                .unwrap()
                .starts_with("/media/frame?exp=1&sig=")
        );
    }

    #[test]
    fn the_key_is_redacted() {
        assert!(!format!("{:?}", signer()).contains('7'));
    }
}
