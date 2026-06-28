use axum::Router;
use axum::routing::{delete, get, post};

use crate::handlers::{attach, build, containers, events, exec, images, networks, system, volumes};
use crate::state::AppState;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/_ping", get(system::ping).head(system::ping))
        .route("/version", get(system::version))
        .route("/info", get(system::info))
        .route("/events", get(events::list))
        .route("/containers/json", get(containers::list))
        .route("/containers/create", post(containers::create))
        .route("/containers/{id}/json", get(containers::inspect))
        .route("/containers/{id}/start", post(containers::start))
        .route("/containers/{id}/stop", post(containers::stop))
        .route("/containers/{id}/kill", post(containers::kill))
        .route("/containers/{id}/wait", post(containers::wait))
        .route("/containers/{id}", delete(containers::remove))
        .route("/containers/{id}/attach", post(attach::attach))
        .route("/containers/{id}/logs", get(containers::logs))
        .route(
            "/containers/{id}/archive",
            get(containers::archive).put(containers::put_archive),
        )
        .route("/containers/{id}/exec", post(exec::create))
        .route("/exec/{id}/start", post(exec::start))
        .route("/exec/{id}/json", get(exec::inspect))
        .route("/images/create", post(images::create))
        .route("/images/json", get(images::list))
        .route("/images/{name}/json", get(images::inspect))
        .route("/images/{name}/push", post(images::push))
        .route("/images/{name}", delete(images::remove))
        .route("/build", post(build::build))
        .route("/networks", get(networks::list))
        .route("/networks/create", post(networks::create))
        .route(
            "/networks/{id}",
            get(networks::inspect).delete(networks::remove),
        )
        .route("/networks/{id}/connect", post(networks::connect))
        .route("/networks/{id}/disconnect", post(networks::disconnect))
        .route("/volumes", get(volumes::list))
        .route("/volumes/create", post(volumes::create))
        .route(
            "/volumes/{name}",
            get(volumes::inspect).delete(volumes::remove),
        )
        .with_state(state)
}
