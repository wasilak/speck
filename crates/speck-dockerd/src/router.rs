use axum::Router;
use axum::routing::{delete, get, post};

use crate::handlers::{attach, build, containers, events, exec, images, networks, system, volumes};
use crate::state::AppState;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/_ping", get(system::ping).head(system::ping))
        .route("/version", get(system::version))
        .route("/info", get(system::info))
        .route("/events", get(events::events_stream))
        .route("/containers/json", get(containers::list))
        .route("/containers/create", post(containers::create))
        .route("/containers/{id}/json", get(containers::inspect))
        .route("/containers/{id}/start", post(containers::start))
        .route("/containers/{id}/stop", post(containers::stop))
        .route("/containers/{id}/kill", post(containers::kill))
        .route("/containers/{id}/wait", post(containers::wait))
        .route("/containers/{id}", delete(containers::remove))
        .route("/containers/{id}/attach", post(attach::container_attach))
        .route("/containers/{id}/logs", get(attach::container_logs))
        .route(
            "/containers/{id}/archive",
            get(attach::container_archive_get).put(attach::container_archive_put),
        )
        .route("/containers/{id}/exec", post(exec::create))
        .route("/exec/{id}/start", post(exec::start))
        .route("/exec/{id}/json", get(exec::inspect))
        .route("/images/create", post(images::image_pull))
        .route("/images/json", get(images::image_list))
        .route("/images/{name}/json", get(images::image_inspect))
        .route("/images/{name}/push", post(images::image_push))
        .route("/images/{name}", delete(images::image_remove))
        .route("/build", post(build::build))
        .route("/networks", get(networks::network_list))
        .route("/networks/create", post(networks::network_create))
        .route(
            "/networks/{id}",
            get(networks::network_inspect).delete(networks::network_remove),
        )
        .route("/networks/{id}/connect", post(networks::network_connect))
        .route(
            "/networks/{id}/disconnect",
            post(networks::network_disconnect),
        )
        .route("/volumes", get(volumes::volume_list))
        .route("/volumes/create", post(volumes::volume_create))
        .route(
            "/volumes/{name}",
            get(volumes::volume_inspect).delete(volumes::volume_remove),
        )
        .with_state(state)
}
