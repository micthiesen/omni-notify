//! Public-internet requests guarded against SSRF.
//!
//! Every request URL and redirect hop passes [`assert_public_http_url_syntax`];
//! every DNS lookup of the transport must answer only public addresses (a
//! mixed answer is rejected whole), which closes the rebinding window.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::{
    DEFAULT_PUBLIC_REDIRECTS, Guard, HttpClient, HttpError, Method, RedirectRule, RequestBuilder,
    Url,
};

/// Which addresses the guard accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressPolicy {
    /// Production: public unicast only.
    PublicOnly,
    /// Tests against a local mock server: public plus loopback
    /// (`127.0.0.0/8`, `::1`, `localhost`). Private ranges stay blocked.
    AllowLoopback,
}

/// The DNS answer for a host included a non-public address.
#[derive(Debug, thiserror::Error)]
#[error("URL host must resolve only to public addresses")]
pub struct NonPublicAddress;

const BLOCKED_V4: &[(Ipv4Addr, u8)] = &[
    (Ipv4Addr::new(0, 0, 0, 0), 8),
    (Ipv4Addr::new(10, 0, 0, 0), 8),
    (Ipv4Addr::new(100, 64, 0, 0), 10),
    (Ipv4Addr::new(127, 0, 0, 0), 8),
    (Ipv4Addr::new(169, 254, 0, 0), 16),
    (Ipv4Addr::new(172, 16, 0, 0), 12),
    (Ipv4Addr::new(192, 0, 0, 0), 24),
    (Ipv4Addr::new(192, 0, 2, 0), 24),
    (Ipv4Addr::new(192, 88, 99, 0), 24),
    (Ipv4Addr::new(192, 168, 0, 0), 16),
    (Ipv4Addr::new(198, 18, 0, 0), 15),
    (Ipv4Addr::new(198, 51, 100, 0), 24),
    (Ipv4Addr::new(203, 0, 113, 0), 24),
    (Ipv4Addr::new(224, 0, 0, 0), 4),
    (Ipv4Addr::new(240, 0, 0, 0), 4),
];

const BLOCKED_V6: &[(Ipv6Addr, u8)] = &[
    (Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0), 128),
    (Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1), 128),
    (Ipv6Addr::new(0x64, 0xff9b, 0, 0, 0, 0, 0, 0), 96),
    (Ipv6Addr::new(0x64, 0xff9b, 1, 0, 0, 0, 0, 0), 48),
    (Ipv6Addr::new(0x100, 0, 0, 0, 0, 0, 0, 0), 64),
    (Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0), 23),
    (Ipv6Addr::new(0x2001, 2, 0, 0, 0, 0, 0, 0), 48),
    (Ipv6Addr::new(0x2001, 0x10, 0, 0, 0, 0, 0, 0), 28),
    (Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0), 32),
    (Ipv6Addr::new(0x2002, 0, 0, 0, 0, 0, 0, 0), 16),
    (Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0), 7),
    (Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0), 10),
    (Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 0), 8),
];

fn in_v4(ip: Ipv4Addr, (network, prefix): (Ipv4Addr, u8)) -> bool {
    let mask = u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0);
    u32::from(ip) & mask == u32::from(network) & mask
}

fn in_v6(ip: Ipv6Addr, (network, prefix): (Ipv6Addr, u8)) -> bool {
    let mask = u128::MAX.checked_shl(128 - u32::from(prefix)).unwrap_or(0);
    u128::from(ip) & mask == u128::from(network) & mask
}

/// IPv4-mapped (`::ffff:a.b.c.d`) and two-group IPv4-compatible (`::x:y`)
/// addresses are judged as the embedded IPv4 address.
fn embedded_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = ip.segments();
    let zero_prefix = s[..5].iter().all(|seg| *seg == 0);
    let mapped = s[5] == 0xffff;
    let compatible = s[5] == 0 && s[6] != 0;
    if zero_prefix && (mapped || compatible) {
        let [a, b] = s[6].to_be_bytes();
        let [c, d] = s[7].to_be_bytes();
        Some(Ipv4Addr::new(a, b, c, d))
    } else {
        None
    }
}

/// IPv4 outside the special-purpose blocks, or IPv6 in
/// `2000::/3` outside the special-purpose blocks.
pub fn is_public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => !BLOCKED_V4.iter().any(|block| in_v4(v4, *block)),
        IpAddr::V6(v6) => match embedded_v4(v6) {
            Some(v4) => is_public_address(IpAddr::V4(v4)),
            None => {
                in_v6(v6, (Ipv6Addr::new(0x2000, 0, 0, 0, 0, 0, 0, 0), 3))
                    && !BLOCKED_V6.iter().any(|block| in_v6(v6, *block))
            }
        },
    }
}

fn allowed(ip: IpAddr, policy: AddressPolicy) -> bool {
    is_public_address(ip) || (policy == AddressPolicy::AllowLoopback && ip.is_loopback())
}

/// Every answer must be allowed; an empty or mixed
/// answer is rejected whole. Returns the answers in resolver order.
pub fn filter_dns_answers(
    answers: Vec<IpAddr>,
    policy: AddressPolicy,
) -> Result<Vec<IpAddr>, NonPublicAddress> {
    if answers.is_empty() || answers.iter().any(|ip| !allowed(*ip, policy)) {
        return Err(NonPublicAddress);
    }
    Ok(answers)
}

/// Http(s), no credentials, a non-localhost host,
/// and a public address when the host is an IP literal.
pub fn assert_public_http_url_syntax(u: &str) -> Result<Url, HttpError> {
    check_url(u, AddressPolicy::PublicOnly)
}

