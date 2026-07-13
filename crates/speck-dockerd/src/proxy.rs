//! HTTP-aware passthrough proxy to the real dockerd running inside the guest.
//!
//! Locked decision D-21: the Docker API surface is a transparent proxy to the
//! real dockerd (`/run/speck/dockerd.sock` in the guest). Speck does not
//! reimplement Docker API endpoints against containerd.

use std::collections::HashMap;
use std::future::Future;
use std::io::Write;
use std::net::Ipv4Addr;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::task::{Context, Poll};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use http_body_util::BodyExt;
use hyper::body::{Bytes, Incoming};
use hyper::header::UPGRADE;
use hyper::upgrade;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use speck_core::VmState;
use speck_vz::{PortMapConfig, VolumeMountConfig};
use tokio::net::UnixStream;
use tower::Service;

use crate::DockerApiError;
use crate::middleware::strip_api_version::strip_version_prefix;
use crate::middleware::restart_503::RestartCheckLayer;

#[derive(Clone)]
struct ProxyState {
    internal_sock_path: Arc<PathBuf>,
    guest: Option<Arc<speck_vz::Guest>>,
    published_ports: Arc<Mutex<PublishedPortsState>>,
}

const TCP_FORWARDER_VSOCK_PORT: u32 = 9006;

#[derive(Debug)]
struct PublishedPortEntry {
    ports: Vec<PortMapConfig>,
    active: bool,
    forwards: Vec<HostPortForward>,
}

#[derive(Debug)]
struct HostPortForward {
    host_port: u16,
    stop: Arc<AtomicBool>,
}

#[derive(Default)]
struct PublishedPortsState {
    by_id: HashMap<String, PublishedPortEntry>,
    aliases: HashMap<String, String>,
}

#[derive(Clone)]
struct UnixConnector {
    sock_path: PathBuf,
}

impl UnixConnector {
    fn new(sock_path: PathBuf) -> Self {
        Self { sock_path }
    }
}

impl Service<hyper::Uri> for UnixConnector {
    type Response = TokioIo<UnixStream>;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _req: hyper::Uri) -> Self::Future {
        let sock_path = self.sock_path.clone();
        Box::pin(async move {
            let stream = UnixStream::connect(sock_path).await?;
            Ok(TokioIo::new(stream))
        })
    }
}

pub fn build_proxy_router(
    internal_sock_path: PathBuf,
    guest: Option<Arc<speck_vz::Guest>>,
    vm_state: Arc<RwLock<VmState>>,
) -> Router {
    let state = ProxyState {
        internal_sock_path: Arc::new(internal_sock_path),
        guest,
        published_ports: Arc::new(Mutex::new(PublishedPortsState::default())),
    };

    Router::new()
        .fallback(passthrough)
        .with_state(state)
        .layer(DefaultBodyLimit::disable())
        .layer(RestartCheckLayer::new(vm_state))
}

async fn passthrough(State(state): State<ProxyState>, mut req: Request) -> crate::Result<Response> {
    let request_path = req.uri().path().to_owned();
    let stripped_path = strip_version_prefix(&request_path).unwrap_or(&request_path);
    tracing::info!(method = %req.method(), path = stripped_path, "proxy request");

    if req.method() == hyper::Method::POST && stripped_path == "/containers/create" {
        return intercept_container_create(state, req).await;
    }
    if req.method() == hyper::Method::POST
        && let Some(id) = extract_container_id(stripped_path, "/containers/", "/start")
    {
        tracing::info!(container = %id, "routing request into intercept_container_start");
        return intercept_container_start(state, req, id.to_owned()).await;
    }
    if req.method() == hyper::Method::POST
        && let Some(id) = extract_container_id(stripped_path, "/containers/", "/stop")
    {
        return intercept_container_stop(state, req, id.to_owned()).await;
    }
    if req.method() == hyper::Method::DELETE
        && let Some(id) = extract_container_id(stripped_path, "/containers/", "")
    {
        return intercept_container_delete(state, req, id.to_owned()).await;
    }

    let wants_upgrade = req.headers().contains_key(UPGRADE);
    let client_upgrade = wants_upgrade.then(|| upgrade::on(&mut req));

    rewrite_proxy_uri(&mut req)?;

    let client = Client::builder(TokioExecutor::new())
        .build(UnixConnector::new((*state.internal_sock_path).clone()));
    let mut backend_resp = client.request(req).await.map_err(|err| {
        DockerApiError::Internal(format!(
            "proxy request to guest dockerd over {} failed: {err}",
            state.internal_sock_path.display()
        ))
    })?;

    let backend_upgrade = (wants_upgrade && backend_resp.status() == StatusCode::SWITCHING_PROTOCOLS)
        .then(|| upgrade::on(&mut backend_resp));
    let response = response_from_backend(backend_resp);

    if let (Some(client_upgrade), Some(backend_upgrade)) = (client_upgrade, backend_upgrade) {
        tokio::spawn(async move {
            let client_upgraded = match client_upgrade.await {
                Ok(upgraded) => upgraded,
                Err(err) => {
                    tracing::warn!(?err, "client upgrade failed in passthrough proxy");
                    return;
                }
            };
            let backend_upgraded = match backend_upgrade.await {
                Ok(upgraded) => upgraded,
                Err(err) => {
                    tracing::warn!(?err, "backend upgrade failed in passthrough proxy");
                    return;
                }
            };

            let mut client_io = TokioIo::new(client_upgraded);
            let mut backend_io = TokioIo::new(backend_upgraded);
            if let Err(err) = tokio::io::copy_bidirectional(&mut client_io, &mut backend_io).await {
                tracing::debug!(?err, "upgraded passthrough tunnel ended with error");
            }
        });
    }

    Ok(response)
}

