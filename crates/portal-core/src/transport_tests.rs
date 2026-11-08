use super::*;
use hickory_resolver::{
    config::{NameServerConfig, Protocol},
    proto::{
        op::{Message, MessageType},
        rr::{rdata::A, RData, Record},
    },
};
use std::{
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, UdpSocket},
    task::JoinHandle,
};

struct LocalDns(Arc<AtomicUsize>);
struct SlowDns;
impl Resolve for SlowDns {
    fn resolve(&self, _: Name) -> Resolving {
        Box::pin(async {
            tokio::time::sleep(Duration::from_secs(7)).await;
            Ok(Box::new(vec!["127.0.0.1:0".parse().unwrap()].into_iter()) as Addrs)
        })
    }
}
#[tokio::test]
async fn dns_resolution_precedes_http_deadline_on_pinned_interface() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(12), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut data = [0u8; 2048];
        let n = socket.read(&mut data).await.unwrap();
        assert!(n > 0);
        socket
            .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
    });
    // This address is deliberately not assigned locally. Upstream curl does not bind it.
    let transport = DirectTransport::build(Ipv4Addr::LOCALHOST, Arc::new(SlowDns)).unwrap();
    let result = transport
        .execute(Request::get(
            format!("http://portal.test:{port}/generate_204"),
            RequestKind::Probe,
        ))
        .await
        .unwrap();
    assert_eq!(result.status, 204);
    task.await.unwrap();
}
impl Resolve for LocalDns {
    fn resolve(&self, _: Name) -> Resolving {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(Box::new(vec!["127.0.0.1:0".parse().unwrap()].into_iter()) as Addrs) })
    }
}
fn transport() -> DirectTransport {
    DirectTransport::build(
        Ipv4Addr::LOCALHOST,
        Arc::new(LocalDns(Arc::new(AtomicUsize::new(0)))),
    )
    .unwrap()
}
async fn server(responses: Vec<Vec<u8>>) -> (String, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for response in responses {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut buffer = [0u8; 2048];
                let size = tokio::time::timeout(Duration::from_secs(3), socket.read(&mut buffer))
                    .await
                    .unwrap()
                    .unwrap();
                assert_ne!(size, 0);
                bytes.extend_from_slice(&buffer[..size]);
                if let Some(index) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..index]);
                    let length: usize = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|n| n.trim().parse().unwrap())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= index + 4 + length {
                        break;
                    }
                }
            }
            requests.push(String::from_utf8(bytes).unwrap());
            // A size-limited client may close before reading a large body.
            let _ = socket.write_all(&response).await;
        }
        requests
    });
    (format!("http://portal.test:{}", address.port()), task)
}
fn response(status: &str, extra: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n{extra}\r\n{body}",
        body.len()
    )
    .into_bytes()
}

#[tokio::test]
async fn cookie_session_headers_encoding_and_reset_over_http() {
    let (base, task) = server(vec![
        response(
            "200 OK",
            "Set-Cookie: session=synthetic; Path=/; HttpOnly\r\n",
            "",
        ),
        response("200 OK", "", "{\"ret\":0}"),
        response("200 OK", "", ""),
        response("200 OK", "", ""),
        response("200 OK", "", ""),
    ])
    .await;
    let transport = transport();
    transport
        .execute(Request::get(format!("{base}/index.html"), RequestKind::Api))
        .await
        .unwrap();
    let mut login = Request::get(format!("{base}/api/login.php?bras=hn"), RequestKind::Api);
    login.post = true;
    login.referer = Some(format!("{base}/index.html?bras=hn"));
    login.accept_json = true;
    login.body = Some(crate::protocol::form("test+user@net", "a+/="));
    transport.execute(login).await.unwrap();
    let mut logoff = Request::get(format!("{base}/api/logoff.php"), RequestKind::Logoff);
    logoff.post = true;
    transport.execute(logoff).await.unwrap();
    transport
        .execute(Request::get(format!("{base}/probe"), RequestKind::Probe))
        .await
        .unwrap();
    transport.reset_session();
    transport
        .execute(Request::get(format!("{base}/index.html"), RequestKind::Api))
        .await
        .unwrap();
    let requests = task.await.unwrap();
    let login = requests[1].to_ascii_lowercase();
    assert!(login.contains("cookie: session=synthetic\r\n"));
    assert!(login.contains("user-agent: mozilla/5.0 (x11; linux x86_64)\r\n"));
    assert!(login.contains("content-type: application/x-www-form-urlencoded; charset=utf-8\r\n"));
    assert!(login.contains("accept: application/json, */*\r\n"));
    assert!(login.contains("referer: http://portal.test:"));
    assert!(requests[1].contains("user=test%2Buser%40net&pass=a%2B%2F%3D"));
    assert!(!requests[2].to_ascii_lowercase().contains("cookie:"));
    assert!(requests[2].to_ascii_lowercase().contains("user-agent:"));
    assert!(!requests[3].to_ascii_lowercase().contains("cookie:"));
    assert!(!requests[4].to_ascii_lowercase().contains("cookie:"));
}

#[tokio::test]
async fn redirects_are_returned_without_following() {
    let (base, task) = server(vec![response(
        "302 Found",
        "Location: http://127.0.0.1:1/never\r\n",
        "",
    )])
    .await;
    let result = transport()
        .execute(Request::get(base, RequestKind::Probe))
        .await
        .unwrap();
    assert_eq!(result.status, 302);
    assert_eq!(result.location.as_deref(), Some("http://127.0.0.1:1/never"));
    assert_eq!(task.await.unwrap().len(), 1);
}

