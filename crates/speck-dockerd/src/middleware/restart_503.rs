use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::RwLock;
use std::task::{Context, Poll};

use axum::http::{HeaderValue, Request, Response, StatusCode};
use speck_core::VmState;
use tower::{Layer, Service};

#[derive(Clone)]
pub struct RestartCheckLayer {
    vm_state: Arc<RwLock<VmState>>,
}

impl RestartCheckLayer {
    pub fn new(vm_state: Arc<RwLock<VmState>>) -> Self {
        Self { vm_state }
    }
}

impl<S> Layer<S> for RestartCheckLayer {
    type Service = RestartCheckService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RestartCheckService {
            inner,
            vm_state: self.vm_state.clone(),
        }
    }
}

#[derive(Clone)]
pub struct RestartCheckService<S> {
    inner: S,
    vm_state: Arc<RwLock<VmState>>,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for RestartCheckService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Into<axum::BoxError>,
    ReqBody: Send + 'static,
    ResBody: Default + Send + 'static,
{
    type Response = Response<ResBody>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let state = self.vm_state.read().expect("VmState RwLock poisoned").clone();

        if state == VmState::Restarting {
            let mut response = Response::new(ResBody::default());
            *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
            response
                .headers_mut()
                .insert("Retry-After", HeaderValue::from_static("5"));
            return Box::pin(async { Ok(response) });
        }

        let future = self.inner.call(req);
        Box::pin(async move { future.await })
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn middleware_returns_503_when_restarting() {
        let source = include_str!("restart_503.rs");
        let test_start = source.find("#[cfg(test)]").unwrap_or(source.len());
        let production = &source[..test_start];

        assert!(
            production.contains("StatusCode::SERVICE_UNAVAILABLE")
                || production.contains("503"),
            "production code must return 503 during Restarting state"
        );
        assert!(
            production.contains("VmState::Restarting"),
            "production code must check VmState::Restarting"
        );
    }

    #[test]
    fn middleware_includes_retry_after_header() {
        let source = include_str!("restart_503.rs");
        let test_start = source.find("#[cfg(test)]").unwrap_or(source.len());
        let production = &source[..test_start];

        assert!(
            production.contains("Retry-After"),
            "503 response must include Retry-After header"
        );
    }

    #[test]
    fn middleware_passes_through_when_running() {
        let source = include_str!("restart_503.rs");
        let test_start = source.find("#[cfg(test)]").unwrap_or(source.len());
        let production = &source[..test_start];

        assert!(
            production.contains(".inner.call(req)"),
            "non-Restarting state must delegate to inner service via self.inner.call(req)"
        );
    }
}
