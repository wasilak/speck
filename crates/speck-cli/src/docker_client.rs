use std::path::Path;

use http_body_util::BodyExt;
use http_body_util::Full;
use hyper::body::Bytes;
use hyper::body::Incoming;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;

pub struct DockerClient {
    sock_path: String,
}

impl DockerClient {
    pub fn new(sock_path: impl AsRef<Path>) -> Self {
        Self {
            sock_path: format!("unix://{}", sock_path.as_ref().display()),
        }
    }

    fn http_client(&self) -> Client<HttpConnector, Full<Bytes>> {
        Client::builder(TokioExecutor::new()).build_http()
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
            !SOURCE.contains("HttpConnector"),
            "DockerClient must not retain the TCP HttpConnector fallback"
        );
        assert!(
            !SOURCE.contains("build_http()"),
            "DockerClient must not build a TCP HTTP client"
        );
    }
}
