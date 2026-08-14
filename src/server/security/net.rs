use reqwest::{
    Client, ClientBuilder, Method, Response,
    header::{HeaderMap, LOCATION},
    redirect::Policy,
};
use std::{
    collections::BTreeSet,
    fmt,
    future::Future,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    pin::Pin,
    sync::Arc,
    time::Duration,
};
use thiserror::Error;
use url::{Host, Url};

pub type ResolveFuture<'a> = Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send + 'a>>;

pub trait AddressResolver: Send + Sync {
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> ResolveFuture<'a>;
}

#[derive(Debug, Clone, Copy, Default)]
struct SystemResolver;

impl AddressResolver for SystemResolver {
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> ResolveFuture<'a> {
        Box::pin(async move {
            tokio::net::lookup_host((host, port))
                .await
                .map(Iterator::collect)
        })
    }
}

pub struct DownloadClientBuilder {
    client: ClientBuilder,
    resolver: Arc<dyn AddressResolver>,
    #[cfg(all(feature = "server", debug_assertions))]
    test_peer_address: Option<SocketAddr>,
}

#[derive(Clone)]
pub struct DownloadClient {
    client: Client,
    resolver: Arc<dyn AddressResolver>,
    #[cfg(all(feature = "server", debug_assertions))]
    test_peer_address: Option<SocketAddr>,
}

impl DownloadClient {
    pub fn builder() -> DownloadClientBuilder {
        DownloadClientBuilder {
            client: Client::builder(),
            resolver: Arc::new(SystemResolver),
            #[cfg(all(feature = "server", debug_assertions))]
            test_peer_address: None,
        }
    }

    pub fn new() -> Result<Self, reqwest::Error> {
        Self::builder().build()
    }

    async fn send(
        &self,
        method: Method,
        url: Url,
        headers: HeaderMap,
    ) -> Result<DownloadResponse, reqwest::Error> {
        let response = self
            .client
            .request(method, url)
            .headers(headers)
            .send()
            .await?;
        let remote_addr = self.test_peer_address().or_else(|| response.remote_addr());
        Ok(DownloadResponse {
            response,
            remote_addr,
        })
    }

    async fn resolve_addresses(&self, url: &Url) -> Result<Vec<IpAddr>, NetSecurityError> {
        resolve_public_addresses_with(url, self.resolver.as_ref()).await
    }

    #[cfg(all(feature = "server", debug_assertions))]
    fn test_peer_address(&self) -> Option<SocketAddr> {
        self.test_peer_address
    }

    #[cfg(not(all(feature = "server", debug_assertions)))]
    fn test_peer_address(&self) -> Option<SocketAddr> {
        None
    }
}

impl DownloadClientBuilder {
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.client = self.client.connect_timeout(timeout);
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.client = self.client.timeout(timeout);
        self
    }

    pub fn tcp_keepalive(mut self, timeout: Duration) -> Self {
        self.client = self.client.tcp_keepalive(timeout);
        self
    }

    pub fn resolve(mut self, domain: &str, address: SocketAddr) -> Self {
        self.client = self.client.resolve(domain, address);
        self
    }

    pub fn resolve_to_addrs(mut self, domain: &str, addresses: &[SocketAddr]) -> Self {
        self.client = self.client.resolve_to_addrs(domain, addresses);
        self
    }

    pub fn resolver<R>(mut self, resolver: R) -> Self
    where
        R: AddressResolver + 'static,
    {
        self.resolver = Arc::new(resolver);
        self
    }

    #[cfg(all(feature = "server", debug_assertions))]
    #[doc(hidden)]
    pub fn test_peer_address(mut self, address: SocketAddr) -> Self {
        self.test_peer_address = Some(address);
        self
    }

    pub fn build(self) -> Result<DownloadClient, reqwest::Error> {
        let client = self.client.redirect(Policy::none()).build()?;
        Ok(DownloadClient {
            client,
            resolver: self.resolver,
            #[cfg(all(feature = "server", debug_assertions))]
            test_peer_address: self.test_peer_address,
        })
    }
}

pub struct DownloadResponse {
    response: Response,
    remote_addr: Option<SocketAddr>,
}

