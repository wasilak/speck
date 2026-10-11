//! HTTP-aware passthrough proxy to the real dockerd running inside the guest.
//!
//! Locked decision D-21: the Docker API surface is a transparent proxy to the
//! real dockerd (`/run/speck/dockerd.sock` in the guest). Speck does not
//! reimplement Docker API endpoints against containerd.

use std::collections::HashMap;
use std::future::Future;
use std::io::ErrorKind;
use std::net::Ipv4Addr;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::task::{Context, Poll};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use http_body_util::BodyExt;
use hyper::Method;
use hyper::body::{Bytes, Incoming};
use hyper::header::{CONTENT_LENGTH, UPGRADE};
use hyper::upgrade;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use speck_core::VmState;
use speck_vz::{
    PortMapConfig, VolumeMountConfig, bind_mount_guest_source_path,
    bind_mount_guest_source_path_for_namespace,
};
use tokio::net::UnixStream;
use tower::Service;

use crate::DockerApiError;
use crate::middleware::restart_503::RestartCheckLayer;
use crate::middleware::strip_api_version::strip_version_prefix;

/// Host-side behaviour switches for the allowlisted middleware.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProxyOptions {
    /// Bind the guest CA bundle (system roots + Speck-injected corporate CAs)
    /// into every new container and point common CA env vars at it (TLS-03).
    pub inject_ca_into_containers: bool,
}

#[derive(Clone)]
struct ProxyState {
    internal_sock_path: Arc<PathBuf>,
    options: ProxyOptions,
    guest: Option<Arc<speck_vz::Guest>>,
    published_ports: Arc<Mutex<PublishedPortsState>>,
    bind_mounts: Arc<Mutex<BindMountState>>,
    next_bind_namespace: Arc<AtomicU64>,
}

const TCP_FORWARDER_VSOCK_PORT: u32 = 9006;

#[derive(Debug)]
struct PublishedPortEntry {
    ports: Vec<PortMapConfig>,
    active: bool,
    forwards: Vec<HostPortForward>,
}

struct HostPortForward {
    host_port: u16,
    stop: Arc<AtomicBool>,
    join: std::thread::JoinHandle<()>,
}

impl std::fmt::Debug for HostPortForward {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostPortForward")
            .field("host_port", &self.host_port)
            .field("stop", &self.stop.load(Ordering::SeqCst))
            .finish()
    }
}

#[derive(Default)]
struct PublishedPortsState {
    by_id: HashMap<String, PublishedPortEntry>,
    aliases: HashMap<String, String>,
}

#[derive(Default)]
struct BindMountState {
    by_id: HashMap<String, Vec<VolumeMountConfig>>,
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
    build_proxy_router_with_options(internal_sock_path, guest, vm_state, ProxyOptions::default())
}

