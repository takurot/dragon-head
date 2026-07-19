use sha2::{Digest, Sha256};
use std::{
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs},
};
use url::{Host, Url};

pub const MAX_PUBLIC_NAVIGATION_URL_BYTES: usize = 8192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationNetworkPolicy {
    PublicOnly,
    AllowPrivate,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NavigationValidationError {
    #[error("navigation URL is empty")]
    Empty,
    #[error("navigation URL exceeds {max_bytes} UTF-8 bytes")]
    TooLong { max_bytes: usize },
    #[error("navigation URL must be an absolute HTTP(S) URL with a host")]
    InvalidUrl,
    #[error("navigation URL must not contain credentials")]
    CredentialsNotAllowed,
    #[error("navigation destination could not be resolved")]
    ResolutionFailed,
    #[error("navigation destination is not a public network address")]
    NonPublicAddress,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedNavigationUrl {
    canonical_url: String,
    sanitized_projection: String,
    destination_digest: String,
}

impl ValidatedNavigationUrl {
    pub fn canonical_url(&self) -> &str {
        &self.canonical_url
    }

    pub fn sanitized_projection(&self) -> &str {
        &self.sanitized_projection
    }

    pub fn destination_digest(&self) -> &str {
        &self.destination_digest
    }
}

pub fn validate_public_navigation_url(
    input: &str,
    policy: NavigationNetworkPolicy,
) -> Result<ValidatedNavigationUrl, NavigationValidationError> {
    validate_public_navigation_url_with(input, policy, |host, port| {
        (host, port)
            .to_socket_addrs()
            .map(|addresses| addresses.map(|address| address.ip()).collect())
    })
}

pub fn validate_public_navigation_url_with<F>(
    input: &str,
    policy: NavigationNetworkPolicy,
    resolver: F,
) -> Result<ValidatedNavigationUrl, NavigationValidationError>
where
    F: Fn(&str, u16) -> io::Result<Vec<IpAddr>>,
{
    if input.is_empty() {
        return Err(NavigationValidationError::Empty);
    }
    if input.len() > MAX_PUBLIC_NAVIGATION_URL_BYTES {
        return Err(NavigationValidationError::TooLong {
            max_bytes: MAX_PUBLIC_NAVIGATION_URL_BYTES,
        });
    }

    let mut url = Url::parse(input).map_err(|_| NavigationValidationError::InvalidUrl)?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return Err(NavigationValidationError::InvalidUrl);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(NavigationValidationError::CredentialsNotAllowed);
    }
    url.set_fragment(None);
    if let Some(Host::Domain(host)) = url.host() {
        let canonical_host = host.trim_end_matches('.');
        if canonical_host.is_empty() {
            return Err(NavigationValidationError::InvalidUrl);
        }
        if canonical_host != host {
            let canonical_host = canonical_host.to_string();
            url.set_host(Some(&canonical_host))
                .map_err(|_| NavigationValidationError::InvalidUrl)?;
        }
    }

    if policy == NavigationNetworkPolicy::PublicOnly {
        let addresses = match url.host().expect("host checked above") {
            Host::Ipv4(address) => vec![IpAddr::V4(address)],
            Host::Ipv6(address) => vec![IpAddr::V6(address)],
            Host::Domain(host) => resolver(host, url.port_or_known_default().unwrap_or(80))
                .map_err(|_| NavigationValidationError::ResolutionFailed)?,
        };
        if addresses.is_empty() {
            return Err(NavigationValidationError::ResolutionFailed);
        }
        if addresses
            .into_iter()
            .any(|address| !is_global_address(address))
        {
            return Err(NavigationValidationError::NonPublicAddress);
        }
    }

    let canonical_url = url.as_str().to_string();
    let sanitized_projection = sanitized_projection(&url);
    let destination_digest = hex::encode(Sha256::digest(canonical_url.as_bytes()));
    Ok(ValidatedNavigationUrl {
        canonical_url,
        sanitized_projection,
        destination_digest,
    })
}

pub(crate) fn redirect_approval_digest(original: &str, destination: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"dragon-head:navigate-redirect:v1\0");
    digest.update(original.len().to_be_bytes());
    digest.update(original.as_bytes());
    digest.update(destination.len().to_be_bytes());
    digest.update(destination.as_bytes());
    hex::encode(digest.finalize())
}

fn sanitized_projection(url: &Url) -> String {
    let host = match url.host().expect("validated URL has a host") {
        Host::Domain(host) => host.to_string(),
        Host::Ipv4(host) => host.to_string(),
        Host::Ipv6(host) => format!("[{host}]"),
    };
    let port = url
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    format!("{}://{host}{port}{}", url.scheme(), url.path())
}

