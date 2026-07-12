//! Integration tests for the HTTP-aware passthrough proxy (decision D-21).
//!
//! An in-process fake dockerd listens on a unix socket and records the raw
//! requests it receives; the proxy router is served over a second unix socket
//! using the same hyper auto-Builder accept loop as production `server.rs`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use axum::Router;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use speck_core::VmState;
use speck_dockerd::proxy::build_proxy_router;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tower::ServiceExt;

#[derive(Debug, Clone)]
struct RecordedRequest {
    method: String,
    path_and_query: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl RecordedRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Clone)]
enum FakeScript {
    Canned(Vec<u8>),
    UpgradeEcho,
}

static NEXT_DIR_ID: AtomicU32 = AtomicU32::new(0);

fn unique_dir() -> PathBuf {
    let id = NEXT_DIR_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("spk-pxy-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create per-test temp dir");
    dir
}

fn running_state() -> Arc<RwLock<VmState>> {
    Arc::new(RwLock::new(VmState::Running))
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

async fn read_head(stream: &mut UnixStream) -> (Vec<u8>, Vec<u8>) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(pos) = find_double_crlf(&buf) {
            let leftover = buf.split_off(pos + 4);
            return (buf, leftover);
        }
        let n = stream
            .read(&mut chunk)
            .await
            .expect("read while waiting for HTTP head");
        assert!(n > 0, "unexpected EOF while reading HTTP head");
        buf.extend_from_slice(&chunk[..n]);
    }
}

fn parse_headers(lines: std::str::Split<'_, &str>) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
        }
    }
    headers
}

fn content_length(headers: &[(String, String)]) -> usize {
    headers
        .iter()
        .find(|(n, _)| n == "content-length")
        .map(|(_, v)| v.parse().expect("valid Content-Length"))
        .unwrap_or(0)
}

async fn read_response(stream: &mut UnixStream) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let (head, leftover) = read_head(stream).await;
    let text = String::from_utf8(head).expect("UTF-8 response head");
    let mut lines = text.split("\r\n");
    let status_line = lines.next().expect("status line");
    let status: u16 = status_line
        .split(' ')
        .nth(1)
        .expect("status code in status line")
        .parse()
        .expect("numeric status code");
    let headers = parse_headers(lines);
    let want = content_length(&headers);
    let mut body = leftover;
    let mut chunk = [0u8; 4096];
    while body.len() < want {
        let n = stream
            .read(&mut chunk)
            .await
            .expect("read response body bytes");
        assert!(n > 0, "unexpected EOF while reading response body");
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(want);
    (status, headers, body)
}

fn spawn_fake_dockerd(path: &Path, script: FakeScript) -> Arc<Mutex<Vec<RecordedRequest>>> {
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path).expect("bind fake dockerd socket");
    let recorded: Arc<Mutex<Vec<RecordedRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let rec = recorded.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _addr)) = listener.accept().await else {
                break;
            };
            let rec = rec.clone();
            let script = script.clone();
            tokio::spawn(async move {
                let (head, leftover) = read_head(&mut stream).await;
                let text = String::from_utf8(head).expect("UTF-8 request head");
                let mut lines = text.split("\r\n");
                let request_line = lines.next().expect("request line");
                let mut parts = request_line.split(' ');
                let method = parts.next().expect("method").to_owned();
                let path_and_query = parts.next().expect("request target").to_owned();
                let headers = parse_headers(lines);
                let want = content_length(&headers);
                let mut body = leftover;
                let mut chunk = [0u8; 65536];
                while body.len() < want {
                    let n = stream
                        .read(&mut chunk)
                        .await
                        .expect("read request body bytes");
                    assert!(n > 0, "unexpected EOF while reading request body");
                    body.extend_from_slice(&chunk[..n]);
                }
                body.truncate(want);
                rec.lock().expect("recorded requests lock").push(RecordedRequest {
                    method,
                    path_and_query,
                    headers,
                    body,
                });
                match script {
                    FakeScript::Canned(bytes) => {
                        stream
                            .write_all(&bytes)
                            .await
                            .expect("write canned response");
                        let mut sink = [0u8; 1];
                        let _ = stream.read(&mut sink).await;
                    }
                    FakeScript::UpgradeEcho => {
                        stream
                            .write_all(
                                b"HTTP/1.1 101 UPGRADED\r\n\
                                  Connection: Upgrade\r\n\
                                  Upgrade: tcp\r\n\r\n",
                            )
                            .await
                            .expect("write 101 head");
                        let mut echo = [0u8; 1024];
                        loop {
                            let n = stream.read(&mut echo).await.expect("echo read");
                            if n == 0 {
                                stream
                                    .write_all(b"late-data")
                                    .await
                                    .expect("write late data after client EOF");
                                break;
                            }
                            stream.write_all(&echo[..n]).await.expect("echo write");
                        }
                    }
                }
            });
        }
    });
    recorded
}

fn spawn_proxy(router: Router, sock_path: &Path) {
    let _ = std::fs::remove_file(sock_path);
    let listener = UnixListener::bind(sock_path).expect("bind proxy socket");
    tokio::spawn(async move {
        loop {
            let Ok((stream, _addr)) = listener.accept().await else {
                break;
            };
            let router = router.clone();
            tokio::spawn(async move {
                let service = service_fn(move |request| {
                    let router = router.clone();
                    async move { router.oneshot(request).await }
                });
                let io = TokioIo::new(stream);
                let builder = Builder::new(TokioExecutor::new());
                let _ = builder.serve_connection_with_upgrades(io, service).await;
            });
        }
    });
}

