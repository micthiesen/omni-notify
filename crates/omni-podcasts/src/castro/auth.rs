//! Castro `APIAuth-HMAC-SHA256` request signing (`castro/auth.ts`).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, KeyInit as _, Mac as _};
use sha2::{Digest as _, Sha256};

const DEFAULT_CONTENT_TYPE: &str = "application/json";

/// Device credentials: the access id and the raw HMAC secret bytes.
#[derive(Clone)]
pub struct CastroCredentials {
    pub access_id: String,
    pub secret: Vec<u8>,
}

impl std::fmt::Debug for CastroCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CastroCredentials")
            .field("access_id", &self.access_id)
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// The parts of a request the signature covers.
#[derive(Clone, Debug, Default)]
pub struct CastroRequestToSign<'a> {
    pub method: &'a str,
    pub path_and_query: &'a str,
    /// RFC 7231 `Date` header value.
    pub date: &'a str,
    pub body: &'a str,
    pub content_type: Option<&'a str>,
}

/// The four headers Castro requires.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CastroAuthHeaders {
    pub authorization: String,
    pub content_type: String,
    pub date: String,
    pub content_sha256: String,
}

/// Base64 SHA-256 of the body (`""` for bodiless requests).
pub fn hash_castro_request_body(body: &str) -> String {
    STANDARD.encode(Sha256::digest(body.as_bytes()))
}

/// `METHOD,content-type,content-hash,path?query,date`.
pub fn create_castro_canonical_string(
    request: &CastroRequestToSign<'_>,
    content_hash: &str,
) -> String {
    [
        request.method.to_uppercase().as_str(),
        request.content_type.unwrap_or(DEFAULT_CONTENT_TYPE),
        content_hash,
        request.path_and_query,
        request.date,
    ]
    .join(",")
}

/// Base64 HMAC-SHA256 of the canonical string.
pub fn sign_castro_canonical_string(canonical: &str, secret: &[u8]) -> String {
    // HMAC accepts keys of any length, so construction cannot fail.
    match Hmac::<Sha256>::new_from_slice(secret) {
        Ok(mut mac) => {
            mac.update(canonical.as_bytes());
            STANDARD.encode(mac.finalize().into_bytes())
        }
        Err(_) => String::new(),
    }
}

pub fn create_castro_auth_headers(
    credentials: &CastroCredentials,
    request: &CastroRequestToSign<'_>,
) -> CastroAuthHeaders {
    let content_hash = hash_castro_request_body(request.body);
    let signature = sign_castro_canonical_string(
        &create_castro_canonical_string(request, &content_hash),
        &credentials.secret,
    );
    CastroAuthHeaders {
        authorization: format!("APIAuth-HMAC-SHA256 {}:{signature}", credentials.access_id),
        content_type: request
            .content_type
            .unwrap_or(DEFAULT_CONTENT_TYPE)
            .to_owned(),
        date: request.date.to_owned(),
        content_sha256: content_hash,
    }
}
