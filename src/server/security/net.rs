use reqwest::{
    Client, Method, Response,
    header::{HeaderMap, LOCATION},
};
use std::{
    collections::BTreeSet,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
};
use thiserror::Error;
use url::{Host, Url};

#[derive(Debug, Error)]
pub enum NetSecurityError {
    #[error("only http and https URLs are allowed: {0}")]
    InvalidScheme(String),
    #[error("URL host is required")]
    MissingHost,
    #[error("could not resolve {host}:{port}: {source}")]
    DnsLookup {
        host: String,
        port: u16,
        #[source]
        source: std::io::Error,
    },
    #[error("no resolved addresses were returned for {0}")]
    NoResolvedAddresses(String),
    #[error("address is not publicly routable: {0}")]
    UnsafeAddress(IpAddr),
    #[error("redirect target is missing a Location header")]
    MissingRedirectLocation,
    #[error("redirect target is invalid: {0}")]
    InvalidRedirectTarget(String),
    #[error("redirect limit exceeded after {0} hops")]
    RedirectLimitExceeded(u8),
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
}

pub fn validate_public_http_url(url: &Url) -> Result<(), NetSecurityError> {
    match url.scheme() {
        "http" | "https" => {}
        scheme => return Err(NetSecurityError::InvalidScheme(scheme.to_string())),
    }
    if url.host().is_none() {
        return Err(NetSecurityError::MissingHost);
    }
    Ok(())
}

pub async fn resolve_public_addresses(url: &Url) -> Result<Vec<IpAddr>, NetSecurityError> {
    validate_public_http_url(url)?;

    let host = url.host().ok_or(NetSecurityError::MissingHost)?;
    let port = url
        .port_or_known_default()
        .ok_or(NetSecurityError::MissingHost)?;
    let addresses = match host {
        Host::Ipv4(ipv4) => vec![IpAddr::V4(ipv4)],
        Host::Ipv6(ipv6) => vec![IpAddr::V6(ipv6)],
        Host::Domain(domain) => {
            let resolved = tokio::net::lookup_host((domain, port))
                .await
                .map_err(|source| NetSecurityError::DnsLookup {
                    host: domain.to_string(),
                    port,
                    source,
                })?;
            let addresses = resolved
                .map(|socket| socket.ip())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            if addresses.is_empty() {
                return Err(NetSecurityError::NoResolvedAddresses(domain.to_string()));
            }
            addresses
        }
    };

    for address in &addresses {
        ensure_public_address(*address)?;
    }

    Ok(addresses)
}

pub async fn follow_checked_redirects(
    client: &Client,
    method: Method,
    url: Url,
    headers: HeaderMap,
    maximum_redirects: u8,
) -> Result<Response, NetSecurityError> {
    let mut current = url;
    for redirect_count in 0..=maximum_redirects {
        resolve_public_addresses(&current).await?;
        let response = client
            .request(method.clone(), current.clone())
            .headers(headers.clone())
            .send()
            .await?;

        if !response.status().is_redirection() {
            return Ok(response);
        }
        if redirect_count == maximum_redirects {
            return Err(NetSecurityError::RedirectLimitExceeded(maximum_redirects));
        }

        let location = response
            .headers()
            .get(LOCATION)
            .ok_or(NetSecurityError::MissingRedirectLocation)?
            .to_str()
            .map_err(|_| NetSecurityError::InvalidRedirectTarget("<non-utf8>".to_string()))?;
        current = current
            .join(location)
            .map_err(|_| NetSecurityError::InvalidRedirectTarget(location.to_string()))?;
    }

    Err(NetSecurityError::RedirectLimitExceeded(maximum_redirects))
}

fn ensure_public_address(address: IpAddr) -> Result<(), NetSecurityError> {
    let allowed = match address {
        IpAddr::V4(ipv4) => is_public_ipv4(ipv4),
        IpAddr::V6(ipv6) => is_public_ipv6(ipv6),
    };
    if allowed {
        Ok(())
    } else {
        Err(NetSecurityError::UnsafeAddress(address))
    }
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    !(address.is_private()
        || address.is_loopback()
        || address.is_link_local()
        || address.is_multicast()
        || address.is_unspecified()
        || octets == [255, 255, 255, 255]
        || matches!(
            octets,
            [192, 0, 2, _] | [198, 51, 100, _] | [203, 0, 113, _]
        ))
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    if address.is_loopback() || address.is_multicast() || address.is_unspecified() {
        return false;
    }

    let segments = address.segments();
    let first = segments[0];
    if (first & 0xfe00) == 0xfc00 {
        return false;
    }
    if (first & 0xffc0) == 0xfe80 {
        return false;
    }
    if first == 0x2001 && segments[1] == 0x0db8 {
        return false;
    }

    if let Some(mapped) = address.to_ipv4_mapped() {
        return is_public_ipv4(mapped);
    }

    true
}