#[tokio::test]
async fn proxy_forwards_request_verbatim_d21() {
    let dir = unique_dir();
    let backend_sock = dir.join("backend.sock");
    let proxy_sock = dir.join("proxy.sock");

    let recorded = spawn_fake_dockerd(
        &backend_sock,
        FakeScript::Canned(
            b"HTTP/1.1 200 OK\r\n\
              Content-Type: application/json\r\n\
              Content-Length: 2\r\n\r\n\
              []"
                .to_vec(),
        ),
    );
    spawn_proxy(build_proxy_router(backend_sock, running_state()), &proxy_sock);

    let mut client = UnixStream::connect(&proxy_sock)
        .await
        .expect("connect to proxy socket");
    client
        .write_all(
            b"GET /v1.55/containers/json?all=1 HTTP/1.1\r\n\
              Host: docker\r\n\
              X-Speck-Test: 42\r\n\r\n",
        )
        .await
        .expect("write request");

    let (status, headers, body) = read_response(&mut client).await;
    assert_eq!(status, 200);
    assert_eq!(body, b"[]");
    assert!(
        headers
            .iter()
            .any(|(n, v)| n == "content-type" && v == "application/json")
    );

    let reqs = recorded.lock().expect("recorded requests lock");
    assert_eq!(reqs.len(), 1);
    let req = &reqs[0];
    assert_eq!(req.method, "GET");
    assert_eq!(req.path_and_query, "/v1.55/containers/json?all=1");
    assert_eq!(req.header("x-speck-test"), Some("42"));
    assert_eq!(req.header("host"), Some("docker"));
}

#[tokio::test]
async fn proxy_streams_body_larger_than_2mb() {
    const BODY_LEN: usize = 3 * 1024 * 1024;

    let dir = unique_dir();
    let backend_sock = dir.join("backend.sock");
    let proxy_sock = dir.join("proxy.sock");

    let recorded = spawn_fake_dockerd(
        &backend_sock,
        FakeScript::Canned(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec()),
    );
    spawn_proxy(build_proxy_router(backend_sock, running_state()), &proxy_sock);

    let mut client = UnixStream::connect(&proxy_sock)
        .await
        .expect("connect to proxy socket");
    client
        .write_all(
            format!(
                "POST /v1.55/build?t=big HTTP/1.1\r\n\
                 Host: docker\r\n\
                 Content-Type: application/x-tar\r\n\
                 Content-Length: {BODY_LEN}\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .expect("write request head");

    let (read_half, mut write_half) = client.into_split();
    let writer = tokio::spawn(async move {
        let body = vec![0xA5u8; BODY_LEN];
        let _ = write_half.write_all(&body).await;
        write_half
    });

    let mut read_half = read_half;
    let mut head = Vec::new();
    let mut chunk = [0u8; 4096];
    while find_double_crlf(&head).is_none() {
        let n = read_half.read(&mut chunk).await.expect("read response");
        assert!(n > 0, "unexpected EOF while reading response head");
        head.extend_from_slice(&chunk[..n]);
    }
    let text = String::from_utf8_lossy(&head);
    assert!(text.starts_with("HTTP/1.1 200"));
    let _ = writer.await;

    let reqs = recorded.lock().expect("recorded requests lock");
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].body.len(), BODY_LEN);
    assert!(reqs[0].body.iter().all(|&b| b == 0xA5));
}

#[tokio::test]
async fn proxy_joins_upgrade_bidirectionally() {
    let dir = unique_dir();
    let backend_sock = dir.join("backend.sock");
    let proxy_sock = dir.join("proxy.sock");

    let _recorded = spawn_fake_dockerd(&backend_sock, FakeScript::UpgradeEcho);
    spawn_proxy(build_proxy_router(backend_sock, running_state()), &proxy_sock);

    let mut client = UnixStream::connect(&proxy_sock)
        .await
        .expect("connect to proxy socket");
    client
        .write_all(
            b"POST /v1.55/exec/abc123/start HTTP/1.1\r\n\
              Host: docker\r\n\
              Connection: Upgrade\r\n\
              Upgrade: tcp\r\n\
              Content-Length: 0\r\n\r\n",
        )
        .await
        .expect("write upgrade request");

    let (head, mut pending) = read_head(&mut client).await;
    let head_text = String::from_utf8_lossy(&head).to_ascii_lowercase();
    assert!(head_text.starts_with("http/1.1 101"));
    assert!(head_text.contains("upgrade"));

    client.write_all(b"ping-1").await.expect("write ping");
    let mut chunk = [0u8; 256];
    while pending.len() < 6 {
        let n = client.read(&mut chunk).await.expect("read echo");
        assert!(n > 0, "unexpected EOF while waiting for echo");
        pending.extend_from_slice(&chunk[..n]);
    }
    assert_eq!(&pending[..6], b"ping-1");
    pending.drain(..6);

    client.shutdown().await.expect("client CloseWrite");

    let mut tail = pending;
    client
        .read_to_end(&mut tail)
        .await
        .expect("read after half-close");
    assert_eq!(tail, b"late-data");
}

#[tokio::test]
async fn proxy_returns_503_while_restarting() {
    let dir = unique_dir();
    let backend_sock = dir.join("missing-backend.sock");
    let proxy_sock = dir.join("proxy.sock");

    let vm_state = Arc::new(RwLock::new(VmState::Restarting));
    spawn_proxy(build_proxy_router(backend_sock, vm_state), &proxy_sock);

    let mut client = UnixStream::connect(&proxy_sock)
        .await
        .expect("connect to proxy socket");
    client
        .write_all(b"GET /v1.55/containers/json HTTP/1.1\r\nHost: docker\r\n\r\n")
        .await
        .expect("write request");

    let (status, headers, _body) = read_response(&mut client).await;
    assert_eq!(status, 503);
    assert!(
        headers.iter().any(|(n, v)| n == "retry-after" && !v.is_empty())
    );
}
