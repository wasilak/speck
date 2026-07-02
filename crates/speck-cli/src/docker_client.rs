use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use http_body_util::BodyExt;
use http_body_util::Full;
use hyper::body::Bytes;
use hyper::body::Incoming;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::UnixStream;
use tower::Service;

type DockerHttpClient = Client<UnixConnector, Full<Bytes>>;

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

pub struct DockerClient {
    sock_path: PathBuf,
}

impl DockerClient {
    pub fn new(sock_path: impl AsRef<Path>) -> Self {
        Self {
            sock_path: sock_path.as_ref().to_owned(),
        }
    }

    fn http_client(&self) -> DockerHttpClient {
        Client::builder(TokioExecutor::new()).build(UnixConnector::new(self.sock_path.clone()))
    }

    pub async fn get(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        let uri: hyper::Uri = format!("http://localhost{path}").parse()?;
        let _ = &self.sock_path;
        let resp = self.http_client().get(uri).await?;
        let body = read_body(resp.into_body()).await?;
        Ok(serde_json::from_slice(&body)?)
    }

    pub async fn post<T: Into<hyper::body::Bytes>>(
        &self,
        path: &str,
        body: T,
        content_type: &str,
    ) -> anyhow::Result<serde_json::Value> {
        let uri: hyper::Uri = format!("http://localhost{path}").parse()?;
        let _ = &self.sock_path;
        let req = hyper::Request::builder()
            .method("POST")
            .uri(&uri)
            .header("Content-Type", content_type)
            .body(Full::new(body.into()))?;
        let resp = self.http_client().request(req).await?;
        let body = read_body(resp.into_body()).await?;
        Ok(serde_json::from_slice(&body)?)
    }

    pub async fn post_json(
        &self,
        path: &str,
        value: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let bytes = serde_json::to_vec(value)?;
        self.post(path, bytes, "application/json").await
    }

    pub async fn post_empty(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        let uri: hyper::Uri = format!("http://localhost{path}").parse()?;
        let _ = &self.sock_path;
        let req = hyper::Request::builder()
            .method("POST")
            .uri(&uri)
            .body(Full::new(Bytes::new()))?;
        let resp = self.http_client().request(req).await?;
        let body = read_body(resp.into_body()).await?;
        Ok(serde_json::from_slice(&body)?)
    }

    pub async fn delete(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        let uri: hyper::Uri = format!("http://localhost{path}").parse()?;
        let _ = &self.sock_path;
        let req = hyper::Request::builder()
            .method("DELETE")
            .uri(&uri)
            .body(Full::new(Bytes::new()))?;
        let resp = self.http_client().request(req).await?;
        let body = read_body(resp.into_body()).await?;
        Ok(serde_json::from_slice(&body)?)
    }

    pub async fn post_body_raw(&self, path: &str, body: Full<Bytes>) -> anyhow::Result<Vec<u8>> {
        let uri: hyper::Uri = format!("http://localhost{path}").parse()?;
        let _ = &self.sock_path;
        let req = hyper::Request::builder()
            .method("POST")
            .uri(&uri)
            .body(body)?;
        let resp = self.http_client().request(req).await?;
        read_body(resp.into_body()).await
    }
}

async fn read_body(body: Incoming) -> anyhow::Result<Vec<u8>> {
    let collected = body.collect().await?;
    Ok(collected.to_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    const SOURCE: &str = include_str!("docker_client.rs");

    #[test]
    fn docker_client_uses_unix_socket_connector_without_tcp_fallback() {
        assert!(
            SOURCE.contains("tokio::net::UnixStream"),
            "DockerClient must dial the configured Unix socket with tokio::net::UnixStream"
        );
        assert!(
            SOURCE.contains("UnixConnector"),
            "DockerClient must define an in-repo Unix connector"
        );
        assert!(
            SOURCE.contains("UnixStream::connect"),
            "connector must call UnixStream::connect using the configured socket path"
        );
        assert!(
            !SOURCE.contains(concat!("Http", "Connector")),
            "DockerClient must not retain the TCP HTTP connector fallback"
        );
        assert!(
            !SOURCE.contains(concat!("build", "_http()")),
            "DockerClient must not build a TCP HTTP client"
        );
    }
}