async fn intercept_container_create(
    state: ProxyState,
    req: Request,
) -> crate::Result<Response> {
    let query_name = query_value(req.uri().query(), "name");
    let (parts, body) = req.into_parts();
    let body_bytes = to_bytes(body, usize::MAX)
        .await
        .map_err(|err| DockerApiError::Internal(format!("read create body: {err}")))?;

    let mut port_maps = Vec::new();
    let mut bind_mounts = Vec::new();
    if let Ok(create) = serde_json::from_slice::<CreateBody>(&body_bytes)
        && let Some(host_config) = create.host_config
    {
        port_maps = extract_port_maps(host_config.port_bindings);
        bind_mounts = extract_bind_mounts(host_config.binds)?;
    } else {
        tracing::debug!("container create interception could not parse HostConfig payload");
    }

    tracing::info!(
        query_name = ?query_name,
        port_maps = port_maps.len(),
        bind_mounts = bind_mounts.len(),
        "intercepted container create"
    );

    let forward_req = Request::from_parts(parts, Body::from(body_bytes));
    let backend_resp = forward_request(&state, forward_req).await?;
    let (parts, body_bytes) = collect_response(backend_resp).await?;

    if parts.status == StatusCode::CREATED
        && let Ok(create_resp) = serde_json::from_slice::<CreateResponse>(&body_bytes)
    {
        if !port_maps.is_empty() {
            remember_published_ports(&state, create_resp.id.clone(), query_name, port_maps, false);
        }
        if !bind_mounts.is_empty()
            && let Some(guest) = state.guest.as_ref()
            && let Err(err) = guest.add_bind_mounts(bind_mounts)
        {
            tracing::warn!(error = %err, container_id = %create_resp.id, "failed to update bind mounts during create interception");
        }
    }

    Ok(Response::from_parts(parts, Body::from(body_bytes)))
}

async fn intercept_container_start(
    state: ProxyState,
    req: Request,
    id_or_name: String,
) -> crate::Result<Response> {
    tracing::info!(container = %id_or_name, "intercept container start before forward");
    let response = forward_plain(&state, req).await?;
    tracing::info!(container = %id_or_name, "intercept container start after forward");
    tracing::info!(status = %response.status(), container = %id_or_name, "intercepted container start response");
    if response.status() == StatusCode::NO_CONTENT && let Some(guest) = state.guest.as_ref() {
        let Some(container_ip) = wait_for_container_ip(&state, &id_or_name).await? else {
            return Ok(response);
        };
        let ports = activate_published_ports(&state, &id_or_name, container_ip);
        tracing::info!(container = %id_or_name, port_count = ports.len(), "published ports activated for container");
        for port in ports {
            match spawn_host_port_forward(guest.clone(), port) {
                Ok(forward) => register_host_port_forward(&state, &id_or_name, forward),
                Err(err) => {
                tracing::warn!(error = %err, host_port = port.host_port, container_port = port.container_port, container = %id_or_name, target_ip = ?port.target_ip, "failed to spawn published port forwarder");
                }
            }
        }
    }
    Ok(response)
}