/// The syntax rules plus the host's current DNS
/// answers, checked before work is accepted. Connection-time DNS is still
/// gated by the transport.
pub async fn assert_public_http_url(u: &str) -> Result<Url, HttpError> {
    let url = assert_public_http_url_syntax(u)?;
    if let Some(url::Host::Domain(host)) = url.host() {
        let answers: Vec<IpAddr> = tokio::net::lookup_host((host, 0))
            .await
            .map_err(|e| HttpError::Network(format!("resolve public HTTP URL: {e}")))?
            .map(|addr| addr.ip())
            .collect();
        filter_dns_answers(answers, AddressPolicy::PublicOnly)
            .map_err(|e| HttpError::Blocked(e.to_string()))?;
    }
    Ok(url)
}

pub(crate) fn check_url(u: &str, policy: AddressPolicy) -> Result<Url, HttpError> {
    let url = Url::parse(u).map_err(|e| HttpError::InvalidUrl(e.to_string()))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(HttpError::Blocked("URLs must use HTTP or HTTPS".to_owned()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(HttpError::Blocked(
            "URLs must not contain credentials".to_owned(),
        ));
    }
    let not_public = || HttpError::Blocked("URLs must use a public host".to_owned());
    match url.host() {
        None => return Err(not_public()),
        Some(url::Host::Ipv4(v4)) if !allowed(IpAddr::V4(v4), policy) => {
            return Err(not_public());
        }
        Some(url::Host::Ipv6(v6)) if !allowed(IpAddr::V6(v6), policy) => {
            return Err(not_public());
        }
        Some(url::Host::Domain(domain)) => {
            let host = domain.to_ascii_lowercase();
            let host = host.strip_suffix('.').unwrap_or(&host);
            let localhost = host == "localhost" || host.ends_with(".localhost");
            if host.is_empty() || (localhost && policy == AddressPolicy::PublicOnly) {
                return Err(not_public());
            }
        }
        Some(_) => {}
    }
    Ok(url)
}

/// reqwest resolver over the system resolver that applies [`filter_dns_answers`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct PublicResolver {
    policy: AddressPolicy,
}

impl PublicResolver {
    pub(crate) fn system(policy: AddressPolicy) -> Self {
        Self { policy }
    }
}

impl reqwest::dns::Resolve for PublicResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        let policy = self.policy;
        Box::pin(async move {
            let answers: Vec<IpAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .map(|addr| addr.ip())
                .collect();
            let allowed = filter_dns_answers(answers, policy)?;
            let addrs: reqwest::dns::Addrs =
                Box::new(allowed.into_iter().map(|ip| SocketAddr::new(ip, 0)));
            Ok(addrs)
        })
    }
}

/// A client whose every URL, DNS answer and redirect hop must be public.
/// Redirects are followed (up to [`DEFAULT_PUBLIC_REDIRECTS`]) and each hop is
/// revalidated before it is requested.
#[derive(Clone, Debug)]
pub struct PublicHttpClient {
    base: HttpClient,
    policy: AddressPolicy,
}

impl PublicHttpClient {
    pub fn new(base: &HttpClient) -> Self {
        Self {
            base: base.clone(),
            policy: AddressPolicy::PublicOnly,
        }
    }

    /// Also admits loopback addresses so tests can reach a local mock
    /// server; private and link-local ranges stay blocked. Never use in
    /// production wiring.
    pub fn allow_loopback_for_tests(self) -> Self {
        Self {
            policy: AddressPolicy::AllowLoopback,
            ..self
        }
    }

    pub fn request(&self, m: Method, url: Url) -> RequestBuilder {
        let transport = self.base.public_transport(self.policy).clone();
        self.base.request_on(
            &transport,
            m,
            url,
            Guard::Public(self.policy),
            RedirectRule::Follow(DEFAULT_PUBLIC_REDIRECTS),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public(s: &str) -> bool {
        s.parse::<IpAddr>().map(is_public_address).unwrap_or(false)
    }

    #[test]
    fn address_rules() {
        assert!(public("8.8.8.8"));
        assert!(!public("10.1.2.3"));
        assert!(!public("100.64.0.1"));
        assert!(!public("127.0.0.1"));
        assert!(!public("::1"));
        assert!(!public("::ffff:127.0.0.1"));
        assert!(public("::ffff:8.8.8.8"));
        assert!(!public("fe80::1"));
        assert!(!public("2001:db8::1"));
        assert!(public("2606:4700::1111"));
        assert!(public("3fff::1"));
        assert!(!public("4000::1"));
    }

    #[tokio::test]
    async fn the_resolver_rejects_names_answering_with_loopback() {
        use reqwest::dns::Resolve as _;
        // `localhost` resolves from the hosts file, no network needed.
        let name = || "localhost".parse::<reqwest::dns::Name>().unwrap();
        let blocked = PublicResolver::system(AddressPolicy::PublicOnly)
            .resolve(name())
            .await;
        assert!(blocked.is_err_and(|e| e.is::<NonPublicAddress>()));
        let allowed = PublicResolver::system(AddressPolicy::AllowLoopback)
            .resolve(name())
            .await
            .unwrap();
        assert!(allowed.into_iter().all(|addr| addr.ip().is_loopback()));
    }

    #[test]
    fn url_syntax_rules() {
        assert!(assert_public_http_url_syntax("https://example.com/a").is_ok());
        assert!(assert_public_http_url_syntax("ftp://example.com").is_err());
        assert!(assert_public_http_url_syntax("https://user:pw@example.com").is_err());
        assert!(assert_public_http_url_syntax("http://localhost:3000").is_err());
        assert!(assert_public_http_url_syntax("http://a.localhost.").is_err());
        assert!(assert_public_http_url_syntax("http://192.168.1.1").is_err());
        assert!(assert_public_http_url_syntax("http://[::1]/").is_err());
    }
}