fn is_global_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_global_ipv4(address),
        IpAddr::V6(address) => is_global_ipv6(address),
    }
}

fn is_global_ipv4(address: Ipv4Addr) -> bool {
    let value = u32::from(address);
    let in_prefix = |network: u32, bits: u32| {
        let mask = if bits == 0 {
            0
        } else {
            u32::MAX << (32 - bits)
        };
        value & mask == network & mask
    };

    if in_prefix(0x0000_0000, 8)
        || in_prefix(0x0a00_0000, 8)
        || in_prefix(0x6440_0000, 10)
        || in_prefix(0x7f00_0000, 8)
        || in_prefix(0xa9fe_0000, 16)
        || in_prefix(0xac10_0000, 12)
        || in_prefix(0xc000_0200, 24)
        || in_prefix(0xc058_6300, 24)
        || in_prefix(0xc0a8_0000, 16)
        || in_prefix(0xc612_0000, 15)
        || in_prefix(0xc633_6400, 24)
        || in_prefix(0xcb00_7100, 24)
        || in_prefix(0xe000_0000, 4)
        || in_prefix(0xf000_0000, 4)
    {
        return false;
    }

    // 192.0.0.0/24 is protocol assignment space; only its two anycast
    // addresses are globally reachable.
    if in_prefix(0xc000_0000, 24) {
        return matches!(address.octets(), [192, 0, 0, 9] | [192, 0, 0, 10]);
    }
    true
}

fn is_global_ipv6(address: Ipv6Addr) -> bool {
    if address.to_ipv4_mapped().is_some() {
        return false;
    }
    let segments = address.segments();
    let value = u128::from(address);
    let in_prefix = |network: u128, bits: u32| {
        let mask = if bits == 0 {
            0
        } else {
            u128::MAX << (128 - bits)
        };
        value & mask == network & mask
    };
    if address.is_unspecified()
        || address.is_loopback()
        || matches!(segments, [0, 0, 0, 0, 0, 0, _, _])
        || matches!(segments, [0, 0, 0, 0, 0xffff, 0, _, _])
        || in_prefix(0x0064_ff9b_0001_0000_0000_0000_0000_0000, 48)
        || in_prefix(0x0100_0000_0000_0000_0000_0000_0000_0000, 64)
        || in_prefix(0x2002_0000_0000_0000_0000_0000_0000_0000, 16)
        || in_prefix(0x2001_0db8_0000_0000_0000_0000_0000_0000, 32)
        || in_prefix(0x3fff_0000_0000_0000_0000_0000_0000_0000, 20)
        || in_prefix(0x5f00_0000_0000_0000_0000_0000_0000_0000, 16)
        || in_prefix(0xfc00_0000_0000_0000_0000_0000_0000_0000, 7)
        || in_prefix(0xfe80_0000_0000_0000_0000_0000_0000_0000, 10)
        || in_prefix(0xfec0_0000_0000_0000_0000_0000_0000_0000, 10)
        || in_prefix(0xff00_0000_0000_0000_0000_0000_0000_0000, 8)
    {
        return false;
    }

    // IETF protocol assignments (2001::/23) are non-global except for the
    // explicitly globally reachable assignments in the IANA registry.
    if in_prefix(0x2001_0000_0000_0000_0000_0000_0000_0000, 23) {
        let globally_reachable_exception =
            matches!(
                value,
                0x2001_0001_0000_0000_0000_0000_0000_0001
                    | 0x2001_0001_0000_0000_0000_0000_0000_0002
            ) || in_prefix(0x2001_0003_0000_0000_0000_0000_0000_0000, 32)
                || in_prefix(0x2001_0004_0112_0000_0000_0000_0000_0000, 48)
                || in_prefix(0x2001_0020_0000_0000_0000_0000_0000_0000, 28)
                || in_prefix(0x2001_0030_0000_0000_0000_0000_0000_0000, 28);
        return globally_reachable_exception;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::redirect_approval_digest;

    #[test]
    fn redirect_digest_binds_both_destinations() {
        let digest = redirect_approval_digest("https://a.test/", "https://b.test/?secret=1");
        assert_ne!(
            digest,
            redirect_approval_digest("https://a.test/", "https://c.test/")
        );
        assert_ne!(
            digest,
            redirect_approval_digest("https://x.test/", "https://b.test/?secret=1")
        );
        assert!(!digest.contains("secret"));
    }
}
