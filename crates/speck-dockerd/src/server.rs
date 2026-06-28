use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use axum::Router;
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use tokio::net::UnixListener;
use tower::ServiceExt;

pub async fn serve(
    router: Router,
    sock_path: PathBuf,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    let _ = tokio::fs::remove_file(&sock_path).await;

    let listener = match UnixListener::bind(&sock_path) {
        Ok(listener) => listener,
        Err(err) => {
            tracing::error!(?err, path = %sock_path.display(), "failed to bind Docker API socket");
            return;
        }
    };

    if let Err(err) =
        tokio::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o600)).await
    {
        tracing::error!(?err, path = %sock_path.display(), "failed to set Docker API socket permissions");
        return;
    }

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _addr)) => {
                        let router = router.clone();
                        tokio::spawn(async move {
                            let service = service_fn(move |request| {
                                let router = router.clone();
                                async move { router.oneshot(request).await }
                            });
                            let io = TokioIo::new(stream);
                            let builder = Builder::new(TokioExecutor::new());
                            if let Err(err) = builder.serve_connection_with_upgrades(io, service).await {
                                tracing::debug!(?err, "Docker API connection ended with error");
                            }
                        });
                    }
                    Err(err) => tracing::warn!(?err, "failed to accept Docker API connection"),
                }
            }
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    break;
                }
            }
        }
    }

    let _ = tokio::fs::remove_file(sock_path).await;
}
