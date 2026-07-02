use std::path::Path;
use std::path::PathBuf;

use hyper_util::rt::TokioIo;
use tokio::sync::Mutex;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

pub mod proto {
    tonic::include_proto!("buildkit.control");
}

pub use proto::control_client::ControlClient;

pub struct BuildkitClient {
    inner: Mutex<ControlClient<Channel>>,
}

#[derive(Debug, Clone)]
pub struct LocalBuildContext {
    pub session_id: String,
    pub context_name: String,
    pub dockerfile_name: String,
    pub tar_path: PathBuf,
}

impl BuildkitClient {
    pub async fn connect_unix<P: AsRef<Path>>(path: P) -> Result<Self, Error> {
        let path = path.as_ref().to_owned();

        let uri: Uri = path
            .to_str()
            .map(|s| format!("http://{}/", s))
            .ok_or_else(|| Error::Buildkit(format!("invalid path: {:?}", path)))?
            .parse()
            .map_err(|e| Error::Buildkit(format!("uri parse: {e}")))?;

        let channel = Endpoint::from(uri)
            .connect_with_connector(service_fn(move |_: Uri| {
                let path = path.clone();
                async move {
                    let stream = tokio::net::UnixStream::connect(&path).await?;
                    Ok::<_, std::io::Error>(TokioIo::new(stream))
                }
            }))
            .await
            .map_err(|e| Error::Buildkit(format!("connect: {e}")))?;

        let client = ControlClient::new(channel);

        Ok(Self {
            inner: Mutex::new(client),
        })
    }

    pub async fn solve(&self, req: proto::SolveRequest) -> Result<proto::SolveResponse, Error> {
        let mut client = self.inner.lock().await;
        client
            .solve(req)
            .await
            .map(|r| r.into_inner())
            .map_err(|e| Error::Buildkit(format!("solve: {e}")))
    }

    pub async fn solve_with_local_context(
        &self,
        ref_id: String,
        frontend: String,
        frontend_attrs: std::collections::HashMap<String, String>,
        context: &LocalBuildContext,
    ) -> Result<proto::SolveResponse, Error> {
        self.upload_local_context(context).await?;
        self.solve(Self::solve_request_with_local_context(
            ref_id,
            frontend,
            frontend_attrs,
            context,
        ))
        .await
    }

    pub fn solve_request_with_local_context(
        ref_id: String,
        frontend: String,
        frontend_attrs: std::collections::HashMap<String, String>,
        context: &LocalBuildContext,
    ) -> proto::SolveRequest {
        let mut frontend_inputs = std::collections::HashMap::new();
        frontend_inputs.insert(context.context_name.clone(), proto::Definition::default());
        frontend_inputs.insert(context.dockerfile_name.clone(), proto::Definition::default());

        proto::SolveRequest {
            r#ref: ref_id,
            definition: None,
            exporter_deprecated: String::new(),
            exporter_attrs_deprecated: std::collections::HashMap::new(),
            session: context.session_id.clone(),
            frontend,
            frontend_attrs,
            cache: None,
            entitlements: Vec::new(),
            frontend_inputs,
            internal: false,
            exporters: Vec::new(),
            enable_session_exporter: false,
            source_policy_session: String::new(),
            compatibility_version: 0,
            proxy_network: false,
        }
    }

    async fn upload_local_context(&self, context: &LocalBuildContext) -> Result<(), Error> {
        let data = tokio::fs::read(&context.tar_path)
            .await
            .map_err(|e| Error::Buildkit(format!("read local build context: {e}")))?;
        let stream = tokio_stream::iter([proto::BytesMessage { data }]);

        let mut client = self.inner.lock().await;
        client
            .session(stream)
            .await
            .map(|_| ())
            .map_err(|e| Error::Buildkit(format!("session: {e}")))
    }

    #[allow(dead_code)]
    pub async fn status(
        &self,
        ref_id: String,
    ) -> Result<tonic::codec::Streaming<proto::StatusResponse>, Error> {
        let mut client = self.inner.lock().await;
        let req = tonic::Request::new(proto::StatusRequest { r#ref: ref_id });
        client
            .status(req)
            .await
            .map(|r| r.into_inner())
            .map_err(|e| Error::Buildkit(format!("status: {e}")))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("BuildKit client error: {0}")]
    Buildkit(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn solve_request_can_represent_local_context_session() {
        let context = LocalBuildContext {
            session_id: "speck-session-1".into(),
            context_name: "context".into(),
            dockerfile_name: "dockerfile".into(),
            tar_path: PathBuf::from("/tmp/context.tar"),
        };
        let mut attrs = HashMap::new();
        attrs.insert("filename".into(), "Dockerfile".into());

        let req = BuildkitClient::solve_request_with_local_context(
            "example:latest".into(),
            "dockerfile.v0".into(),
            attrs,
            &context,
        );

        assert_eq!(req.session, "speck-session-1");
        assert!(req.frontend_inputs.contains_key("context"));
        assert!(req.frontend_inputs.contains_key("dockerfile"));
    }
}