async fn intercept_container_stop(
    state: ProxyState,
    req: Request,
    id_or_name: String,
) -> crate::Result<Response> {
    let response = forward_plain(&state, req).await?;
    tracing::info!(status = %response.status(), container = %id_or_name, "intercepted container stop response");
    if response.status().is_success() {
        stop_host_port_forwards(&state, &id_or_name);
    }
    Ok(response)
}

async fn intercept_container_delete(
    state: ProxyState,
    req: Request,
    id_or_name: String,
) -> crate::Result<Response> {
    let response = forward_plain(&state, req).await?;
    if response.status().is_success() {
        stop_host_port_forwards(&state, &id_or_name);
        forget_published_ports(&state, &id_or_name);
    }
    Ok(response)
}

async fn forward_plain(state: &ProxyState, req: Request) -> crate::Result<Response> {
    let backend_resp = forward_request(state, req).await?;
    Ok(response_from_backend(backend_resp))
}

async fn forward_request(state: &ProxyState, mut req: Request) -> crate::Result<hyper::Response<Incoming>> {
    rewrite_proxy_uri(&mut req)?;
    let client = Client::builder(TokioExecutor::new())
        .build(UnixConnector::new((*state.internal_sock_path).clone()));
    client.request(req).await.map_err(|err| {
        DockerApiError::Internal(format!(
            "proxy request to guest dockerd over {} failed: {err}",
            state.internal_sock_path.display()
        ))
    })
}

async fn collect_response(
    resp: hyper::Response<Incoming>,
) -> crate::Result<(hyper::http::response::Parts, Bytes)> {
    let (parts, body) = resp.into_parts();
    let body_bytes = body
        .collect()
        .await
        .map_err(|err| DockerApiError::Internal(format!("read proxied response body: {err}")))?
        .to_bytes();
    Ok((parts, body_bytes))
}

fn rewrite_proxy_uri(req: &mut Request) -> crate::Result<()> {
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");
    let uri = format!("http://localhost{path_and_query}")
        .parse()
        .map_err(|err| DockerApiError::Internal(format!("invalid proxied URI: {err}")))?;
    *req.uri_mut() = uri;
    Ok(())
}