#[tokio::test]
async fn request_deadline_covers_waiting_for_response_headers() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    let client = transport();
    let result = tokio::time::timeout(
        Duration::from_secs(8),
        client.execute(Request::get(
            format!("http://portal.test:{}/", address.port()),
            RequestKind::Probe,
        )),
    )
    .await
    .unwrap();
    assert!(matches!(result, Err(TransportError::Timeout)));
    task.abort();
}

#[tokio::test]
async fn declared_and_chunked_body_size_are_bounded() {
    let oversized = "a".repeat(64 * 1024 + 1);
    let chunks = format!("HTTP/1.1 200 OK\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{oversized}\r\n0\r\n\r\n", oversized.len()).into_bytes();
    for payload in [response("200 OK", "", &oversized), chunks] {
        let (base, task) = server(vec![payload]).await;
        let result = transport()
            .execute(Request::get(base, RequestKind::Api))
            .await;
        assert!(matches!(result, Err(TransportError::TooLarge)));
        task.await.unwrap();
    }
}

#[tokio::test]
async fn literal_ip_skips_dns_and_errors_do_not_reveal_url() {
    let (base, task) = server(vec![response("204 No Content", "", "")]).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let transport =
        DirectTransport::build(Ipv4Addr::LOCALHOST, Arc::new(LocalDns(calls.clone()))).unwrap();
    let base = base.replace("portal.test", "127.0.0.1");
    assert_eq!(
        transport
            .execute(Request::get(base, RequestKind::Probe))
            .await
            .unwrap()
            .status,
        204
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    task.await.unwrap();
    let result = transport
        .execute(Request::get(
            "http://127.0.0.1:1/api/login.php?user=secret".into(),
            RequestKind::Api,
        ))
        .await;
    let error = result.err().unwrap();
    assert!(!error.to_string().contains("secret"));
}

async fn dns_upstream(answer: Ipv4Addr) -> (TokioAsyncResolver, JoinHandle<()>) {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = socket.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut data = [0u8; 2048];
        let (len, source) =
            tokio::time::timeout(Duration::from_secs(3), socket.recv_from(&mut data))
                .await
                .unwrap()
                .unwrap();
        let request = Message::from_vec(&data[..len]).unwrap();
        let mut reply = Message::new();
        reply
            .set_id(request.id())
            .set_message_type(MessageType::Response)
            .set_recursion_available(true);
        for query in request.queries() {
            reply.add_query(query.clone());
            reply.add_answer(Record::from_rdata(
                query.name().clone(),
                30,
                RData::A(A(answer)),
            ));
        }
        socket
            .send_to(&reply.to_vec().unwrap(), source)
            .await
            .unwrap();
    });
    let config = ResolverConfig::from_parts(
        None,
        Vec::new(),
        vec![NameServerConfig::new(address, Protocol::Udp)],
    );
    let mut options = ResolverOpts::default();
    options.use_hosts_file = false;
    options.cache_size = 0;
    (TokioAsyncResolver::tokio(config, options), task)
}

#[tokio::test]
async fn independent_dns_discards_fake_ip_and_tries_next_upstream() {
    let (fake, fake_task) = dns_upstream(Ipv4Addr::new(198, 18, 1, 2)).await;
    let (real, real_task) = dns_upstream(Ipv4Addr::new(203, 0, 113, 10)).await;
    let dns = IndependentDns(vec![fake, real]);
    let addresses: Vec<_> = dns
        .resolve("campus-probe.example".parse().unwrap())
        .await
        .unwrap()
        .collect();
    assert_eq!(
        addresses,
        vec!["203.0.113.10:0".parse::<SocketAddr>().unwrap()]
    );
    fake_task.await.unwrap();
    real_task.await.unwrap();
}

#[tokio::test]
async fn https_rejects_untrusted_certificate() {
    use std::io::BufReader;
    use tokio_rustls::{rustls::ServerConfig, TlsAcceptor};
    let certificates = rustls_pemfile::certs(&mut BufReader::new(
        include_bytes!("../../../fixtures/tls-test-cert.pem").as_slice(),
    ))
    .collect::<Result<Vec<_>, _>>()
    .unwrap();
    let private_key = rustls_pemfile::private_key(&mut BufReader::new(
        include_bytes!("../../../fixtures/tls-test-key.pem").as_slice(),
    ))
    .unwrap()
    .unwrap();
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certificates, private_key)
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        // Client must terminate the handshake instead of trusting this fixture.
        assert!(acceptor.accept(socket).await.is_err());
    });
    let result = transport()
        .execute(Request::get(
            format!("https://portal.test:{}/", address.port()),
            RequestKind::Api,
        ))
        .await;
    assert!(matches!(result, Err(TransportError::Unavailable)));
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
}

#[test]
fn proxy_environment_child() {
    if std::env::var_os("PORTAL_PROXY_CHILD").is_none() {
        return;
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let (base, task) = server(vec![response("204 No Content", "", "")]).await;
            // Hostname avoids normal localhost proxy exemptions; only injected DNS
            // knows portal.test, and every explicit proxy points to an unusable port.
            assert_eq!(
                transport()
                    .execute(Request::get(base, RequestKind::Probe))
                    .await
                    .unwrap()
                    .status,
                204
            );
            task.await.unwrap();
        });
}

#[test]
fn ignores_proxy_environment_in_isolated_process() {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "transport::tests::proxy_environment_child",
            "--exact",
            "--nocapture",
        ])
        .env("PORTAL_PROXY_CHILD", "1")
        .env("NO_PROXY", "")
        .env("no_proxy", "");
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env(name, "http://127.0.0.1:1");
    }
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
