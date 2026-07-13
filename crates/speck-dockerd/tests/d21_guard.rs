fn strip_comment_lines(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn d21_speck_sock_is_served_by_transparent_proxy() {
    let source = strip_comment_lines(include_str!("../src/lib.rs"));
    assert!(
        source.contains("build_proxy_router"),
        "D-21: SpeckDockerd::start must wire speck.sock through build_proxy_router"
    );
    assert!(
        source.contains("docker_api_unix_proxy"),
        "D-21: SpeckDockerd::start must reach guest dockerd via docker_api_unix_proxy"
    );
}

#[test]
fn d21_speck_sock_never_wired_to_reimplemented_handlers() {
    let source = strip_comment_lines(include_str!("../src/lib.rs"));
    assert!(
        !source.contains("router::build_router") && !source.contains("build_router("),
        "D-21: speck.sock must never be rewired to containerd-backed reimplemented handlers"
    );
}

#[test]
fn d21_wire_path_does_not_strip_api_version() {
    let source = strip_comment_lines(include_str!("../src/server.rs"));
    assert!(
        !source.contains("rewrite_uri") && !source.contains("strip_version_prefix"),
        "D-21: the wire path must forward /v1.xx URIs unchanged to real dockerd"
    );
}