impl fmt::Debug for DownloadResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DownloadResponse")
            .field("status", &self.status())
            .field("url", &self.url())
            .field("remote_addr", &self.remote_addr)
            .finish()
    }
}

impl DownloadResponse {
    pub fn status(&self) -> reqwest::StatusCode {
        self.response.status()
    }

    pub fn headers(&self) -> &HeaderMap {
        self.response.headers()
    }

    pub fn url(&self) -> &Url {
        self.response.url()
    }

    pub fn remote_addr(&self) -> Option<SocketAddr> {
        self.remote_addr
    }

    pub fn content_length(&self) -> Option<u64> {
        self.response.content_length()
    }

    pub async fn chunk(&mut self) -> Result<Option<Vec<u8>>, reqwest::Error> {
        Ok(self.response.chunk().await?.map(|chunk| chunk.to_vec()))
    }

    pub async fn bytes(self) -> Result<Vec<u8>, reqwest::Error> {
        Ok(self.response.bytes().await?.to_vec())
    }
}

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
        source: io::Error,
    },
    #[error("no resolved addresses were returned for {0}")]
    NoResolvedAddresses(String),
    #[error("address is not publicly routable: {0}")]
    UnsafeAddress(IpAddr),
    #[error("connected peer address was unavailable")]
    ConnectedAddressUnavailable,
    #[error("connected peer address is not in validated DNS results: {0}")]
    ConnectedAddressMismatch(IpAddr),
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

#[cfg_attr(not(feature = "server"), allow(dead_code))]
pub async fn resolve_public_addresses(url: &Url) -> Result<Vec<IpAddr>, NetSecurityError> {
    resolve_public_addresses_with(url, &SystemResolver).await
}

async fn resolve_public_addresses_with(
    url: &Url,
    resolver: &dyn AddressResolver,
) -> Result<Vec<IpAddr>, NetSecurityError> {
    validate_public_http_url(url)?;

    let host = url.host().ok_or(NetSecurityError::MissingHost)?;
    let port = url
        .port_or_known_default()
        .ok_or(NetSecurityError::MissingHost)?;
    let addresses = match host {
        Host::Ipv4(ipv4) => vec![IpAddr::V4(ipv4)],
        Host::Ipv6(ipv6) => vec![IpAddr::V6(ipv6)],
        Host::Domain(domain) => {
            let resolved = resolver.resolve(domain, port).await.map_err(|source| {
                NetSecurityError::DnsLookup {
                    host: domain.to_string(),
                    port,
                    source,
                }
            })?;
            let addresses = resolved
                .into_iter()
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
    client: &DownloadClient,
    method: Method,
    url: Url,
    headers: HeaderMap,
    maximum_redirects: u8,
) -> Result<DownloadResponse, NetSecurityError> {
    let mut current = url;
    for redirect_count in 0..=maximum_redirects {
        let validated_addresses = client.resolve_addresses(&current).await?;
        let response = client
            .send(method.clone(), current.clone(), headers.clone())
            .await?;
        validate_connected_peer(&validated_addresses, response.remote_addr())?;

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

fn validate_connected_peer(
    validated_addresses: &[IpAddr],
    remote_addr: Option<SocketAddr>,
) -> Result<(), NetSecurityError> {
    let remote = remote_addr.ok_or(NetSecurityError::ConnectedAddressUnavailable)?;
    ensure_public_address(remote.ip())?;
    if validated_addresses
        .iter()
        .any(|address| equivalent_ip(*address, remote.ip()))
    {
        Ok(())
    } else {
        Err(NetSecurityError::ConnectedAddressMismatch(remote.ip()))
    }
}

fn equivalent_ip(address: IpAddr, other: IpAddr) -> bool {
    normalize_ip(address) == normalize_ip(other)
}

fn normalize_ip(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(ipv6) => ipv6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(ipv6)),
        address => address,
    }
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

#[cfg(test)]
mod tests {
    use super::{NetSecurityError, validate_connected_peer};
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn missing_connected_peer_is_rejected() {
        let error =
            validate_connected_peer(&[IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))], None).unwrap_err();

        assert!(matches!(
            error,
            NetSecurityError::ConnectedAddressUnavailable
        ));
    }
}