pub fn build_proxy_router_with_options(
    internal_sock_path: PathBuf,
    guest: Option<Arc<speck_vz::Guest>>,
    vm_state: Arc<RwLock<VmState>>,
    options: ProxyOptions,
) -> Router {
    let state = ProxyState {
        internal_sock_path: Arc::new(internal_sock_path),
        options,
        guest,
        published_ports: Arc::new(Mutex::new(PublishedPortsState::default())),
        bind_mounts: Arc::new(Mutex::new(BindMountState::default())),
        next_bind_namespace: Arc::new(AtomicU64::new(1)),
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
    if req.method() == hyper::Method::GET
        && let Some(id) = extract_container_id(stripped_path, "/containers/", "/json")
    {
        return intercept_container_inspect(state, req, id.to_owned()).await;
    }
    if req.method() == hyper::Method::GET && stripped_path == "/containers/json" {
        return intercept_container_list(state, req).await;
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

    let backend_upgrade = (wants_upgrade
        && backend_resp.status() == StatusCode::SWITCHING_PROTOCOLS)
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

async fn intercept_container_create(state: ProxyState, req: Request) -> crate::Result<Response> {
    let query_name = query_value(req.uri().query(), "name");
    let (parts, body) = req.into_parts();
    let body_bytes = to_bytes(body, usize::MAX)
        .await
        .map_err(|err| DockerApiError::Internal(format!("read create body: {err}")))?;

    let mut port_maps = Vec::new();
    let mut parsed_binds = Vec::new();
    if let Ok(create) = serde_json::from_slice::<CreateBody>(&body_bytes)
        && let Some(host_config) = create.host_config
    {
        port_maps = extract_port_maps(host_config.port_bindings);
        parsed_binds = extract_bind_mounts(host_config.binds)?;
    } else {
        tracing::debug!("container create interception could not parse HostConfig payload");
    }

    let bind_namespace = parsed_binds
        .iter()
        .any(|bind| matches!(bind, DockerMountSpec::HostBind(_)))
        .then(|| allocate_bind_namespace(&state));
    let bind_mounts: Vec<VolumeMountConfig> = parsed_binds
        .iter()
        .filter_map(|bind| match bind {
            DockerMountSpec::HostBind(bind) => Some(VolumeMountConfig {
                host_path: bind.host_path.clone(),
                container_path: bind.container_path.clone(),
                read_only: bind.read_only,
                volume_name: None,
                runtime_namespace: bind_namespace.clone(),
            }),
            DockerMountSpec::Passthrough(_) => None,
        })
        .collect();

    tracing::info!(
        query_name = ?query_name,
        port_maps = port_maps.len(),
        bind_mounts = bind_mounts.len(),
        "intercepted container create"
    );

    let forward_body_bytes = if bind_mounts.is_empty() {
        body_bytes
    } else {
        Bytes::from(rewrite_create_body_bind_mounts(
            &body_bytes,
            &parsed_binds,
            bind_namespace.as_deref(),
        )?)
    };
    let forward_body_bytes = if state.options.inject_ca_into_containers {
        Bytes::from(inject_ca_bundle(&forward_body_bytes)?)
    } else {
        forward_body_bytes
    };

    let mut parts = parts;
    parts.headers.insert(
        CONTENT_LENGTH,
        hyper::header::HeaderValue::from_str(&forward_body_bytes.len().to_string())
            .map_err(|err| DockerApiError::Internal(format!("set create body length: {err}")))?,
    );

    let forward_req = Request::from_parts(parts, Body::from(forward_body_bytes));
    let backend_resp = forward_request(&state, forward_req).await?;
    let (parts, body_bytes) = collect_response(backend_resp).await?;

    if parts.status == StatusCode::CREATED
        && let Ok(create_resp) = serde_json::from_slice::<CreateResponse>(&body_bytes)
    {
        if !port_maps.is_empty() {
            remember_published_ports(
                &state,
                create_resp.id.clone(),
                query_name.clone(),
                port_maps,
                false,
            );
        }
        if !bind_mounts.is_empty() {
            remember_bind_mounts(
                &state,
                create_resp.id.clone(),
                query_name,
                bind_mounts.clone(),
            );
            if let Some(guest) = state.guest.as_ref()
                && let Err(err) = guest.add_bind_mounts(bind_mounts)
            {
                forget_bind_mounts(&state, &create_resp.id);
                forget_published_ports(&state, &create_resp.id);
                best_effort_remove_container(&state, &create_resp.id).await;
                return Err(DockerApiError::Internal(format!(
                    "failed to register bind mounts for container {}: {err}",
                    create_resp.id
                )));
            }
        }
    }

    Ok(Response::from_parts(parts, Body::from(body_bytes)))
}

/// Guest trust bundle: system roots plus CAs vminitd installed (TLS-01).
const GUEST_CA_BUNDLE: &str = "/etc/ssl/certs/ca-certificates.crt";
/// Where common distros look for the system CA bundle.
const CONTAINER_CA_PATHS: &[&str] = &[
    "/etc/ssl/certs/ca-certificates.crt", // Debian, Ubuntu, Alpine
    "/etc/pki/tls/certs/ca-bundle.crt",   // RHEL, Fedora, Amazon Linux
];
/// CA env vars honoured by OpenSSL, Python requests and Node.js.
const CA_ENV_VARS: &[&str] = &["SSL_CERT_FILE", "REQUESTS_CA_BUNDLE", "NODE_EXTRA_CA_CERTS"];

/// Add read-only binds of the guest CA bundle and CA env vars to a container
/// create body. The bind sources are guest paths, so they bypass the host →
/// VirtioFS bind translation. Paths and variables the user already set are
/// left alone.
fn inject_ca_bundle(body_bytes: &[u8]) -> crate::Result<Vec<u8>> {
    let mut create: serde_json::Value = serde_json::from_slice(body_bytes)
        .map_err(|err| DockerApiError::Internal(format!("parse create body for CA: {err}")))?;
    let root = create
        .as_object_mut()
        .ok_or_else(|| DockerApiError::Internal("create body is not an object".into()))?;

    let env = root
        .entry("Env")
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    if env.is_null() {
        *env = serde_json::Value::Array(Vec::new());
    }
    if let Some(env) = env.as_array_mut() {
        for var in CA_ENV_VARS {
            let prefix = format!("{var}=");
            let set = env
                .iter()
                .any(|e| e.as_str().is_some_and(|e| e.starts_with(&prefix)));
            if !set {
                env.push(format!("{prefix}{GUEST_CA_BUNDLE}").into());
            }
        }
    }

    let host_config = root
        .entry("HostConfig")
        .or_insert_with(|| serde_json::Value::Object(Default::default()));
    if let Some(host_config) = host_config.as_object_mut() {
        let binds = host_config
            .entry("Binds")
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
        if binds.is_null() {
            *binds = serde_json::Value::Array(Vec::new());
        }
        if let Some(binds) = binds.as_array_mut() {
            for target in CONTAINER_CA_PATHS {
                let taken = binds.iter().any(|b| {
                    b.as_str()
                        .and_then(|b| b.split(':').nth(1))
                        .is_some_and(|t| t == *target)
                });
                if !taken {
                    binds.push(format!("{GUEST_CA_BUNDLE}:{target}:ro").into());
                }
            }
        }
    }

    serde_json::to_vec(&create)
        .map_err(|err| DockerApiError::Internal(format!("serialize create body with CA: {err}")))
}

fn rewrite_create_body_bind_mounts(
    body_bytes: &[u8],
    bind_mounts: &[DockerMountSpec],
    namespace: Option<&str>,
) -> crate::Result<Vec<u8>> {
    let mut create_value: serde_json::Value = serde_json::from_slice(body_bytes)
        .map_err(|err| DockerApiError::Internal(format!("parse create body for rewrite: {err}")))?;

    let host_config = create_value
        .get_mut("HostConfig")
        .and_then(|value| value.as_object_mut())
        .ok_or_else(|| {
            DockerApiError::Internal(
                "container create interception missing HostConfig during bind rewrite".into(),
            )
        })?;

    host_config.insert(
        "Binds".to_string(),
        serde_json::Value::Array(
            bind_mounts
                .iter()
                .map(|bind| serde_json::Value::String(render_rewritten_bind_mount(bind, namespace)))
                .collect(),
        ),
    );

    serde_json::to_vec(&create_value)
        .map_err(|err| DockerApiError::Internal(format!("serialize rewritten create body: {err}")))
}

fn render_rewritten_bind_mount(bind: &DockerMountSpec, namespace: Option<&str>) -> String {
    match bind {
        DockerMountSpec::HostBind(bind) => {
            let guest_source = guest_identity_source_path(&bind.host_path).unwrap_or_else(|| {
                namespace
                    .map(|value| {
                        bind_mount_guest_source_path_for_namespace(value, &bind.container_path)
                    })
                    .unwrap_or_else(|| bind_mount_guest_source_path(&bind.container_path))
            });
            if bind.suffix.is_empty() {
                format!(
                    "{}:{}",
                    guest_source.display(),
                    bind.container_path.display()
                )
            } else {
                format!(
                    "{}:{}:{}",
                    guest_source.display(),
                    bind.container_path.display(),
                    bind.suffix
                )
            }
        }
        DockerMountSpec::Passthrough(bind) => bind.clone(),
    }
}

fn guest_identity_source_path(host_path: &PathBuf) -> Option<PathBuf> {
    let canonical = std::fs::canonicalize(host_path).ok()?;
    for root in ["/Users", "/Volumes", "/private/tmp", "/private/var"] {
        let root = PathBuf::from(root);
        if canonical == root || canonical.starts_with(&root) {
            return Some(canonical);
        }
    }
    None
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
    if response.status() == StatusCode::NO_CONTENT
        && let Some(guest) = state.guest.as_ref()
    {
        if !has_published_ports(&state, &id_or_name) {
            return Ok(response);
        }
        let Some(container_ip) = wait_for_container_ip(&state, &id_or_name).await? else {
            best_effort_stop_container(&state, &id_or_name).await;
            return Err(DockerApiError::Internal(format!(
                "failed to resolve published-port target IP for started container {id_or_name}"
            )));
        };
        let ports = activate_published_ports(&state, &id_or_name, container_ip);
        tracing::info!(container = %id_or_name, port_count = ports.len(), "published ports activated for container");
        for port in ports {
            match spawn_host_port_forward(guest.clone(), port) {
                Ok(forward) => register_host_port_forward(&state, &id_or_name, forward),
                Err(err) => {
                    stop_host_port_forwards(&state, &id_or_name);
                    best_effort_stop_container(&state, &id_or_name).await;
                    return Err(DockerApiError::Internal(format!(
                        "failed to expose published port {} for container {id_or_name}: {err}",
                        port.host_port,
                    )));
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
        let removed_binds = forget_bind_mounts(&state, &id_or_name);
        if !removed_binds.is_empty()
            && let Some(guest) = state.guest.as_ref()
            && let Err(err) = guest.remove_bind_mounts(removed_binds)
        {
            tracing::warn!(error = %err, container = %id_or_name, "failed to remove bind mounts during delete interception");
        }
    }
    Ok(response)
}

async fn intercept_container_inspect(
    state: ProxyState,
    req: Request,
    id_or_name: String,
) -> crate::Result<Response> {
    let backend_resp = forward_request(&state, req).await?;
    let (mut parts, body_bytes) = collect_response(backend_resp).await?;

    let should_rewrite = {
        let published = state
            .published_ports
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        resolve_container_id(&published, &id_or_name)
            .and_then(|id| published.by_id.get(&id))
            .map(|entry| !entry.ports.is_empty())
            .unwrap_or(false)
    };

    tracing::info!(container = %id_or_name, should_rewrite, "intercept_container_inspect");

    let response_bytes = if !should_rewrite {
        body_bytes.to_vec()
    } else {
        match serde_json::from_slice::<serde_json::Value>(&body_bytes) {
            Ok(mut json) => {
                if let Some(ports) = json
                    .get_mut("NetworkSettings")
                    .and_then(|ns| ns.get_mut("Ports"))
                    .and_then(|p| p.as_object_mut())
                {
                    for bindings in ports.values_mut() {
                        if let Some(arr) = bindings.as_array_mut() {
                            for binding in arr {
                                if let Some(obj) = binding.as_object_mut() {
                                    obj.insert(
                                        "HostIp".to_string(),
                                        serde_json::Value::String("127.0.0.1".to_string()),
                                    );
                                }
                            }
                        }
                    }
                }
                match serde_json::to_vec(&json) {
                    Ok(v) => v,
                    Err(err) => {
                        tracing::warn!(error = %err, "failed to serialize rewritten inspect response; passing through");
                        body_bytes.to_vec()
                    }
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "failed to parse inspect response for HostIp rewrite; passing through");
                body_bytes.to_vec()
            }
        }
    };

    parts.headers.remove(CONTENT_LENGTH);
    parts.headers.remove("transfer-encoding");
    parts.headers.insert(
        CONTENT_LENGTH,
        hyper::header::HeaderValue::from_str(&response_bytes.len().to_string()).map_err(|err| {
            DockerApiError::Internal(format!("set inspect response body length: {err}"))
        })?,
    );

    Ok(Response::from_parts(parts, Body::from(response_bytes)))
}

async fn intercept_container_list(state: ProxyState, req: Request) -> crate::Result<Response> {
    let backend_resp = forward_request(&state, req).await?;
    let (mut parts, body_bytes) = collect_response(backend_resp).await?;

    let response_bytes = match serde_json::from_slice::<serde_json::Value>(&body_bytes) {
        Ok(mut json) => {
            let published_ids: std::collections::HashSet<String> = {
                let published = state
                    .published_ports
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                published.by_id.keys().cloned().collect()
            };

            if let Some(arr) = json.as_array_mut() {
                for item in arr {
                    let has_ports = item
                        .get("Id")
                        .and_then(|v| v.as_str())
                        .map(|id| published_ids.contains(id))
                        .unwrap_or(false);
                    if has_ports
                        && let Some(ports) = item.get_mut("Ports").and_then(|p| p.as_array_mut())
                    {
                        for port in ports {
                            if let Some(obj) = port.as_object_mut() {
                                obj.insert(
                                    "IP".to_string(),
                                    serde_json::Value::String("127.0.0.1".to_string()),
                                );
                            }
                        }
                    }
                }
            }

            match serde_json::to_vec(&json) {
                Ok(v) => v,
                Err(err) => {
                    tracing::warn!(error = %err, "failed to serialize rewritten list response; passing through");
                    body_bytes.to_vec()
                }
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, "failed to parse list response for IP rewrite; passing through");
            body_bytes.to_vec()
        }
    };

    parts.headers.remove(CONTENT_LENGTH);
    parts.headers.remove("transfer-encoding");
    parts.headers.insert(
        CONTENT_LENGTH,
        hyper::header::HeaderValue::from_str(&response_bytes.len().to_string()).map_err(|err| {
            DockerApiError::Internal(format!("set list response body length: {err}"))
        })?,
    );

    Ok(Response::from_parts(parts, Body::from(response_bytes)))
}

async fn forward_plain(state: &ProxyState, req: Request) -> crate::Result<Response> {
    let backend_resp = forward_request(state, req).await?;
    Ok(response_from_backend(backend_resp))
}

async fn forward_request(
    state: &ProxyState,
    mut req: Request,
) -> crate::Result<hyper::Response<Incoming>> {
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

async fn best_effort_remove_container(state: &ProxyState, id: &str) {
    let uri = format!("http://localhost/containers/{id}?force=1");
    let request = match Request::builder()
        .method(Method::DELETE)
        .uri(uri)
        .body(Body::empty())
    {
        Ok(request) => request,
        Err(err) => {
            tracing::warn!(error = %err, container = %id, "failed to build cleanup remove request");
            return;
        }
    };
    match forward_request(state, request).await {
        Ok(response)
            if response.status().is_success() || response.status() == StatusCode::NOT_FOUND => {}
        Ok(response) => {
            tracing::warn!(status = %response.status(), container = %id, "cleanup remove request failed");
        }
        Err(err) => {
            tracing::warn!(error = %err, container = %id, "cleanup remove request errored");
        }
    }
}

async fn best_effort_stop_container(state: &ProxyState, id_or_name: &str) {
    let uri = format!("http://localhost/containers/{id_or_name}/stop?t=0");
    let request = match Request::builder()
        .method(Method::POST)
        .uri(uri)
        .body(Body::empty())
    {
        Ok(request) => request,
        Err(err) => {
            tracing::warn!(error = %err, container = %id_or_name, "failed to build cleanup stop request");
            return;
        }
    };
    match forward_request(state, request).await {
        Ok(response)
            if response.status().is_success()
                || response.status() == StatusCode::NOT_MODIFIED
                || response.status() == StatusCode::NOT_FOUND => {}
        Ok(response) => {
            tracing::warn!(status = %response.status(), container = %id_or_name, "cleanup stop request failed");
        }
        Err(err) => {
            tracing::warn!(error = %err, container = %id_or_name, "cleanup stop request errored");
        }
    }
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
    suffix: String,
    read_only: bool,
}

#[derive(Debug, Clone)]
enum DockerMountSpec {
    HostBind(DockerBind),
    Passthrough(String),
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

fn extract_bind_mounts(binds: Option<Vec<String>>) -> crate::Result<Vec<DockerMountSpec>> {
    let Some(binds) = binds else {
        return Ok(Vec::new());
    };
    binds
        .into_iter()
        .map(|bind| parse_docker_bind(&bind))
        .collect()
}

fn parse_docker_bind(s: &str) -> crate::Result<DockerMountSpec> {
    let parts: Vec<&str> = s.splitn(3, ':').collect();
    let (host, container, suffix) = match parts.as_slice() {
        [host, container] => (*host, *container, ""),
        [host, container, suffix] => (*host, *container, *suffix),
        _ => {
            return Err(DockerApiError::BadRequest(
                "bind mount must be in format 'host_path:container_path[:ro|:rw]'".into(),
            ));
        }
    };

    if host.is_empty() || container.is_empty() {
        return Err(DockerApiError::BadRequest(
            "bind mount paths cannot be empty".into(),
        ));
    }
    let container_path = PathBuf::from(container);
    if !container_path.is_absolute() || container.contains("..") {
        return Err(DockerApiError::BadRequest(format!(
            "bind container path invalid: {container}"
        )));
    }
    let host_path = PathBuf::from(host);
    if !host_path.is_absolute() {
        if host.contains('/') || host.starts_with('.') {
            return Err(DockerApiError::BadRequest(format!(
                "bind host path invalid: {host}"
            )));
        }
        return Ok(DockerMountSpec::Passthrough(s.to_owned()));
    }
    if !host_path.exists() {
        return Err(DockerApiError::BadRequest(format!(
            "bind host path invalid: {host}"
        )));
    }
    let read_only = suffix.split(',').any(|option| option == "ro");

    Ok(DockerMountSpec::HostBind(DockerBind {
        host_path,
        container_path,
        suffix: suffix.to_owned(),
        read_only,
    }))
}

fn query_value(query: Option<&str>, key: &str) -> Option<String> {
    query?.split('&').find_map(|pair| {
        pair.split_once('=')
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| v.to_string())
    })
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
    let mut published = state
        .published_ports
        .lock()
        .unwrap_or_else(|e| e.into_inner());
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

fn remember_bind_mounts(
    state: &ProxyState,
    id: String,
    alias: Option<String>,
    binds: Vec<VolumeMountConfig>,
) {
    let mut bind_mounts = state.bind_mounts.lock().unwrap_or_else(|e| e.into_inner());
    bind_mounts.by_id.insert(id.clone(), binds);
    if let Some(alias) = alias {
        bind_mounts.aliases.insert(alias, id);
    }
}

fn has_published_ports(state: &ProxyState, id_or_name: &str) -> bool {
    let published = state
        .published_ports
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    resolve_container_id(&published, id_or_name)
        .and_then(|id| published.by_id.get(&id))
        .map(|entry| !entry.ports.is_empty())
        .unwrap_or(false)
}

fn activate_published_ports(
    state: &ProxyState,
    id_or_name: &str,
    container_ip: Ipv4Addr,
) -> Vec<PortMapConfig> {
    let mut published = state
        .published_ports
        .lock()
        .unwrap_or_else(|e| e.into_inner());
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
    let mut published = state
        .published_ports
        .lock()
        .unwrap_or_else(|e| e.into_inner());
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

fn forget_bind_mounts(state: &ProxyState, id_or_name: &str) -> Vec<VolumeMountConfig> {
    let mut bind_mounts = state.bind_mounts.lock().unwrap_or_else(|e| e.into_inner());
    let Some(id) = resolve_bind_mount_container_id(&bind_mounts, id_or_name) else {
        return Vec::new();
    };
    bind_mounts.aliases.retain(|_, value| value != &id);
    bind_mounts.by_id.remove(&id).unwrap_or_default()
}

fn stop_host_port_forwards(state: &ProxyState, id_or_name: &str) {
    let published = &mut *state
        .published_ports
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(id) = resolve_container_id(published, id_or_name) else {
        return;
    };
    if let Some(entry) = published.by_id.get_mut(&id) {
        entry.active = false;
        for forward in entry.forwards.drain(..) {
            tracing::info!(host_port = forward.host_port, container = %id_or_name, "stopping published port forwarder");
            forward.stop.store(true, Ordering::SeqCst);
            let _ = std::net::TcpStream::connect(("127.0.0.1", forward.host_port));
            if let Err(err) = forward.join.join() {
                tracing::debug!(?err, host_port = forward.host_port, container = %id_or_name, "published port forwarder thread panicked during shutdown");
            }
        }
    }
}

fn register_host_port_forward(state: &ProxyState, id_or_name: &str, forward: HostPortForward) {
    let published = &mut *state
        .published_ports
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(id) = resolve_container_id(published, id_or_name) else {
        return;
    };
    if let Some(entry) = published.by_id.get_mut(&id) {
        entry.forwards.push(forward);
    }
}

/// Concurrent connections per published port; extra connections are refused.
const MAX_FORWARDS_PER_PORT: usize = 256;
/// How long the accept loop blocks before re-checking its stop flag.
const ACCEPT_POLL_MS: libc::c_int = 250;

fn spawn_host_port_forward(
    guest: Arc<speck_vz::Guest>,
    config: PortMapConfig,
) -> std::io::Result<HostPortForward> {
    let target_ip = config.target_ip.ok_or_else(|| {
        std::io::Error::other(format!(
            "published port {} has no target_ip",
            config.host_port
        ))
    })?;
    // Bind IPv6 loopback too: `localhost` may resolve to ::1 first.
    let mut listeners = vec![TcpListener::bind(("127.0.0.1", config.host_port))?];
    match TcpListener::bind(("::1", config.host_port)) {
        Ok(v6) => listeners.push(v6),
        Err(err) => {
            tracing::debug!(error = %err, host_port = config.host_port, "published port not bound on ::1")
        }
    }
    for listener in &listeners {
        listener.set_nonblocking(true)?;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = Arc::clone(&stop);
    let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let join = std::thread::spawn(move || {
        let mut pfds: Vec<libc::pollfd> = listeners
            .iter()
            .map(|l| libc::pollfd {
                fd: std::os::unix::io::AsRawFd::as_raw_fd(l),
                events: libc::POLLIN,
                revents: 0,
            })
            .collect();
        'accept: loop {
            if stop_for_thread.load(Ordering::SeqCst) {
                break;
            }
            // Block until a connection arrives (or the timeout lets us see
            // the stop flag) instead of sleep-polling the listeners.
            let ready = unsafe {
                libc::poll(
                    pfds.as_mut_ptr(),
                    pfds.len() as libc::nfds_t,
                    ACCEPT_POLL_MS,
                )
            };
            if ready <= 0 {
                continue;
            }
            for (listener, pfd) in listeners.iter().zip(pfds.iter_mut()) {
                if pfd.revents == 0 {
                    continue;
                }
                pfd.revents = 0;
                match listener.accept() {
                    Ok((stream, _)) => {
                        if stop_for_thread.load(Ordering::SeqCst) {
                            break 'accept;
                        }
                        if active.load(Ordering::SeqCst) >= MAX_FORWARDS_PER_PORT {
                            tracing::warn!(
                                host_port = config.host_port,
                                "published port at connection limit; refusing"
                            );
                            continue;
                        }
                        active.fetch_add(1, Ordering::SeqCst);
                        let guest = Arc::clone(&guest);
                        let active = Arc::clone(&active);
                        std::thread::spawn(move || {
                            if let Err(err) = bridge_host_to_guest_tcp(
                                guest,
                                target_ip,
                                config.container_port,
                                stream,
                            ) {
                                tracing::warn!(error = %err, host_port = config.host_port, target_ip = %target_ip, target_port = config.container_port, "published port connection failed");
                            }
                            active.fetch_sub(1, Ordering::SeqCst);
                        });
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(err) => {
                        tracing::warn!(error = %err, host_port = config.host_port, "published port accept loop failed");
                        break 'accept;
                    }
                }
            }
        }
    });

    Ok(HostPortForward {
        host_port: config.host_port,
        stop,
        join,
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
    write_all_retrying_wouldblock(
        &mut guest_stream,
        format!("{target_ip}:{target_port}\n").as_bytes(),
    )?;

    let mut host_reader = host_stream.try_clone()?;
    let mut host_writer = host_stream;
    let mut guest_writer = guest_stream.try_clone()?;

    let h2g = std::thread::spawn(move || {
        let _ = copy_retrying_wouldblock(&mut host_reader, &mut guest_writer);
        let _ = guest_writer.shutdown(Shutdown::Write);
    });

    let _ = copy_retrying_wouldblock(&mut guest_stream, &mut host_writer);
    let _ = host_writer.shutdown(Shutdown::Write);
    let _ = h2g.join();
    Ok(())
}

/// Copy bytes from `src` to `dst` until EOF, retrying `WouldBlock` on both
/// reads and writes by waiting for fd readiness via `poll()` instead of
/// treating it as fatal.
///
/// VZ's vsock connection fd (`VZVirtioSocketConnection.fileDescriptor`,
/// reached here via `guest.vsock_connect()`) can return `EWOULDBLOCK` on
/// `write()` even when `fcntl(F_GETFL)` reports no `O_NONBLOCK` — it does
/// not behave like an ordinary blocking POSIX socket (confirmed directly:
/// debug session buildx-http2-frame-too-large). `std::io::copy` treats any
/// `WouldBlock` as a fatal error and aborts immediately without retrying,
/// silently dropping whatever hadn't been written yet and triggering a
/// premature half-close of the bridged connection — the same invariant
/// (#4: "never let a blocking downstream write stall reads from a vsock
/// fd... any new vsock consumer must drain eagerly into userspace memory")
/// that `speck-vz::guest::write_chunk_blocking` already protects against
/// for a different vsock consumer. This mirrors that established pattern.
fn copy_retrying_wouldblock<R, W>(src: &mut R, dst: &mut W) -> std::io::Result<u64>
where
    R: std::io::Read + std::os::unix::io::AsRawFd,
    W: std::io::Write + std::os::unix::io::AsRawFd,
{
    let mut buf = [0u8; 65536];
    let mut total = 0u64;
    loop {
        let n = match src.read(&mut buf) {
            Ok(0) => return Ok(total),
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                wait_for_fd(src.as_raw_fd(), libc::POLLIN)?;
                continue;
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        write_all_retrying_wouldblock(dst, &buf[..n])?;
        total += n as u64;
    }
}

/// Write `data` fully to `dst`, retrying on `WouldBlock` by waiting for
/// writability via `poll()`. See [`copy_retrying_wouldblock`] for why this
/// is required for VZ vsock fds.
fn write_all_retrying_wouldblock<W: std::io::Write + std::os::unix::io::AsRawFd>(
    dst: &mut W,
    mut data: &[u8],
) -> std::io::Result<()> {
    while !data.is_empty() {
        match dst.write(data) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    ErrorKind::WriteZero,
                    "bridge_host_to_guest_tcp: downstream write returned 0",
                ));
            }
            Ok(n) => data = &data[n..],
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                wait_for_fd(dst.as_raw_fd(), libc::POLLOUT)?;
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn wait_for_fd(fd: std::os::unix::io::RawFd, events: libc::c_short) -> std::io::Result<()> {
    loop {
        let mut pfd = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        let ret = unsafe { libc::poll(&mut pfd, 1, -1) };
        if ret >= 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        if e.kind() != ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

fn resolve_container_id(published: &PublishedPortsState, id_or_name: &str) -> Option<String> {
    resolve_registered_container_id(&published.by_id, &published.aliases, id_or_name)
}

fn resolve_bind_mount_container_id(
    bind_mounts: &BindMountState,
    id_or_name: &str,
) -> Option<String> {
    resolve_registered_container_id(&bind_mounts.by_id, &bind_mounts.aliases, id_or_name)
}

fn resolve_registered_container_id<T>(
    by_id: &HashMap<String, T>,
    aliases: &HashMap<String, String>,
    id_or_name: &str,
) -> Option<String> {
    if by_id.contains_key(id_or_name) {
        return Some(id_or_name.to_owned());
    }
    if let Some(id) = aliases.get(id_or_name) {
        return Some(id.clone());
    }

    let mut matches = by_id.keys().filter(|id| id.starts_with(id_or_name));
    let first = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(first.clone())
}

fn allocate_bind_namespace(state: &ProxyState) -> String {
    format!(
        "b{:x}",
        state.next_bind_namespace.fetch_add(1, Ordering::Relaxed)
    )
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

#[cfg(test)]
mod bridge_host_to_guest_tcp_tests {
    use super::copy_retrying_wouldblock;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::os::unix::io::AsRawFd;
    use std::time::Duration;

    /// Regression test for `bridge_host_to_guest_tcp`'s data-loss bug: VZ's
    /// vsock connection fd can return `WouldBlock` on `write()` even without
    /// `O_NONBLOCK` set (confirmed live — debug session
    /// buildx-http2-frame-too-large), and `std::io::copy` treats any
    /// `WouldBlock` as fatal, silently truncating the copy. This mimics that
    /// scenario with a real loopback TCP pair whose writer-side `SO_SNDBUF`
    /// is capped small: `copy_retrying_wouldblock` must retry via poll(),
    /// not truncate, so every byte reaches the far end intact.
    #[test]
    fn copy_retrying_wouldblock_survives_wouldblock_without_truncating() {
        // Destination side (mimics writing into the VZ vsock fd downstream):
        // a loopback pair whose accepted end has a tiny SO_SNDBUF, and whose
        // client end only starts reading from a background thread — forcing
        // writes into `accepted` to hit WouldBlock before draining begins.
        let dst_listener = TcpListener::bind("127.0.0.1:0").expect("bind dst listener");
        let dst_addr = dst_listener.local_addr().expect("dst addr");
        let mut client = TcpStream::connect(dst_addr).expect("client connect");
        let mut accepted = dst_listener.accept().expect("dst accept").0;

        let sndbuf: libc::c_int = 4096;
        let rc = unsafe {
            libc::setsockopt(
                accepted.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                &sndbuf as *const libc::c_int as *const libc::c_void,
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        assert_eq!(rc, 0, "setsockopt(SO_SNDBUF=4096) failed");
        // Force the destination into non-blocking mode so a full SNDBUF
        // surfaces as WouldBlock rather than a plain blocking wait — this is
        // what makes the test actually exercise the WouldBlock-retry path
        // (the real VZ vsock fd returns WouldBlock even without O_NONBLOCK,
        // but a standard TCP socket needs this to reproduce the same
        // observable condition deterministically).
        accepted
            .set_nonblocking(true)
            .expect("set accepted stream nonblocking");

        // Source side (mimics reading the client request off the vsock fd):
        // a plain loopback TcpStream pair, fully fed up front.
        let src_listener = TcpListener::bind("127.0.0.1:0").expect("bind src listener");
        let src_addr = src_listener.local_addr().expect("src addr");
        let mut src_writer = TcpStream::connect(src_addr).expect("src writer connect");
        let mut src_reader = src_listener.accept().expect("src accept").0;

        const TOTAL: usize = 262_144; // 256KB — far larger than the 4KB SNDBUF cap.
        let expected: Vec<u8> = (0..TOTAL).map(|i| (i % 251) as u8).collect();

        let expected_for_writer = expected.clone();
        let writer_thread = std::thread::spawn(move || {
            src_writer
                .write_all(&expected_for_writer)
                .expect("feed src payload");
            src_writer
                .shutdown(std::net::Shutdown::Write)
                .expect("shutdown src writer");
        });

        // Delay the client's reads slightly so the copy loop reliably hits
        // WouldBlock at least once before anything drains the SNDBUF.
        let reader_thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let mut got = Vec::new();
            client.read_to_end(&mut got).expect("client read_to_end");
            got
        });

        let copied = copy_retrying_wouldblock(&mut src_reader, &mut accepted)
            .expect("copy_retrying_wouldblock must not error under backpressure");
        drop(accepted); // close write side so the client sees EOF

        writer_thread.join().expect("writer thread panicked");
        let got = reader_thread.join().expect("reader thread panicked");

        assert_eq!(
            copied as usize, TOTAL,
            "copy_retrying_wouldblock must report the full byte count"
        );
        assert_eq!(got.len(), TOTAL, "byte count mismatch at downstream client");
        assert_eq!(got, expected, "payload corrupted or reordered in transit");
    }
}

#[cfg(test)]
mod inject_ca_bundle_tests {
    use super::inject_ca_bundle;

    fn inject(body: serde_json::Value) -> serde_json::Value {
        let out = inject_ca_bundle(&serde_json::to_vec(&body).unwrap()).unwrap();
        serde_json::from_slice(&out).unwrap()
    }

    #[test]
    fn adds_binds_and_env_to_a_minimal_body() {
        let out = inject(serde_json::json!({"Image": "alpine"}));
        let binds = out["HostConfig"]["Binds"].as_array().unwrap();
        assert!(binds.contains(
            &"/etc/ssl/certs/ca-certificates.crt:/etc/ssl/certs/ca-certificates.crt:ro".into()
        ));
        assert!(binds.contains(
            &"/etc/ssl/certs/ca-certificates.crt:/etc/pki/tls/certs/ca-bundle.crt:ro".into()
        ));
        let env = out["Env"].as_array().unwrap();
        assert!(env.contains(&"SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt".into()));
        assert!(env.contains(&"NODE_EXTRA_CA_CERTS=/etc/ssl/certs/ca-certificates.crt".into()));
        assert_eq!(out["Image"], "alpine");
    }

    #[test]
    fn keeps_user_binds_env_and_null_fields_intact() {
        let out = inject(serde_json::json!({
            "Image": "python",
            "Env": ["REQUESTS_CA_BUNDLE=/custom.pem", "A=1"],
            "HostConfig": {
                "Binds": ["/host/ca.pem:/etc/ssl/certs/ca-certificates.crt:ro", "/data:/data"],
                "Memory": 0
            }
        }));
        let binds = out["HostConfig"]["Binds"].as_array().unwrap();
        assert_eq!(binds.len(), 3); // user CA bind kept, RHEL path added
        assert!(binds.contains(&"/host/ca.pem:/etc/ssl/certs/ca-certificates.crt:ro".into()));
        let env = out["Env"].as_array().unwrap();
        assert!(env.contains(&"REQUESTS_CA_BUNDLE=/custom.pem".into()));
        assert_eq!(
            env.iter()
                .filter(|e| e.as_str().unwrap().starts_with("REQUESTS_CA_BUNDLE="))
                .count(),
            1
        );
        assert_eq!(out["HostConfig"]["Memory"], 0);

        let out =
            inject(serde_json::json!({"Image": "x", "Env": null, "HostConfig": {"Binds": null}}));
        assert_eq!(out["Env"].as_array().unwrap().len(), 3);
        assert_eq!(out["HostConfig"]["Binds"].as_array().unwrap().len(), 2);
    }
}
