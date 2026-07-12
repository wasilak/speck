//! HTTP-aware passthrough proxy to the real dockerd running inside the guest.
//!
//! Locked decision D-21: the Docker API surface is a transparent proxy to the
//! real dockerd (`/run/speck/dockerd.sock` in the guest). Speck does not
//! reimplement Docker API endpoints against containerd.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::task::{Context, Poll};

use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use hyper::body::Incoming;
use hyper::header::UPGRADE;
use hyper::upgrade;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use speck_core::VmState;
use tokio::net::UnixStream;
use tower::Service;

use crate::DockerApiError;
use crate::middleware::restart_503::RestartCheckLayer;

#[derive(Clone)]
struct ProxyState {
    internal_sock_path: Arc<PathBuf>,
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

pub fn build_proxy_router(internal_sock_path: PathBuf, vm_state: Arc<RwLock<VmState>>) -> Router {
    let state = ProxyState {
        internal_sock_path: Arc::new(internal_sock_path),
    };

    Router::new()
        .fallback(passthrough)
        .with_state(state)
        .layer(DefaultBodyLimit::disable())
        .layer(RestartCheckLayer::new(vm_state))
}

async fn passthrough(State(state): State<ProxyState>, mut req: Request) -> crate::Result<Response> {
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
