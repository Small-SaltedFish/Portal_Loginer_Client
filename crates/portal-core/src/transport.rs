#[cfg(not(windows))]
use hickory_resolver::config::{LookupIpStrategy, NameServerConfigGroup};
#[cfg(any(test, not(windows)))]
use hickory_resolver::{
    config::{ResolverConfig, ResolverOpts},
    TokioAsyncResolver,
};
use reqwest::{
    cookie::{CookieStore, Jar},
    dns::{Addrs, Name, Resolve, Resolving},
    redirect::Policy,
    Client,
};
use std::{
    collections::HashMap,
    future::Future,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, RwLock},
    time::Duration,
};
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy)]
pub enum RequestKind {
    Probe,
    Script,
    Api,
    Logoff,
}

/// Deliberately not Debug: URLs, Referer and bodies contain session information.
pub struct Request {
    pub url: String,
    pub kind: RequestKind,
    pub post: bool,
    pub body: Option<Zeroizing<String>>,
    pub referer: Option<String>,
    pub accept_json: bool,
}

impl Request {
    pub fn get(url: String, kind: RequestKind) -> Self {
        Self {
            url,
            kind,
            post: false,
            body: None,
            referer: None,
            accept_json: false,
        }
    }
    pub fn limits(&self) -> (Duration, usize) {
        match self.kind {
            RequestKind::Probe => (Duration::from_secs(6), 0),
            RequestKind::Script => (Duration::from_secs(8), 256 * 1024),
            RequestKind::Api | RequestKind::Logoff => (Duration::from_secs(15), 64 * 1024),
        }
    }
}

pub struct Response {
    pub status: u16,
    pub location: Option<String>,
    pub body: Zeroizing<Vec<u8>>,
}

impl Response {
    pub fn successful(&self) -> bool {
        // curl -f rejects >=400, but does not reject an unfollowed 3xx body.
        (200..400).contains(&self.status)
    }
}

#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum TransportError {
    #[error("direct request failed")]
    Unavailable,
    #[error("direct request timed out")]
    Timeout,
    #[error("response exceeds size limit")]
    TooLarge,
    #[error("no independent DNS upstream available")]
    Dns,
}

pub trait Transport: Send + Sync {
    fn execute(
        &self,
        request: Request,
    ) -> impl Future<Output = Result<Response, TransportError>> + Send;
    fn reset_session(&self);
    fn source_ipv4(&self) -> Option<Ipv4Addr> {
        None
    }
}

#[derive(Default)]
struct SessionCookies(RwLock<Jar>);
impl CookieStore for SessionCookies {
    fn set_cookies(
        &self,
        headers: &mut dyn Iterator<Item = &reqwest::header::HeaderValue>,
        url: &url::Url,
    ) {
        self.0
            .read()
            .expect("cookie lock")
            .set_cookies(headers, url);
    }
    fn cookies(&self, url: &url::Url) -> Option<reqwest::header::HeaderValue> {
        self.0.read().expect("cookie lock").cookies(url)
    }
}

#[cfg(any(test, not(windows)))]
struct IndependentDns(Vec<TokioAsyncResolver>);
#[cfg(any(test, not(windows)))]
impl Resolve for IndependentDns {
    fn resolve(&self, name: Name) -> Resolving {
        let resolvers = self.0.clone();
        let name = name.as_str().to_owned();
        Box::pin(async move {
            for resolver in resolvers {
                if let Ok(Ok(result)) = tokio::time::timeout(
                    Duration::from_secs(2),
                    resolver.ipv4_lookup(name.as_str()),
                )
                .await
                {
                    let addresses: Vec<_> = result
                        .iter()
                        .map(|ip| ip.0)
                        .filter(usable_dns_answer)
                        .map(|ip| SocketAddr::new(IpAddr::V4(ip), 0))
                        .collect();
                    if !addresses.is_empty() {
                        return Ok(Box::new(addresses.into_iter()) as Addrs);
                    }
                }
            }
            Err(std::io::Error::other("no usable independent A record").into())
        })
    }
}

/// Like upstream --resolve: resolve independently before starting curl's HTTP
/// timeout, while keeping the original URL, Host and TLS server name.
struct PreparedDns {
    upstream: Arc<dyn Resolve>,
    answers: RwLock<HashMap<String, Vec<SocketAddr>>>,
}
impl PreparedDns {
    async fn prepare(&self, host: &str) -> Result<(), TransportError> {
        let name = host.parse().map_err(|_| TransportError::Dns)?;
        let addresses: Vec<_> = self
            .upstream
            .resolve(name)
            .await
            .map_err(|_| TransportError::Dns)?
            .collect();
        if addresses.is_empty() {
            return Err(TransportError::Dns);
        }
        self.answers
            .write()
            .expect("DNS lock")
            .insert(host.to_owned(), addresses);
        Ok(())
    }
}
impl Resolve for PreparedDns {
    fn resolve(&self, name: Name) -> Resolving {
        let addresses = self
            .answers
            .read()
            .expect("DNS lock")
            .get(name.as_str())
            .cloned();
        Box::pin(async move {
            addresses
                .map(|values| Box::new(values.into_iter()) as Addrs)
                .ok_or_else(|| std::io::Error::other("independent DNS not prepared").into())
        })
    }
}

pub(crate) fn usable_dns_answer(ip: &Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    a != 0 && a != 127 && !(a == 169 && b == 254) && !(a == 198 && matches!(b, 18 | 19)) && a < 224
}