fn response_from_backend(resp: hyper::Response<Incoming>) -> Response {
    let (parts, body) = resp.into_parts();
    Response::from_parts(parts, Body::new(body))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CreateBody {
    #[serde(default)]
    host_config: Option<CreateHostConfig>,
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
struct CreateHostConfig {
    #[serde(default)]
    port_bindings: Option<HashMap<String, Vec<PortBindingBody>>>,
    #[serde(default)]
    binds: Option<Vec<String>>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PortBindingBody {
    #[serde(default)]
    host_port: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CreateResponse {
    id: String,
}

#[derive(Debug, Clone)]
struct DockerBind {
    host_path: PathBuf,
    container_path: PathBuf,
    read_only: bool,
}

fn extract_port_maps(
    port_bindings: Option<HashMap<String, Vec<PortBindingBody>>>,
) -> Vec<PortMapConfig> {
    let mut result = Vec::new();
    let Some(port_bindings) = port_bindings else {
        return result;
    };
    for (port_proto, host_bindings) in port_bindings {
        let Some(container_port) = port_proto
            .split('/')
            .next()
            .and_then(|s| s.parse::<u16>().ok())
        else {
            continue;
        };
        for binding in host_bindings {
            let Some(host_port) = binding.host_port.as_deref().and_then(|s| s.parse().ok()) else {
                continue;
            };
            // In the transparent-proxy architecture dockerd still performs the
            // guest-side publish/NAT. The host netstack only needs to reach the
            // published port on the guest, so the target port is HostPort, not
            // the container's inner port from the Docker API key.
            result.push(PortMapConfig {
                host_port,
                container_port,
                target_ip: None,
            });
        }
    }
    result
}

fn extract_bind_mounts(binds: Option<Vec<String>>) -> crate::Result<Vec<VolumeMountConfig>> {
    let Some(binds) = binds else {
        return Ok(Vec::new());
    };
    binds
        .into_iter()
        .map(|bind| parse_docker_bind(&bind))
        .map(|parsed| {
            parsed.map(|bind| VolumeMountConfig {
                host_path: bind.host_path,
                container_path: bind.container_path,
                read_only: bind.read_only,
                volume_name: None,
            })
        })
        .collect()
}

fn parse_docker_bind(s: &str) -> crate::Result<DockerBind> {
    let parts: Vec<&str> = s.splitn(3, ':').collect();
    let (host, container, read_only) = match parts.as_slice() {
        [host, container] => (*host, *container, false),
        [host, container, mode] => {
            let read_only = match *mode {
                "ro" => true,
                "rw" => false,
                other => {
                    return Err(DockerApiError::BadRequest(format!(
                        "invalid bind mount mode '{other}'; expected 'ro' or 'rw'"
                    )));
                }
            };
            (*host, *container, read_only)
        }
        _ => {
            return Err(DockerApiError::BadRequest(
                "bind mount must be in format 'host_path:container_path[:ro|:rw]'".into(),
            ));
        }
    };

    if host.is_empty() || container.is_empty() {
        return Err(DockerApiError::BadRequest("bind mount paths cannot be empty".into()));
    }
    let host_path = PathBuf::from(host);
    if !host_path.is_absolute() || !host_path.exists() {
        return Err(DockerApiError::BadRequest(format!("bind host path invalid: {host}")));
    }
    let container_path = PathBuf::from(container);
    if !container_path.is_absolute() || container.contains("..") {
        return Err(DockerApiError::BadRequest(format!(
            "bind container path invalid: {container}"
        )));
    }

    Ok(DockerBind {
        host_path,
        container_path,
        read_only,
    })
}

fn query_value(query: Option<&str>, key: &str) -> Option<String> {
    query?
        .split('&')
        .find_map(|pair| pair.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| v.to_string()))
}

fn extract_container_id<'a>(path: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    let rest = path.strip_prefix(prefix)?;
    if suffix.is_empty() {
        return (!rest.is_empty() && !rest.contains('/')).then_some(rest);
    }
    let id = rest.strip_suffix(suffix)?;
    (!id.is_empty() && !id.contains('/')).then_some(id)
}

fn remember_published_ports(
    state: &ProxyState,
    id: String,
    alias: Option<String>,
    ports: Vec<PortMapConfig>,
    active: bool,
) {
    let mut published = state.published_ports.lock().expect("published ports lock poisoned");
    published.by_id.insert(
        id.clone(),
        PublishedPortEntry {
            ports,
            active,
            forwards: Vec::new(),
        },
    );
    if let Some(alias) = alias {
        published.aliases.insert(alias, id);
    }
}

fn activate_published_ports(
    state: &ProxyState,
    id_or_name: &str,
    container_ip: Ipv4Addr,
) -> Vec<PortMapConfig> {
    let mut published = state.published_ports.lock().expect("published ports lock poisoned");
    let Some(id) = resolve_container_id(&published, id_or_name) else {
        return Vec::new();
    };
    let Some(entry) = published.by_id.get_mut(&id) else {
        return Vec::new();
    };
    if entry.active {
        return Vec::new();
    }
    entry.active = true;
    let activated: Vec<PortMapConfig> = entry
        .ports
        .iter()
        .map(|port| PortMapConfig {
            host_port: port.host_port,
            container_port: port.container_port,
            target_ip: Some(container_ip),
        })
        .collect();
    entry.ports = activated.clone();

    for port in &entry.ports {
        tracing::info!(
            container = %id_or_name,
            host_port = port.host_port,
            container_port = port.container_port,
            target_ip = %container_ip,
            "activating published port"
        );
    }
    activated
}

fn forget_published_ports(state: &ProxyState, id_or_name: &str) -> Vec<PortMapConfig> {
    let mut published = state.published_ports.lock().expect("published ports lock poisoned");
    let Some(id) = resolve_container_id(&published, id_or_name) else {
        return Vec::new();
    };
    published.aliases.retain(|_, value| value != &id);
    published
        .by_id
        .remove(&id)
        .map(|entry| entry.ports)
        .unwrap_or_default()
}

fn stop_host_port_forwards(state: &ProxyState, id_or_name: &str) {
    let published = &mut *state.published_ports.lock().expect("published ports lock poisoned");
    let Some(id) = resolve_container_id(published, id_or_name) else {
        return;
    };
    if let Some(entry) = published.by_id.get_mut(&id) {
        entry.active = false;
        for forward in entry.forwards.drain(..) {
            tracing::info!(host_port = forward.host_port, container = %id_or_name, "stopping published port forwarder");
            forward.stop.store(true, Ordering::SeqCst);
            let _ = std::net::TcpStream::connect(("127.0.0.1", forward.host_port));
        }
    }
}

fn register_host_port_forward(state: &ProxyState, id_or_name: &str, forward: HostPortForward) {
    let published = &mut *state.published_ports.lock().expect("published ports lock poisoned");
    let Some(id) = resolve_container_id(published, id_or_name) else {
        return;
    };
    if let Some(entry) = published.by_id.get_mut(&id) {
        entry.forwards.push(forward);
    }
}

fn spawn_host_port_forward(guest: Arc<speck_vz::Guest>, config: PortMapConfig) -> std::io::Result<HostPortForward> {
    let target_ip = config.target_ip.expect("activated port map must have target_ip");
    let listener = TcpListener::bind(("127.0.0.1", config.host_port))?;
    listener.set_nonblocking(true)?;
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = Arc::clone(&stop);

    std::thread::spawn(move || {
        loop {
            if stop_for_thread.load(Ordering::SeqCst) {
                break;
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    let guest = Arc::clone(&guest);
                    std::thread::spawn(move || {
                        if let Err(err) = bridge_host_to_guest_tcp(guest, target_ip, config.container_port, stream) {
                            tracing::warn!(error = %err, host_port = config.host_port, target_ip = %target_ip, target_port = config.container_port, "published port connection failed");
                        }
                    });
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(err) => {
                    tracing::warn!(error = %err, host_port = config.host_port, "published port accept loop failed");
                    break;
                }
            }
        }
    });

    Ok(HostPortForward {
        host_port: config.host_port,
        stop,
    })
}

fn bridge_host_to_guest_tcp(
    guest: Arc<speck_vz::Guest>,
    target_ip: Ipv4Addr,
    target_port: u16,
    host_stream: TcpStream,
) -> std::io::Result<()> {
    let mut guest_stream = guest
        .vsock_connect(TCP_FORWARDER_VSOCK_PORT)
        .map_err(|err| std::io::Error::other(err.to_string()))?
        .into_unix_stream();
    guest_stream.write_all(format!("{target_ip}:{target_port}\n").as_bytes())?;

    let mut host_reader = host_stream.try_clone()?;
    let mut host_writer = host_stream;
    let mut guest_writer = guest_stream.try_clone()?;

    let h2g = std::thread::spawn(move || {
        let _ = std::io::copy(&mut host_reader, &mut guest_writer);
        let _ = guest_writer.shutdown(Shutdown::Write);
    });

    let _ = std::io::copy(&mut guest_stream, &mut host_writer);
    let _ = host_writer.shutdown(Shutdown::Write);
    let _ = h2g.join();
    Ok(())
}

fn resolve_container_id(published: &PublishedPortsState, id_or_name: &str) -> Option<String> {
    if published.by_id.contains_key(id_or_name) {
        Some(id_or_name.to_owned())
    } else {
        published.aliases.get(id_or_name).cloned()
    }
}

async fn inspect_container_ip(state: &ProxyState, id: &str) -> crate::Result<Option<Ipv4Addr>> {
    let uri: hyper::Uri = format!("http://localhost/containers/{id}/json")
        .parse()
        .map_err(|err| DockerApiError::Internal(format!("invalid inspect URI: {err}")))?;
    let req = Request::builder()
        .method(hyper::Method::GET)
        .uri(uri)
        .body(Body::empty())
        .map_err(|err| DockerApiError::Internal(format!("build inspect request: {err}")))?;
    let backend_resp = forward_request(state, req).await?;
    let (_parts, body_bytes) = collect_response(backend_resp).await?;
    let inspect: serde_json::Value = serde_json::from_slice(&body_bytes)
        .map_err(|err| DockerApiError::Internal(format!("parse inspect response: {err}")))?;

    if let Some(networks) = inspect
        .get("NetworkSettings")
        .and_then(|settings| settings.get("Networks"))
        .and_then(|networks| networks.as_object())
    {
        for network in networks.values() {
            if let Some(ip) = network
                .get("IPAddress")
                .and_then(|value| value.as_str())
                .and_then(|ip| ip.parse::<Ipv4Addr>().ok())
            {
                return Ok(Some(ip));
            }
        }
    }

    Ok(inspect
        .get("NetworkSettings")
        .and_then(|settings| settings.get("IPAddress"))
        .and_then(|value| value.as_str())
        .and_then(|ip| ip.parse::<Ipv4Addr>().ok()))
}

async fn wait_for_container_ip(state: &ProxyState, id: &str) -> crate::Result<Option<Ipv4Addr>> {
    for _ in 0..30 {
        if let Some(ip) = inspect_container_ip(state, id).await? {
            tracing::info!(container = %id, target_ip = %ip, "resolved container IP for published-port activation");
            return Ok(Some(ip));
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    tracing::warn!(container = %id, "container IP not available after start; skipping published-port activation");
    Ok(None)
}
