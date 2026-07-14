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
    Sequential(Arc<Mutex<Vec<Vec<u8>>>>),
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
                    FakeScript::Sequential(responses) => {
                        let bytes = {
                            let mut vec = responses.lock().expect("sequential responses lock");
                            if vec.is_empty() {
                                panic!("sequential fake dockerd ran out of responses");
                            }
                            vec.remove(0)
                        };
                        stream
                            .write_all(&bytes)
                            .await
                            .expect("write sequential response");
                        let mut sink = [0u8; 1];
                        let _ = stream.read(&mut sink).await;
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
    spawn_proxy(build_proxy_router(backend_sock, None, running_state()), &proxy_sock);

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
async fn create_rewrites_bind_sources_for_guest_path() {
    let dir = unique_dir();
    let backend_sock = dir.join("backend.sock");
    let proxy_sock = dir.join("proxy.sock");
    let host_dir = std::env::temp_dir().join(format!("spk-bind-src-{}", std::process::id()));
    std::fs::create_dir_all(&host_dir).expect("create host bind source");

    let recorded = spawn_fake_dockerd(
        &backend_sock,
        FakeScript::Canned(
            b"HTTP/1.1 201 CREATED\r\n\
              Content-Type: application/json\r\n\
              Content-Length: 16\r\n\r\n\
              {\"Id\":\"ctr-123\"}"
                .to_vec(),
        ),
    );
    spawn_proxy(build_proxy_router(backend_sock, None, running_state()), &proxy_sock);

    let create_body = serde_json::json!({
        "Image": "alpine",
        "HostConfig": {
            "Binds": [
                format!("{}:/var/data:ro", host_dir.display()),
                format!("{}:/var/cache:rw", host_dir.display()),
            ]
        }
    })
    .to_string();

    let mut client = UnixStream::connect(&proxy_sock)
        .await
        .expect("connect to proxy socket");
    client
        .write_all(
            format!(
                "POST /v1.55/containers/create?name=bind-rewrite HTTP/1.1\r\n\
                 Host: docker\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\r\n{}",
                create_body.len(),
                create_body
            )
            .as_bytes(),
        )
        .await
        .expect("write create request");

    let (status, _headers, body) = read_response(&mut client).await;
    assert_eq!(status, 201);
    assert_eq!(body, br#"{"Id":"ctr-123"}"#);

    let reqs = recorded.lock().expect("recorded requests lock");
    assert_eq!(reqs.len(), 1);
    let req = &reqs[0];
    assert_eq!(req.method, "POST");
    assert_eq!(req.path_and_query, "/v1.55/containers/create?name=bind-rewrite");

    let forwarded: serde_json::Value =
        serde_json::from_slice(&req.body).expect("forwarded create body as json");
    let binds = forwarded["HostConfig"]["Binds"]
        .as_array()
        .expect("forwarded binds array");
    let bind_specs: Vec<&str> = binds
        .iter()
        .map(|bind| bind.as_str().expect("bind spec string"))
        .collect();
    let canonical_host_dir = std::fs::canonicalize(&host_dir).expect("canonical host bind source");
    assert_eq!(bind_specs.len(), 2);
    assert_eq!(bind_specs[0], format!("{}:/var/data:ro", canonical_host_dir.display()));
    assert_eq!(bind_specs[1], format!("{}:/var/cache:rw", canonical_host_dir.display()));
}

#[tokio::test]
async fn create_uses_distinct_bind_namespaces_for_same_container_path() {
    let dir = unique_dir();
    let backend_sock = dir.join("backend.sock");
    let proxy_sock = dir.join("proxy.sock");
    let host_a = dir.join("host-a");
    let host_b = dir.join("host-b");
    std::fs::create_dir_all(&host_a).expect("create first host bind source");
    std::fs::create_dir_all(&host_b).expect("create second host bind source");

    let recorded = spawn_fake_dockerd(
        &backend_sock,
        FakeScript::Canned(
            b"HTTP/1.1 201 CREATED\r\n\
              Content-Type: application/json\r\n\
              Content-Length: 16\r\n\r\n\
              {\"Id\":\"ctr-123\"}"
                .to_vec(),
        ),
    );
    spawn_proxy(build_proxy_router(backend_sock, None, running_state()), &proxy_sock);

    for (index, host_dir) in [host_a, host_b].into_iter().enumerate() {
        let create_body = serde_json::json!({
            "Image": "alpine",
            "HostConfig": {
                "Binds": [format!("{}:/var/data:ro", host_dir.display())]
            }
        })
        .to_string();

        let mut client = UnixStream::connect(&proxy_sock)
            .await
            .expect("connect to proxy socket");
        client
            .write_all(
                format!(
                    "POST /v1.55/containers/create?case={index} HTTP/1.1\r\n\
                     Host: docker\r\n\
                     Content-Type: application/json\r\n\
                     Content-Length: {}\r\n\r\n{}",
                    create_body.len(),
                    create_body
                )
                .as_bytes(),
            )
            .await
            .expect("write create request");

    let (status, _headers, body) = read_response(&mut client).await;
    assert_eq!(status, 201);
    assert_eq!(body, br#"{"Id":"ctr-123"}"#);
    }

    let reqs = recorded.lock().expect("recorded requests lock");
    assert_eq!(reqs.len(), 2);
    let canonical_host_a = std::fs::canonicalize(dir.join("host-a")).expect("canonical first host bind source");
    let canonical_host_b = std::fs::canonicalize(dir.join("host-b")).expect("canonical second host bind source");
    let first: serde_json::Value = serde_json::from_slice(&reqs[0].body).expect("first create body");
    let second: serde_json::Value = serde_json::from_slice(&reqs[1].body).expect("second create body");
    let first_bind = first["HostConfig"]["Binds"][0]
        .as_str()
        .expect("first bind string");
    let second_bind = second["HostConfig"]["Binds"][0]
        .as_str()
        .expect("second bind string");
    assert_ne!(first_bind, second_bind, "same container target must not reuse a global bind share name");
    assert_eq!(first_bind, format!("{}:/var/data:ro", canonical_host_a.display()));
    assert_eq!(second_bind, format!("{}:/var/data:ro", canonical_host_b.display()));
}

#[tokio::test]
async fn create_preserves_bind_suffix_options_during_rewrite() {
    let dir = unique_dir();
    let backend_sock = dir.join("backend.sock");
    let proxy_sock = dir.join("proxy.sock");
    let host_dir = dir.join("host-with-options");
    std::fs::create_dir_all(&host_dir).expect("create host bind source");

    let recorded = spawn_fake_dockerd(
        &backend_sock,
        FakeScript::Canned(
            b"HTTP/1.1 201 CREATED\r\n\
              Content-Type: application/json\r\n\
              Content-Length: 17\r\n\r\n\
              {\"Id\":\"ctr-opts\"}"
                .to_vec(),
        ),
    );
    spawn_proxy(build_proxy_router(backend_sock, None, running_state()), &proxy_sock);

    let create_body = serde_json::json!({
        "Image": "alpine",
        "HostConfig": {
            "Binds": [format!("{}:/var/data:ro,z,rshared", host_dir.display())]
        }
    })
    .to_string();

    let mut client = UnixStream::connect(&proxy_sock)
        .await
        .expect("connect to proxy socket");
    client
        .write_all(
            format!(
                "POST /v1.55/containers/create HTTP/1.1\r\n\
                 Host: docker\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\r\n{}",
                create_body.len(),
                create_body
            )
            .as_bytes(),
        )
        .await
        .expect("write create request");

    let (status, _headers, body) = read_response(&mut client).await;
    assert_eq!(status, 201);
    assert_eq!(body, br#"{"Id":"ctr-opts"}"#);

    let reqs = recorded.lock().expect("recorded requests lock");
    assert_eq!(reqs.len(), 1);
    let canonical_host_dir = std::fs::canonicalize(&host_dir).expect("canonical host bind source");
    let forwarded: serde_json::Value =
        serde_json::from_slice(&reqs[0].body).expect("forwarded create body as json");
    let bind = forwarded["HostConfig"]["Binds"][0]
        .as_str()
        .expect("forwarded bind spec string");
    assert_eq!(
        bind,
        format!("{}:/var/data:ro,z,rshared", canonical_host_dir.display()),
        "bind suffix/options must be preserved verbatim after source rewrite: {bind}"
    );
}

#[tokio::test]
async fn create_preserves_named_volume_bind_specs_while_rewriting_host_binds() {
    let dir = unique_dir();
    let backend_sock = dir.join("backend.sock");
    let proxy_sock = dir.join("proxy.sock");
    let host_dir = dir.join("host-bind-src");
    std::fs::create_dir_all(&host_dir).expect("create host bind source");

    let recorded = spawn_fake_dockerd(
        &backend_sock,
        FakeScript::Canned(
            b"HTTP/1.1 201 CREATED\r\n\
              Content-Type: application/json\r\n\
              Content-Length: 18\r\n\r\n\
              {\"Id\":\"ctr-mixed\"}"
                .to_vec(),
        ),
    );
    spawn_proxy(build_proxy_router(backend_sock, None, running_state()), &proxy_sock);

    let create_body = serde_json::json!({
        "Image": "alpine",
        "HostConfig": {
            "Binds": [
                "cache:/var/cache:rw",
                format!("{}:/var/data:ro,z", host_dir.display())
            ]
        }
    })
    .to_string();

    let mut client = UnixStream::connect(&proxy_sock)
        .await
        .expect("connect to proxy socket");
    client
        .write_all(
            format!(
                "POST /v1.55/containers/create HTTP/1.1\r\n\
                 Host: docker\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\r\n{}",
                create_body.len(),
                create_body
            )
            .as_bytes(),
        )
        .await
        .expect("write create request");

    let (status, _headers, body) = read_response(&mut client).await;
    assert_eq!(status, 201);
    assert_eq!(body, br#"{"Id":"ctr-mixed"}"#);

    let reqs = recorded.lock().expect("recorded requests lock");
    let forwarded: serde_json::Value =
        serde_json::from_slice(&reqs[0].body).expect("forwarded create body as json");
    let binds = forwarded["HostConfig"]["Binds"]
        .as_array()
        .expect("forwarded binds array");
    assert_eq!(binds[0].as_str(), Some("cache:/var/cache:rw"));
    let canonical_host_dir = std::fs::canonicalize(&host_dir).expect("canonical host bind source");
    let rewritten = binds[1].as_str().expect("rewritten host bind string");
    assert_eq!(rewritten, format!("{}:/var/data:ro,z", canonical_host_dir.display()));
}

#[tokio::test]
async fn create_rejects_missing_bind_host_path_before_forwarding() {
    let dir = unique_dir();
    let backend_sock = dir.join("backend.sock");
    let proxy_sock = dir.join("proxy.sock");
    let missing_host_dir = dir.join("missing-host-dir");

    let recorded = spawn_fake_dockerd(
        &backend_sock,
        FakeScript::Canned(b"HTTP/1.1 500 INTERNAL SERVER ERROR\r\nContent-Length: 0\r\n\r\n".to_vec()),
    );
    spawn_proxy(build_proxy_router(backend_sock, None, running_state()), &proxy_sock);

    let create_body = serde_json::json!({
        "Image": "alpine",
        "HostConfig": {
            "Binds": [format!("{}:/var/data:ro", missing_host_dir.display())]
        }
    })
    .to_string();

    let mut client = UnixStream::connect(&proxy_sock)
        .await
        .expect("connect to proxy socket");
    client
        .write_all(
            format!(
                "POST /v1.55/containers/create HTTP/1.1\r\n\
                 Host: docker\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\r\n{}",
                create_body.len(),
                create_body
            )
            .as_bytes(),
        )
        .await
        .expect("write create request");

    let (status, _headers, body) = read_response(&mut client).await;
    let body_text = String::from_utf8(body).expect("error body utf8");
    assert_eq!(status, 400);
    assert!(body_text.contains("bind host path invalid"));

    let reqs = recorded.lock().expect("recorded requests lock");
    assert!(reqs.is_empty(), "invalid bind create must not be forwarded");
}

#[tokio::test]
async fn create_rejects_invalid_bind_specs_with_400() {
    let dir = unique_dir();
    let backend_sock = dir.join("backend.sock");
    let proxy_sock = dir.join("proxy.sock");
    let valid_host_dir = std::env::temp_dir().join(format!("spk-bind-valid-{}", std::process::id()));
    std::fs::create_dir_all(&valid_host_dir).expect("create valid host bind source");

    let recorded = spawn_fake_dockerd(
        &backend_sock,
        FakeScript::Canned(b"HTTP/1.1 500 INTERNAL SERVER ERROR\r\nContent-Length: 0\r\n\r\n".to_vec()),
    );
    spawn_proxy(build_proxy_router(backend_sock, None, running_state()), &proxy_sock);

    let cases = vec![
        (
            "relative host path",
            "./relative-host:/var/data:ro".to_string(),
            "bind host path invalid",
        ),
        (
            "relative container path",
            format!("{}:var/data:ro", valid_host_dir.display()),
            "bind container path invalid",
        ),
        (
            "container path traversal",
            format!("{}:/var/../data:ro", valid_host_dir.display()),
            "bind container path invalid",
        ),
    ];

    for (index, (name, bind, expected_error)) in cases.into_iter().enumerate() {
        let create_body = serde_json::json!({
            "Image": "alpine",
            "HostConfig": {
                "Binds": [bind]
            }
        })
        .to_string();

        let mut client = UnixStream::connect(&proxy_sock)
            .await
            .expect("connect to proxy socket");
        client
            .write_all(
                format!(
                    "POST /v1.55/containers/create?case={index} HTTP/1.1\r\n\
                     Host: docker\r\n\
                     Content-Type: application/json\r\n\
                     Content-Length: {}\r\n\r\n{}",
                    create_body.len(),
                    create_body
                )
                .as_bytes(),
            )
            .await
            .expect("write create request");

        let (status, _headers, body) = read_response(&mut client).await;
        let body_text = String::from_utf8(body).expect("error body utf8");
        assert_eq!(status, 400, "case {name} should be rejected");
        assert!(
            body_text.contains(expected_error),
            "case {name} expected error containing {expected_error:?}, got {body_text:?}"
        );
    }

    let reqs = recorded.lock().expect("recorded requests lock");
    assert!(
        reqs.is_empty(),
        "invalid bind create requests must never reach the backend"
    );

    let _ = std::fs::remove_dir_all(&valid_host_dir);
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
    spawn_proxy(build_proxy_router(backend_sock, None, running_state()), &proxy_sock);

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
    spawn_proxy(build_proxy_router(backend_sock, None, running_state()), &proxy_sock);

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
    spawn_proxy(build_proxy_router(backend_sock, None, vm_state), &proxy_sock);

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

// ---- Inspect HostIp rewriting (MW-03) ----

#[tokio::test]
async fn inspect_rewrites_published_port_host_ip() {
    let dir = unique_dir();
    let backend_sock = dir.join("backend.sock");
    let proxy_sock = dir.join("proxy.sock");

    let inspect_response = serde_json::json!({
        "Id": "ctr-abc",
        "NetworkSettings": {
            "Ports": {
                "80/tcp": [
                    {"HostIp": "0.0.0.0", "HostPort": "18081"}
                ]
            }
        }
    });
    let inspect_response_bytes = inspect_response.to_string().into_bytes();

    let _recorded = spawn_fake_dockerd(
        &backend_sock,
        FakeScript::Sequential(Arc::new(Mutex::new(vec![
            b"HTTP/1.1 201 CREATED\r\n\
              Content-Type: application/json\r\n\
              Content-Length: 16\r\n\r\n\
              {\"Id\":\"ctr-abc\"}"
                .to_vec(),
            format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\r\n{}",
                inspect_response_bytes.len(),
                String::from_utf8_lossy(&inspect_response_bytes)
            )
            .into_bytes(),
        ]))),
    );
    spawn_proxy(build_proxy_router(backend_sock, None, running_state()), &proxy_sock);

    // Step 1: create container with published port to register it in proxy state
    let create_body = serde_json::json!({
        "Image": "nginx:alpine",
        "HostConfig": {
            "PortBindings": {
                "80/tcp": [{"HostIp": "", "HostPort": "18081"}]
            }
        }
    })
    .to_string();

    let mut client = UnixStream::connect(&proxy_sock)
        .await
        .expect("connect to proxy socket");
    client
        .write_all(
            format!(
                "POST /v1.55/containers/create?name=inspect-rewrite HTTP/1.1\r\n\
                 Host: docker\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\r\n{}",
                create_body.len(),
                create_body
            )
            .as_bytes(),
        )
        .await
        .expect("write create request");

    let (status, _headers, body) = read_response(&mut client).await;
    assert_eq!(status, 201);
    assert_eq!(body, br#"{"Id":"ctr-abc"}"#);

    // Step 2: inspect the container — proxy should rewrite HostIp to 127.0.0.1
    let mut client = UnixStream::connect(&proxy_sock)
        .await
        .expect("connect to proxy socket for inspect");
    client
        .write_all(b"GET /v1.55/containers/ctr-abc/json HTTP/1.1\r\nHost: docker\r\n\r\n")
        .await
        .expect("write inspect request");

    let (status, _headers, body) = read_response(&mut client).await;
    assert_eq!(status, 200);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("inspect response as json");
    let host_ip = json["NetworkSettings"]["Ports"]["80/tcp"][0]["HostIp"]
        .as_str()
        .expect("HostIp string");
    assert_eq!(host_ip, "127.0.0.1", "HostIp must be rewritten to 127.0.0.1");
    let host_port = json["NetworkSettings"]["Ports"]["80/tcp"][0]["HostPort"]
        .as_str()
        .expect("HostPort string");
    assert_eq!(host_port, "18081", "HostPort must be preserved");
}

#[tokio::test]
async fn inspect_passthrough_without_published_ports() {
    let dir = unique_dir();
    let backend_sock = dir.join("backend.sock");
    let proxy_sock = dir.join("proxy.sock");

    let inspect_response = serde_json::json!({
        "Id": "ctr-no-ports",
        "NetworkSettings": {
            "Ports": {}
        }
    });
    let inspect_response_bytes = inspect_response.to_string().into_bytes();

    let _recorded = spawn_fake_dockerd(
        &backend_sock,
        FakeScript::Canned(
            format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\r\n{}",
                inspect_response_bytes.len(),
                String::from_utf8_lossy(&inspect_response_bytes)
            )
            .into_bytes(),
        ),
    );
    spawn_proxy(build_proxy_router(backend_sock, None, running_state()), &proxy_sock);

    let mut client = UnixStream::connect(&proxy_sock)
        .await
        .expect("connect to proxy socket");
    client
        .write_all(b"GET /v1.55/containers/ctr-no-ports/json HTTP/1.1\r\nHost: docker\r\n\r\n")
        .await
        .expect("write inspect request");

    let (status, _headers, body) = read_response(&mut client).await;
    assert_eq!(status, 200);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("inspect response as json");
    assert_eq!(
        json, inspect_response,
        "inspect for container without published ports must pass through untouched"
    );
}

#[tokio::test]
async fn list_rewrites_published_port_ip() {
    let dir = unique_dir();
    let backend_sock = dir.join("backend.sock");
    let proxy_sock = dir.join("proxy.sock");

    let list_response = serde_json::json!([
        {
            "Id": "ctr-with-ports",
            "Names": ["/web"],
            "Ports": [
                {"PrivatePort": 80, "PublicPort": 18081, "Type": "tcp", "IP": "0.0.0.0"}
            ]
        },
        {
            "Id": "ctr-no-ports",
            "Names": ["/db"],
            "Ports": []
        }
    ]);
    let list_response_bytes = list_response.to_string().into_bytes();

    let create_response = b"{\"Id\":\"ctr-with-ports\"}";
    let _recorded = spawn_fake_dockerd(
        &backend_sock,
        FakeScript::Sequential(Arc::new(Mutex::new(vec![
            format!(
                "HTTP/1.1 201 CREATED\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\r\n{}",
                create_response.len(),
                String::from_utf8_lossy(create_response)
            )
            .into_bytes(),
            format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\r\n{}",
                list_response_bytes.len(),
                String::from_utf8_lossy(&list_response_bytes)
            )
            .into_bytes(),
        ]))),
    );
    spawn_proxy(build_proxy_router(backend_sock, None, running_state()), &proxy_sock);

    // Step 1: create container with published port
    let create_body = serde_json::json!({
        "Image": "nginx:alpine",
        "HostConfig": {
            "PortBindings": {
                "80/tcp": [{"HostIp": "", "HostPort": "18081"}]
            }
        }
    })
    .to_string();

    let mut client = UnixStream::connect(&proxy_sock)
        .await
        .expect("connect to proxy socket");
    client
        .write_all(
            format!(
                "POST /v1.55/containers/create?name=list-rewrite HTTP/1.1\r\n\
                 Host: docker\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\r\n{}",
                create_body.len(),
                create_body
            )
            .as_bytes(),
        )
        .await
        .expect("write create request");

    let (status, _headers, _body) = read_response(&mut client).await;
    assert_eq!(status, 201);

    // Step 2: list containers — proxy should rewrite IP for ctr-with-ports only
    let mut client = UnixStream::connect(&proxy_sock)
        .await
        .expect("connect to proxy socket for list");
    client
        .write_all(b"GET /v1.55/containers/json HTTP/1.1\r\nHost: docker\r\n\r\n")
        .await
        .expect("write list request");

    let (status, _headers, body) = read_response(&mut client).await;
    assert_eq!(status, 200);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("list response as json");
    let arr = json.as_array().expect("list response array");
    assert_eq!(arr.len(), 2);

    let with_ports = &arr[0];
    assert_eq!(with_ports["Id"], "ctr-with-ports");
    let ports = with_ports["Ports"].as_array().expect("ports array");
    assert_eq!(ports[0]["IP"], "127.0.0.1", "IP must be rewritten to 127.0.0.1");
    assert_eq!(ports[0]["PublicPort"], 18081, "PublicPort must be preserved");

    let no_ports = &arr[1];
    assert_eq!(no_ports["Id"], "ctr-no-ports");
    assert_eq!(
        no_ports["Ports"],
        serde_json::json!([]),
        "container without published ports must pass through untouched"
    );
}

#[tokio::test]
async fn list_passthrough_without_published_ports() {
    let dir = unique_dir();
    let backend_sock = dir.join("backend.sock");
    let proxy_sock = dir.join("proxy.sock");

    let list_response = serde_json::json!([
        {
            "Id": "ctr-no-ports",
            "Names": ["/db"],
            "Ports": []
        }
    ]);
    let list_response_bytes = list_response.to_string().into_bytes();

    let _recorded = spawn_fake_dockerd(
        &backend_sock,
        FakeScript::Canned(
            format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\r\n{}",
                list_response_bytes.len(),
                String::from_utf8_lossy(&list_response_bytes)
            )
            .into_bytes(),
        ),
    );
    spawn_proxy(build_proxy_router(backend_sock, None, running_state()), &proxy_sock);

    let mut client = UnixStream::connect(&proxy_sock)
        .await
        .expect("connect to proxy socket");
    client
        .write_all(b"GET /v1.55/containers/json HTTP/1.1\r\nHost: docker\r\n\r\n")
        .await
        .expect("write list request");

    let (status, _headers, body) = read_response(&mut client).await;
    assert_eq!(status, 200);
    let json: serde_json::Value = serde_json::from_slice(&body).expect("list response as json");
    assert_eq!(
        json, list_response,
        "list for containers without published ports must pass through untouched"
    );
}
