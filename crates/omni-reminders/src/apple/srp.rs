//! Apple GSA SRP-6a (SHA-256, 2048-bit group), adapted from ticaki/ioBroker.icloud
//! v2.1.2 (MIT, Copyright (c) 2026 ticaki <github@renopoint.de>; full notice in
//! docs/licenses/ioBroker.icloud-MIT.txt).

use hmac::{Hmac, KeyInit as _, Mac as _};
use num_bigint::BigUint;
use sha2::{Digest as _, Sha256};

use crate::json::{base64_encode, node_base64};

/// RFC 5054 group 14 / Apple GSA prime.
const SRP_N_HEX: &str = "ac6bdb41324a9a9bf166de5e1389582faf72b6651987ee07fc3192943db56050a37329cbb4a099ed8193e0757767a13dd52312ab4b03310dcd7f48a9da04fd50e8083969edb767b0cf6095179a163ab3661a05fbd5faaae82918a9962f0b93b855f97993ec975eeaa80d740adbf4ff747359d041d5c33ea71d281e446b14773bca97b43a23fb801676bd207a436c6481f1d2b9078717461a5b9d32e688f87748544523b524b0d57d5ea77a2775d2ecfa032cfbdbf52fb3786160279004e57ae6af874e7303ce53299ccc041c7bc308d82a5698f3a8d0c38271ae35f8e9dbfbb694b5c803d89f7ae435de236d525f54759b65e372fcd68ef20fa7111f9e4aff73";
pub(crate) const SRP_N_BYTES: usize = 256;

pub(crate) fn srp_n() -> BigUint {
    BigUint::parse_bytes(SRP_N_HEX.as_bytes(), 16).unwrap_or_default()
}

fn g() -> BigUint {
    BigUint::from(2u8)
}

/// `srpBytesFromBigint`: minimal big-endian bytes (`[0]` for zero).
fn bytes(n: &BigUint) -> Vec<u8> {
    n.to_bytes_be()
}

fn pad(n: &BigUint, len: usize) -> Vec<u8> {
    let raw = bytes(n);
    if raw.len() >= len {
        return raw;
    }
    let mut out = vec![0u8; len - raw.len()];
    out.extend(raw);
    out
}

fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

fn big(digest: &[u8]) -> BigUint {
    BigUint::from_bytes_be(digest)
}

/// PBKDF2-HMAC-SHA256 with one 32-byte block.
fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32) -> Option<[u8; 32]> {
    let mac = Hmac::<Sha256>::new_from_slice(password).ok()?;
    let mut first = mac.clone();
    first.update(salt);
    first.update(&1u32.to_be_bytes());
    let mut u: [u8; 32] = first.finalize().into_bytes().into();
    let mut t = u;
    for _ in 1..iterations {
        let mut next = mac.clone();
        next.update(&u);
        u = next.finalize().into_bytes().into();
        for (acc, byte) in t.iter_mut().zip(u.iter()) {
            *acc ^= byte;
        }
    }
    Some(t)
}

/// The server's `signin/init` challenge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerChallenge {
    pub protocol: SrpProtocol,
    pub iteration: u32,
    pub salt: String,
    pub b: String,
    pub c: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SrpProtocol {
    S2k,
    S2kFo,
}

impl SrpProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::S2k => "s2k",
            Self::S2kFo => "s2k_fo",
        }
    }
}

/// The `signin/complete` proof fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SrpProof {
    pub account_name: String,
    pub m1: String,
    pub m2: String,
    pub c: String,
}

/// One client handshake (`GSASRPAuthenticator`).
pub struct GsaSrpAuthenticator {
    username: String,
    a: BigUint,
    big_a: BigUint,
}

impl GsaSrpAuthenticator {
    /// A fresh ephemeral secret from the OS CSPRNG.
    pub fn new(username: &str) -> Self {
        let secret: [u8; SRP_N_BYTES] = std::array::from_fn(|_| rand::random::<u8>());
        Self::with_secret(username, BigUint::from_bytes_be(&secret))
    }

    /// A fixed ephemeral secret (golden tests).
    pub fn with_secret(username: &str, a: BigUint) -> Self {
        let big_a = g().modpow(&a, &srp_n());
        Self {
            username: username.to_owned(),
            a,
            big_a,
        }
    }

    /// `getInit().a`: base64 of `A`.
    pub fn public_a(&self) -> String {
        base64_encode(&bytes(&self.big_a))
    }

    fn derive_password(
        protocol: SrpProtocol,
        password: &str,
        salt: &[u8],
        iterations: u32,
    ) -> Option<[u8; 32]> {
        let hash = sha256(&[password.as_bytes()]);
        match protocol {
            SrpProtocol::S2k => pbkdf2_sha256(&hash, salt, iterations),
            SrpProtocol::S2kFo => pbkdf2_sha256(hex::encode(hash).as_bytes(), salt, iterations),
        }
    }

    /// `getComplete(password, serverData)`; CPU-bound (PBKDF2), run off the reactor.
    pub fn complete(&self, password: &str, server: &ServerChallenge) -> Option<SrpProof> {
        let n = srp_n();
        let salt = node_base64(&server.salt);
        let big_b = BigUint::from_bytes_be(&node_base64(&server.b));
        let k = big(&sha256(&[&bytes(&n), &pad(&g(), SRP_N_BYTES)]));
        let u = big(&sha256(&[
            &pad(&self.big_a, SRP_N_BYTES),
            &pad(&big_b, SRP_N_BYTES),
        ]));
        let p = Self::derive_password(server.protocol, password, &salt, server.iteration)?;
        let x = big(&sha256(&[&salt, &sha256(&[b":", &p])]));
        // S = (B - k*g^x)^(a + u*x) mod N, normalising the negative base like JS.
        let kgx = (g().modpow(&x, &n) * &k) % &n;
        let base = ((&big_b % &n) + &n - kgx) % &n;
        let s = base.modpow(&(&self.a + &u * &x), &n);
        let key = sha256(&[&bytes(&s)]);
        let hn = sha256(&[&bytes(&n)]);
        let hg = sha256(&[&pad(&g(), SRP_N_BYTES)]);
        let xor: Vec<u8> = hn.iter().zip(hg.iter()).map(|(a, b)| a ^ b).collect();
        let m1 = sha256(&[
            &xor,
            &sha256(&[self.username.as_bytes()]),
            &salt,
            &bytes(&self.big_a),
            &bytes(&big_b),
            &key,
        ]);
        let m2 = sha256(&[&bytes(&self.big_a), &m1, &key]);
        Some(SrpProof {
            account_name: self.username.clone(),
            m1: base64_encode(&m1),
            m2: base64_encode(&m2),
            c: server.c.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pbkdf2_matches_rfc7914_vector() {
        // RFC 7914 section 11: PBKDF2-HMAC-SHA256("passwd", "salt", 1, 64) first block.
        let out = pbkdf2_sha256(b"passwd", b"salt", 1).unwrap();
        assert_eq!(
            hex::encode(out),
            "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc"
        );
    }
}