/// One instance per physical-network generation. Dropping it discards cookies,
/// prepared DNS and connections. Explicit proxies are disabled; OS routing still
/// applies, just as upstream curl outside its OpenClash-specific owner worker.
pub struct DirectTransport {
    clients: [Client; 4],
    cookies: Arc<SessionCookies>,
    resolver: Arc<PreparedDns>,
}

impl DirectTransport {
    pub fn new(source: Ipv4Addr, upstreams: &[Ipv4Addr]) -> Result<Self, TransportError> {
        if !usable_dns_answer(&source) {
            return Err(TransportError::Unavailable);
        }
        let mut servers = Vec::new();
        for ip in upstreams
            .iter()
            .copied()
            .chain([Ipv4Addr::new(223, 5, 5, 5), Ipv4Addr::new(119, 29, 29, 29)])
        {
            if usable_dns_answer(&ip) && ip != source && !servers.contains(&ip) {
                servers.push(ip);
            }
        }
        if servers.is_empty() {
            return Err(TransportError::Dns);
        }
        #[cfg(windows)]
        {
            Self::build(
                source,
                Arc::new(crate::bound_dns::BoundDns { source, servers }),
            )
        }
        #[cfg(not(windows))]
        {
            let mut options = ResolverOpts::default();
            options.ip_strategy = LookupIpStrategy::Ipv4Only;
            options.timeout = Duration::from_secs(2);
            options.attempts = 1;
            options.num_concurrent_reqs = 1;
            options.use_hosts_file = false;
            options.cache_size = 0;
            let resolvers = servers
                .into_iter()
                .map(|ip| {
                    let nameservers =
                        NameServerConfigGroup::from_ips_clear(&[IpAddr::V4(ip)], 53, true);
                    TokioAsyncResolver::tokio(
                        ResolverConfig::from_parts(None, Vec::new(), nameservers),
                        options.clone(),
                    )
                })
                .collect();
            let resolver = Arc::new(IndependentDns(resolvers));
            Self::build(source, resolver)
        }
    }

    fn build<R: Resolve + 'static>(
        _source: Ipv4Addr,
        resolver: Arc<R>,
    ) -> Result<Self, TransportError> {
        let jar = Arc::new(SessionCookies::default());
        let resolver = Arc::new(PreparedDns {
            upstream: resolver,
            answers: RwLock::new(HashMap::new()),
        });
        let create = |connect: u64, cookies: bool| {
            let mut builder = Client::builder()
                .user_agent("Mozilla/5.0 (X11; Linux x86_64)")
                .http1_title_case_headers()
                .http1_only()
                .no_proxy()
                .redirect(Policy::none())
                .local_address(std::net::IpAddr::V4(_source))
                .dns_resolver(resolver.clone())
                .connect_timeout(Duration::from_secs(connect))
                .pool_max_idle_per_host(0)
                .pool_idle_timeout(Duration::from_secs(30));
            if cookies {
                builder = builder.cookie_provider(jar.clone());
            }
            builder.build().map_err(|_| TransportError::Unavailable)
        };
        Ok(Self {
            clients: [
                create(4, false)?,
                create(5, false)?,
                create(8, true)?,
                create(8, false)?,
            ],
            cookies: jar,
            resolver,
        })
    }
}

impl Transport for DirectTransport {
    fn reset_session(&self) {
        *self.cookies.0.write().expect("cookie lock") = Jar::default();
    }
    async fn execute(&self, request: Request) -> Result<Response, TransportError> {
        let url = url::Url::parse(&request.url).map_err(|_| TransportError::Unavailable)?;
        if let Some(host) = url
            .host_str()
            .filter(|host| host.parse::<IpAddr>().is_err())
        {
            self.resolver.prepare(host).await?;
        }
        let (limit, size) = request.limits();
        let future = async {
            let index = match request.kind {
                RequestKind::Probe => 0,
                RequestKind::Script => 1,
                RequestKind::Api => 2,
                RequestKind::Logoff => 3,
            };
            let client = &self.clients[index];
            let mut builder = if request.post {
                client.post(&request.url)
            } else {
                client.get(&request.url)
            };
            if matches!(request.kind, RequestKind::Api) {
                builder = builder.header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64)");
            }
            if let Some(referer) = request.referer {
                builder = builder.header("Referer", referer);
            }
            if request.accept_json {
                builder = builder.header("Accept", "application/json, */*");
            }
            if let Some(body) = request.body {
                // reqwest owns its wire buffer. Caller-owned secret buffers zero on drop.
                builder = builder
                    .header(
                        "Content-Type",
                        "application/x-www-form-urlencoded; charset=UTF-8",
                    )
                    .body(body.as_bytes().to_vec());
            }
            let mut response = builder
                .header("Connection", "close")
                .timeout(limit)
                .send()
                .await
                .map_err(map_error)?;
            let status = response.status().as_u16();
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let mut body = Zeroizing::new(Vec::new());
            if size != 0 {
                if response
                    .content_length()
                    .is_some_and(|length| length > size as u64)
                {
                    return Err(TransportError::TooLarge);
                }
                while let Some(chunk) = response.chunk().await.map_err(map_error)? {
                    if chunk.len() > size - body.len() {
                        return Err(TransportError::TooLarge);
                    }
                    body.extend_from_slice(&chunk);
                }
            }
            Ok(Response {
                status,
                location,
                body,
            })
        };
        tokio::time::timeout(limit, future)
            .await
            .map_err(|_| TransportError::Timeout)?
    }
}

fn map_error(error: reqwest::Error) -> TransportError {
    // Never expose reqwest errors: their text can include credential-bearing URLs.
    if error.is_timeout() {
        TransportError::Timeout
    } else {
        TransportError::Unavailable
    }
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
