use bollard::Docker;
use bollard::exec::StartExecResults;
use bollard::models::{
    ContainerCreateBody, ExecConfig, HostConfig, NetworkConnectRequest, NetworkCreateRequest,
    NetworkDisconnectRequest, PortBinding, VolumeCreateRequest,
};
use bollard::query_parameters::CreateImageOptionsBuilder;
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, InspectContainerOptions, ListContainersOptions, LogsOptions,
    RemoveContainerOptions, StartContainerOptions, WaitContainerOptions,
};
use futures_util::StreamExt;
use futures_util::TryStreamExt;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::net::TcpStream;

fn speck_sock() -> PathBuf {
    if let Ok(sock) = std::env::var("SPECK_SOCK") {
        return PathBuf::from(sock);
    }
    let home = std::env::var("SPECK_HOME")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            PathBuf::from(home).join(".speck")
        });
    home.join("speck.sock")
}

fn speck_docker() -> Docker {
    let sock_path = speck_sock();
    assert!(
        sock_path.exists(),
        "speck socket not found at {}. Start 'spk up' first.",
        sock_path.display()
    );
    Docker::connect_with_unix(
        sock_path.to_str().unwrap(),
        u64::try_from(Duration::from_secs(120).as_millis()).unwrap_or(120_000),
        bollard::API_DEFAULT_VERSION,
    )
    .expect("connect to speck Docker socket")
}

#[tokio::test]
#[ignore = "requires signed binary + spk up running"]
async fn test_ping() {
    let docker = speck_docker();
    docker.ping().await.expect("ping should succeed");
}

#[tokio::test]
#[ignore = "requires signed binary + spk up running"]
async fn test_version() {
    let docker = speck_docker();
    let version = docker.version().await.expect("version should succeed");
    assert!(
        version.api_version.is_some(),
        "ApiVersion should be present"
    );
    assert!(
        version.min_api_version.is_some(),
        "MinAPIVersion should be present"
    );
}

#[tokio::test]
#[ignore = "requires signed binary + spk up running"]
async fn test_info() {
    let docker = speck_docker();
    let info = docker.info().await.expect("info should succeed");
    assert!(
        info.server_version.is_some(),
        "ServerVersion should be present"
    );
}

#[tokio::test]
#[ignore = "requires signed binary + spk up running"]
async fn test_image_pull_alpine() {
    let docker = speck_docker();
    let options = CreateImageOptionsBuilder::default()
        .from_image("alpine")
        .tag("latest")
        .build();
    let stream = docker.create_image(Some(options), None, None);
    let results: Vec<_> = stream.try_collect().await.expect("pull alpine");
    assert!(
        !results.is_empty(),
        "should get at least one progress event"
    );
}

#[tokio::test]
#[ignore = "requires signed binary + spk up running"]
async fn test_image_list() {
    let docker = speck_docker();
    let images = docker
        .list_images(None::<bollard::query_parameters::ListImagesOptions>)
        .await
        .expect("list images should succeed");
    let _ = images;
}

#[tokio::test]
#[ignore = "requires signed binary + spk up running"]
async fn test_container_create_list_remove() {
    let docker = speck_docker();
    let config = ContainerCreateBody {
        image: Some("alpine".to_string()),
        cmd: Some(vec!["echo".to_string(), "hello".to_string()]),
        ..Default::default()
    };
    let options = CreateContainerOptionsBuilder::default()
        .name("test-conformance-create-list-remove")
        .build();
    let response = docker
        .create_container(Some(options), config)
        .await
        .expect("create container");
    let container_id = response.id;

    let containers = docker
        .list_containers(Some(ListContainersOptions {
            all: true,
            ..Default::default()
        }))
        .await
        .expect("list containers");
    assert!(
        containers.iter().any(|c| {
            c.id.as_deref()
                .is_some_and(|id| id.starts_with(&container_id))
        }),
        "created container should appear in list"
    );

    docker
        .remove_container(
            &container_id,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await
        .expect("remove container");
}

#[tokio::test]
#[ignore = "requires signed binary + spk up running"]
async fn test_container_start_wait_remove() {
    let docker = speck_docker();
    let config = ContainerCreateBody {
        image: Some("alpine".to_string()),
        cmd: Some(vec![
            "sh".to_string(),
            "-c".to_string(),
            "exit 0".to_string(),
        ]),
        ..Default::default()
    };
    let options = CreateContainerOptionsBuilder::default()
        .name("test-conformance-start-wait")
        .build();
    let response = docker
        .create_container(Some(options), config)
        .await
        .expect("create container");
    let container_id = response.id;

    docker
        .start_container(&container_id, None::<StartContainerOptions>)
        .await
        .expect("start container");

    let wait = docker
        .wait_container(&container_id, None::<WaitContainerOptions>)
        .try_collect::<Vec<_>>()
        .await
        .expect("wait container");
    assert_eq!(
        wait.first().map(|r| r.status_code),
        Some(0),
        "exit code should be 0"
    );

    docker
        .remove_container(
            &container_id,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await
        .expect("remove container");
}

#[tokio::test]
async fn test_container_logs() {
    if std::env::var("SPECK_TEST_INTEGRATION").is_err() {
        eprintln!("skipping integration test: SPECK_TEST_INTEGRATION not set");
        return;
    }
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let docker = speck_docker();
        let config = ContainerCreateBody {
            image: Some("alpine".to_string()),
            cmd: Some(vec!["echo".to_string(), "hello-from-speck".to_string()]),
            ..Default::default()
        };
        let options = CreateContainerOptionsBuilder::default()
            .name("test-conformance-logs")
            .build();
        let response = docker
            .create_container(Some(options), config)
            .await
            .expect("create container");
        let container_id = response.id;

        docker
            .start_container(&container_id, None::<StartContainerOptions>)
            .await
            .expect("start container");

        docker
            .wait_container(&container_id, None::<WaitContainerOptions>)
            .try_collect::<Vec<_>>()
            .await
            .expect("wait container");

        let logs = docker
            .logs(
                &container_id,
                Some(LogsOptions {
                    stdout: true,
                    stderr: true,
                    ..Default::default()
                }),
            )
            .try_collect::<Vec<_>>()
            .await
            .expect("get logs");
        let output: String = logs
            .iter()
            .flat_map(|l| l.as_ref())
            .map(|&b| b as char)
            .collect();
        assert!(
            output.contains("hello-from-speck"),
            "log output should contain 'hello-from-speck', got: {output:?}"
        );

        docker
            .remove_container(
                &container_id,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
            .expect("remove container");
    })
    .await;
    assert!(result.is_ok(), "test_container_logs timed out after 30s");
}

#[tokio::test]
async fn test_inspect_shows_port_bindings() {
    if std::env::var("SPECK_TEST_INTEGRATION").is_err() {
        eprintln!("skipping integration test: SPECK_TEST_INTEGRATION not set");
        return;
    }
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let docker = speck_docker();
        let mut port_bindings = HashMap::new();
        port_bindings.insert(
            "80/tcp".to_string(),
            Some(vec![PortBinding {
                host_ip: Some("127.0.0.1".to_string()),
                host_port: Some("18081".to_string()),
            }]),
        );
        let config = ContainerCreateBody {
            image: Some("alpine".to_string()),
            cmd: Some(vec![
                "sh".to_string(),
                "-c".to_string(),
                "exit 0".to_string(),
            ]),
            host_config: Some(HostConfig {
                port_bindings: Some(port_bindings),
                ..Default::default()
            }),
            ..Default::default()
        };
        let options = CreateContainerOptionsBuilder::default()
            .name("test-conformance-inspect-ports")
            .build();
        let response = docker
            .create_container(Some(options), config)
            .await
            .expect("create container");
        let container_id = response.id;

        let inspect = docker
            .inspect_container(&container_id, None::<InspectContainerOptions>)
            .await
            .expect("inspect container");

        let ports = inspect
            .network_settings
            .as_ref()
            .and_then(|ns| ns.ports.as_ref())
            .expect("NetworkSettings.Ports should be present");
        assert!(
            ports.contains_key("80/tcp"),
            "Ports should contain '80/tcp', got: {ports:?}"
        );
        let binding = ports
            .get("80/tcp")
            .and_then(|bindings| bindings.as_ref())
            .and_then(|bindings| bindings.first())
            .expect("80/tcp should have a port binding");
        assert_eq!(binding.host_ip.as_deref(), Some("127.0.0.1"));
        assert_eq!(binding.host_port.as_deref(), Some("18081"));

        docker
            .remove_container(
                &container_id,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
            .expect("remove container");
    })
    .await;
    assert!(
        result.is_ok(),
        "test_inspect_shows_port_bindings timed out after 30s"
    );
}

#[tokio::test]
async fn test_logs_follow_streams_delayed_output() {
    if std::env::var("SPECK_TEST_INTEGRATION").is_err() {
        eprintln!("skipping integration test: SPECK_TEST_INTEGRATION not set");
        return;
    }
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let docker = speck_docker();
        let config = ContainerCreateBody {
            image: Some("alpine".to_string()),
            cmd: Some(vec![
                "sh".to_string(),
                "-c".to_string(),
                "sleep 3; echo follow-me-speck".to_string(),
            ]),
            ..Default::default()
        };
        let options = CreateContainerOptionsBuilder::default()
            .name("test-conformance-logs-follow")
            .build();
        let response = docker
            .create_container(Some(options), config)
            .await
            .expect("create container");
        let container_id = response.id;

        docker
            .start_container(&container_id, None::<StartContainerOptions>)
            .await
            .expect("start container");

        let mut logs = docker.logs(
            &container_id,
            Some(LogsOptions {
                follow: true,
                stdout: true,
                stderr: true,
                ..Default::default()
            }),
        );
        let output = tokio::time::timeout(Duration::from_secs(20), async {
            let mut bytes = Vec::new();
            while let Some(chunk) = logs.next().await {
                let chunk = chunk.expect("read logs chunk");
                bytes.extend_from_slice(chunk.as_ref());
                if String::from_utf8_lossy(&bytes).contains("follow-me-speck") {
                    break;
                }
            }
            bytes
        })
        .await
        .expect("follow stream should yield delayed output");
        let output = String::from_utf8_lossy(&output);
        assert!(
            output.contains("follow-me-speck"),
            "follow log output should contain delayed line, got: {output:?}"
        );

        docker
            .remove_container(
                &container_id,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
            .expect("remove container");
    })
    .await;
    assert!(
        result.is_ok(),
        "test_logs_follow_streams_delayed_output timed out after 30s"
    );
}

#[tokio::test]
async fn test_inspect_default_host_ip() {
    if std::env::var("SPECK_TEST_INTEGRATION").is_err() {
        eprintln!("skipping integration test: SPECK_TEST_INTEGRATION not set");
        return;
    }
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let docker = speck_docker();
        let mut port_bindings = HashMap::new();
        port_bindings.insert(
            "80/tcp".to_string(),
            Some(vec![PortBinding {
                host_ip: None,
                host_port: Some("18082".to_string()),
            }]),
        );
        let config = ContainerCreateBody {
            image: Some("alpine".to_string()),
            cmd: Some(vec![
                "sh".to_string(),
                "-c".to_string(),
                "exit 0".to_string(),
            ]),
            host_config: Some(HostConfig {
                port_bindings: Some(port_bindings),
                ..Default::default()
            }),
            ..Default::default()
        };
        let options = CreateContainerOptionsBuilder::default()
            .name("test-conformance-default-host-ip")
            .build();
        let response = docker
            .create_container(Some(options), config)
            .await
            .expect("create container");
        let container_id = response.id;

        let inspect = docker
            .inspect_container(&container_id, None::<InspectContainerOptions>)
            .await
            .expect("inspect container");
        let binding = inspect
            .network_settings
            .as_ref()
            .and_then(|ns| ns.ports.as_ref())
            .and_then(|ports| ports.get("80/tcp"))
            .and_then(|bindings| bindings.as_ref())
            .and_then(|bindings| bindings.first())
            .expect("80/tcp should have a port binding");
        assert_eq!(binding.host_ip.as_deref(), Some("0.0.0.0"));
        assert_eq!(binding.host_port.as_deref(), Some("18082"));

        docker
            .remove_container(
                &container_id,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
            .expect("remove container");
    })
    .await;
    assert!(
        result.is_ok(),
        "test_inspect_default_host_ip timed out after 30s"
    );
}

#[tokio::test]
async fn test_inspect_empty_string_host_ip_defaults() {
    if std::env::var("SPECK_TEST_INTEGRATION").is_err() {
        eprintln!("skipping integration test: SPECK_TEST_INTEGRATION not set");
        return;
    }
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let docker = speck_docker();
        let mut port_bindings = HashMap::new();
        port_bindings.insert(
            "80/tcp".to_string(),
            Some(vec![PortBinding {
                host_ip: Some("".to_string()),
                host_port: Some("18083".to_string()),
            }]),
        );
        let config = ContainerCreateBody {
            image: Some("alpine".to_string()),
            cmd: Some(vec![
                "sh".to_string(),
                "-c".to_string(),
                "exit 0".to_string(),
            ]),
            host_config: Some(HostConfig {
                port_bindings: Some(port_bindings),
                ..Default::default()
            }),
            ..Default::default()
        };
        let options = CreateContainerOptionsBuilder::default()
            .name("test-conformance-empty-host-ip")
            .build();
        let response = docker
            .create_container(Some(options), config)
            .await
            .expect("create container");
        let container_id = response.id;

        docker
            .start_container(&container_id, None::<StartContainerOptions>)
            .await
            .expect("start container");

        let _wait = docker
            .wait_container(&container_id, None::<WaitContainerOptions>)
            .try_collect::<Vec<_>>()
            .await
            .expect("wait container");

        let inspect = docker
            .inspect_container(&container_id, None::<InspectContainerOptions>)
            .await
            .expect("inspect container");
        let binding = inspect
            .network_settings
            .as_ref()
            .and_then(|ns| ns.ports.as_ref())
            .and_then(|ports| ports.get("80/tcp"))
            .and_then(|bindings| bindings.as_ref())
            .and_then(|bindings| bindings.first())
            .expect("80/tcp should have a port binding");
        assert_eq!(binding.host_ip.as_deref(), Some("0.0.0.0"));
        assert_eq!(binding.host_port.as_deref(), Some("18083"));

        docker
            .remove_container(
                &container_id,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
            .expect("remove container");
    })
    .await;
    assert!(
        result.is_ok(),
        "test_inspect_empty_string_host_ip_defaults timed out after 30s"
    );
}

#[tokio::test]
async fn test_network_connect_missing_network_404() {
    if std::env::var("SPECK_TEST_INTEGRATION").is_err() {
        eprintln!("skipping integration test: SPECK_TEST_INTEGRATION not set");
        return;
    }
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let docker = speck_docker();
        let err = docker
            .connect_network(
                "speck-missing-net-conformance",
                NetworkConnectRequest {
                    container: "speck-missing-container".to_string(),
                    endpoint_config: None,
                },
            )
            .await
            .expect_err("missing network should return 404");
        assert!(matches!(
            err,
            bollard::errors::Error::DockerResponseServerError {
                status_code: 404,
                ..
            }
        ));
    })
    .await;
    assert!(
        result.is_ok(),
        "test_network_connect_missing_network_404 timed out after 30s"
    );
}

#[tokio::test]
async fn test_network_disconnect_missing_network_404() {
    if std::env::var("SPECK_TEST_INTEGRATION").is_err() {
        eprintln!("skipping integration test: SPECK_TEST_INTEGRATION not set");
        return;
    }
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let docker = speck_docker();
        let err = docker
            .disconnect_network(
                "speck-missing-net-conformance",
                NetworkDisconnectRequest {
                    container: "speck-missing-container".to_string(),
                    force: Some(false),
                },
            )
            .await
            .expect_err("missing network should return 404");
        assert!(matches!(
            err,
            bollard::errors::Error::DockerResponseServerError {
                status_code: 404,
                ..
            }
        ));
    })
    .await;
    assert!(
        result.is_ok(),
        "test_network_disconnect_missing_network_404 timed out after 30s"
    );
}

#[tokio::test]
async fn test_network_connect_missing_container_404() {
    if std::env::var("SPECK_TEST_INTEGRATION").is_err() {
        eprintln!("skipping integration test: SPECK_TEST_INTEGRATION not set");
        return;
    }
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let docker = speck_docker();
        let response = docker
            .create_network(NetworkCreateRequest {
                name: "test-net-conn-validate".to_string(),
                ..Default::default()
            })
            .await
            .expect("create network");

        let err = docker
            .connect_network(
                &response.id,
                NetworkConnectRequest {
                    container: "speck-missing-container".to_string(),
                    endpoint_config: None,
                },
            )
            .await
            .expect_err("missing container should return 404");
        assert!(matches!(
            err,
            bollard::errors::Error::DockerResponseServerError {
                status_code: 404,
                ..
            }
        ));

        docker
            .remove_network(&response.id)
            .await
            .expect("remove network");
    })
    .await;
    assert!(
        result.is_ok(),
        "test_network_connect_missing_container_404 timed out after 30s"
    );
}

#[tokio::test]
async fn test_container_wait_exit_code() {
    if std::env::var("SPECK_TEST_INTEGRATION").is_err() {
        eprintln!("skipping integration test: SPECK_TEST_INTEGRATION not set");
        return;
    }
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let docker = speck_docker();
        let config = ContainerCreateBody {
            image: Some("alpine".to_string()),
            cmd: Some(vec![
                "sh".to_string(),
                "-c".to_string(),
                "exit 42".to_string(),
            ]),
            ..Default::default()
        };
        let options = CreateContainerOptionsBuilder::default()
            .name("test-conformance-wait-exit-code")
            .build();
        let response = docker
            .create_container(Some(options), config)
            .await
            .expect("create container");
        let container_id = response.id;

        docker
            .start_container(&container_id, None::<StartContainerOptions>)
            .await
            .expect("start container");

        let wait = docker
            .wait_container(&container_id, None::<WaitContainerOptions>)
            .try_collect::<Vec<_>>()
            .await
            .expect("wait container");
        assert_eq!(
            wait.first().map(|r| r.status_code),
            Some(42),
            "exit code should be 42"
        );

        docker
            .remove_container(
                &container_id,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
            .expect("remove container");
    })
    .await;
    assert!(
        result.is_ok(),
        "test_container_wait_exit_code timed out after 30s"
    );
}

#[tokio::test]
#[ignore = "requires signed binary + spk up running"]
async fn test_exec_create_start() {
    let docker = speck_docker();
    let config = ContainerCreateBody {
        image: Some("alpine".to_string()),
        cmd: Some(vec!["sleep".to_string(), "30".to_string()]),
        ..Default::default()
    };
    let options = CreateContainerOptionsBuilder::default()
        .name("test-conformance-exec")
        .build();
    let response = docker
        .create_container(Some(options), config)
        .await
        .expect("create container");
    let container_id = response.id;

    docker
        .start_container(&container_id, None::<StartContainerOptions>)
        .await
        .expect("start container");

    let exec = docker
        .create_exec(
            &container_id,
            ExecConfig {
                cmd: Some(vec!["echo".to_string(), "from-exec".to_string()]),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                ..Default::default()
            },
        )
        .await
        .expect("create exec");

    let output = docker.start_exec(&exec.id, None).await.expect("start exec");
    let output_text = match output {
        StartExecResults::Attached { output, .. } => {
            let lines: Vec<_> = output.try_collect().await.expect("exec output stream");
            lines.iter().map(|l| l.to_string()).collect::<String>()
        }
        StartExecResults::Detached => String::new(),
    };
    assert!(
        output_text.contains("from-exec"),
        "exec output should contain 'from-exec', got: {output_text:?}"
    );

    // Verify the exec process exit code via inspect_exec.
    let inspect = docker
        .inspect_exec(&exec.id)
        .await
        .expect("inspect exec should succeed");
    assert_eq!(
        inspect.exit_code,
        Some(0),
        "exec exit code should be 0, got: {:?}",
        inspect.exit_code,
    );

    docker
        .remove_container(
            &container_id,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await
        .expect("remove container");
}

#[tokio::test]
#[ignore = "requires signed binary + spk up running"]
async fn test_events_stream_starts() {
    let docker = speck_docker();
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        let mut stream = docker.events(None::<bollard::query_parameters::EventsOptions>);
        let _first = stream.next().await;
    })
    .await;
}

#[tokio::test]
#[ignore = "requires signed binary + spk up running"]
async fn test_network_create_list_remove() {
    let docker = speck_docker();
    let response = docker
        .create_network(NetworkCreateRequest {
            name: "test-net-conformance".to_string(),
            ..Default::default()
        })
        .await
        .expect("create network");

    let networks = docker
        .list_networks(None::<bollard::query_parameters::ListNetworksOptions>)
        .await
        .expect("list networks");
    assert!(
        networks
            .iter()
            .any(|n| n.name.as_deref() == Some("test-net-conformance")),
        "test-net-conformance should appear in network list"
    );

    docker
        .remove_network(&response.id)
        .await
        .expect("remove network");
}

#[tokio::test]
#[ignore = "requires signed binary + spk up running"]
async fn test_volume_create_list_remove() {
    let docker = speck_docker();
    let _volume = docker
        .create_volume(VolumeCreateRequest {
            name: Some("test-vol-conformance".to_string()),
            ..Default::default()
        })
        .await
        .expect("create volume");

    let volumes = docker
        .list_volumes(None::<bollard::query_parameters::ListVolumesOptions>)
        .await
        .expect("list volumes");
    let vol_names: Vec<_> = volumes
        .volumes
        .unwrap_or_default()
        .into_iter()
        .map(|v| v.name)
        .collect();
    assert!(
        vol_names.iter().any(|n| n == "test-vol-conformance"),
        "test-vol-conformance should appear in volume list: {vol_names:?}"
    );

    docker
        .remove_volume(
            "test-vol-conformance",
            None::<bollard::query_parameters::RemoveVolumeOptions>,
        )
        .await
        .expect("remove volume");
}

// ─── End-to-end integration tests ─────────────────────────────────────────
//
// These tests exercise features that must work end-to-end through the
// transparent vsock proxy.
//
// Prerequisites:
//   - Signed speck binary (`cargo xtask codesign-dev`)
//   - `spk up` running
//   - For DNS tests: external network access from within the VM
//
// Run:
//   cargo test -p speck-dockerd --test api_conformance -- --ignored
// ─────────────────────────────────────────────────────────────────────────────

/// Verify that a host path bind-mounted via `-v src:dst:ro` is readable inside a container.
///
/// This exercises the VirtioFS identity-mount path: the host path must be mounted at
/// the same absolute path inside the guest so Podman can find it when resolving the
/// bind spec without any JSON rewriting in the Speck proxy.
#[tokio::test]
#[ignore = "requires signed binary + spk up running + /private/tmp identity mount"]
async fn test_bind_mount_host_path() {
    let tmp_dir = std::env::temp_dir(); // /private/tmp on macOS
    let tmp_file = tmp_dir.join("speck-bind-test.txt");
    std::fs::write(&tmp_file, "bind-mount-ok\n").expect("write bind test file");

    let docker = speck_docker();
    let config = ContainerCreateBody {
        image: Some("alpine".to_string()),
        cmd: Some(vec![
            "cat".to_string(),
            format!("{}/speck-bind-test.txt", tmp_dir.display()),
        ]),
        host_config: Some(HostConfig {
            binds: Some(vec![format!(
                "{}:{}:ro",
                tmp_dir.display(),
                tmp_dir.display()
            )]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let options = CreateContainerOptionsBuilder::default()
        .name("test-bind-mount-backend")
        .build();
    let response = docker
        .create_container(Some(options), config)
        .await
        .expect("create container with bind mount");
    let container_id = response.id;

    docker
        .start_container(&container_id, None::<StartContainerOptions>)
        .await
        .expect("start container");

    docker
        .wait_container(&container_id, None::<WaitContainerOptions>)
        .try_collect::<Vec<_>>()
        .await
        .expect("wait container");

    let logs = docker
        .logs(
            &container_id,
            Some(LogsOptions {
                stdout: true,
                stderr: true,
                ..Default::default()
            }),
        )
        .try_collect::<Vec<_>>()
        .await
        .expect("get bind mount logs");
    let output: String = logs
        .iter()
        .flat_map(|l| l.as_ref())
        .map(|&b| b as char)
        .collect();

    docker
        .remove_container(
            &container_id,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await
        .expect("remove container");

    let _ = std::fs::remove_file(&tmp_file);

    assert!(
        output.contains("bind-mount-ok"),
        "bind-mounted file content should be readable inside container, got: {output:?}"
    );
}

#[tokio::test]
#[ignore = "requires signed binary + spk up running + live bind rewrite coverage for host edits after container start"]
async fn test_bind_mount_host_path_live_updates() {
    let tmp_dir = std::env::temp_dir().join("speck-bind-live-updates");
    std::fs::create_dir_all(&tmp_dir).expect("create bind source dir");
    let tmp_file = tmp_dir.join("live-update.txt");
    std::fs::write(&tmp_file, "before-update\n").expect("write initial bind file");

    let docker = speck_docker();
    let _ = docker
        .remove_container(
            "test-bind-mount-live-updates",
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await;
    let config = ContainerCreateBody {
        image: Some("alpine".to_string()),
        cmd: Some(vec![
            "sh".to_string(),
            "-lc".to_string(),
            "sleep 5; cat /bind/live-update.txt".to_string(),
        ]),
        host_config: Some(HostConfig {
            binds: Some(vec![format!("{}:/bind:rw", tmp_dir.display())]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let options = CreateContainerOptionsBuilder::default()
        .name("test-bind-mount-live-updates")
        .build();
    let response = docker
        .create_container(Some(options), config)
        .await
        .expect("create container with bind mount");
    let container_id = response.id;

    docker
        .start_container(&container_id, None::<StartContainerOptions>)
        .await
        .expect("start container");

    tokio::time::sleep(Duration::from_secs(1)).await;
    std::fs::write(&tmp_file, "after-update\n").expect("update bind file after start");

    docker
        .wait_container(&container_id, None::<WaitContainerOptions>)
        .try_collect::<Vec<_>>()
        .await
        .expect("wait container");

    let logs = docker
        .logs(
            &container_id,
            Some(LogsOptions {
                stdout: true,
                stderr: true,
                ..Default::default()
            }),
        )
        .try_collect::<Vec<_>>()
        .await
        .expect("get bind mount logs");
    let output_text: String = logs
        .iter()
        .flat_map(|l| l.as_ref())
        .map(|&b| b as char)
        .collect();

    docker
        .remove_container(
            &container_id,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await
        .expect("remove container");

    let _ = std::fs::remove_file(&tmp_file);
    let _ = std::fs::remove_dir_all(&tmp_dir);

    assert!(
        output_text.contains("after-update"),
        "running container should observe host file updates after start, got: {output_text:?}"
    );
}

/// Verify that a port-published container starts without error and the binding
/// is negotiated through the Speck proxy without reintroducing VZNATNetworkDeviceAttachment.
#[tokio::test]
#[ignore = "requires signed binary + spk up running + nginx:alpine image"]
async fn test_port_publish_nginx() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let docker = speck_docker();
    let _ = docker
        .remove_container(
            "test-port-publish-backend",
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await;
    let mut port_bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();
    port_bindings.insert(
        "80/tcp".to_string(),
        Some(vec![PortBinding {
            host_ip: Some("127.0.0.1".to_string()),
            host_port: Some("18080".to_string()),
        }]),
    );

    let config = ContainerCreateBody {
        image: Some("nginx:alpine".to_string()),
        host_config: Some(HostConfig {
            port_bindings: Some(port_bindings),
            ..Default::default()
        }),
        ..Default::default()
    };
    let options = CreateContainerOptionsBuilder::default()
        .name("test-port-publish-backend")
        .build();
    let response = docker
        .create_container(Some(options), config)
        .await
        .expect("create nginx container");
    let container_id = response.id;

    docker
        .start_container(&container_id, None::<StartContainerOptions>)
        .await
        .expect("start nginx");

    // Give nginx a moment to bind its port.
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify actual HTTP traffic through the published port on the macOS host.
    // This exercises the full path: host → smoltcp port forwarding → VM → nginx.
    let mut tcp = TcpStream::connect("127.0.0.1:18080")
        .await
        .expect("TCP connect to published port 18080 should succeed");

    tcp.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .expect("write HTTP request to published port");
    let mut response = Vec::new();
    tcp.read_to_end(&mut response)
        .await
        .expect("read HTTP response from published port");

    let response_str = String::from_utf8_lossy(&response);
    assert!(
        response_str.contains("nginx"),
        "HTTP response from published port should contain 'nginx', got: {response_str:.80}"
    );

    docker
        .remove_container(
            &container_id,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await
        .expect("remove nginx container");
}

/// Verify that stop/remove both tear down localhost listeners and that a forced
/// remove allows the same host port to be rebound immediately.
#[tokio::test]
#[ignore = "requires signed binary + spk up running + nginx:alpine image + live localhost listener cleanup"]
async fn test_port_publish_stale_listener_cleanup() {
    async fn assert_published_http(port: u16) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut tcp = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("published port should accept localhost connections");
        tcp.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .expect("write HTTP request to published port");

        let mut response = Vec::new();
        tcp.read_to_end(&mut response)
            .await
            .expect("read HTTP response from published port");

        let response_str = String::from_utf8_lossy(&response);
        assert!(
            response_str.contains("nginx"),
            "HTTP response from published port should contain 'nginx', got: {response_str:.80}"
        );
    }

    let docker = speck_docker();
    let host_port = 18099;
    let primary_name = "test-port-cleanup-primary";
    let rebound_name = "test-port-cleanup-rebind";

    let _ = docker
        .remove_container(
            primary_name,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await;
    let _ = docker
        .remove_container(
            rebound_name,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await;

    let mut port_bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();
    port_bindings.insert(
        "80/tcp".to_string(),
        Some(vec![PortBinding {
            host_ip: Some("127.0.0.1".to_string()),
            host_port: Some(host_port.to_string()),
        }]),
    );

    let config = ContainerCreateBody {
        image: Some("nginx:alpine".to_string()),
        host_config: Some(HostConfig {
            port_bindings: Some(port_bindings.clone()),
            ..Default::default()
        }),
        ..Default::default()
    };

    let primary_id = docker
        .create_container(
            Some(CreateContainerOptionsBuilder::default().name(primary_name).build()),
            config,
        )
        .await
        .expect("create primary nginx container")
        .id;

    docker
        .start_container(&primary_id, None::<StartContainerOptions>)
        .await
        .expect("start primary nginx");
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_published_http(host_port).await;

    // Stop by short container ID to exercise id-resolution cleanup separately
    // from the alias-based rm -f path below.
    let primary_short_id = primary_id.chars().take(12).collect::<String>();
    docker
        .stop_container(
            &primary_short_id,
            None::<bollard::query_parameters::StopContainerOptions>,
        )
        .await
        .expect("stop primary nginx by short id");
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        TcpStream::connect(("127.0.0.1", host_port)).await.is_err(),
        "localhost port {host_port} should close after stop"
    );

    docker
        .start_container(&primary_short_id, None::<StartContainerOptions>)
        .await
        .expect("restart primary nginx by short id");
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_published_http(host_port).await;

    docker
        .remove_container(
            primary_name,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await
        .expect("force-remove primary nginx by alias");
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        TcpStream::connect(("127.0.0.1", host_port)).await.is_err(),
        "localhost port {host_port} should close after rm -f"
    );

    let rebound_id = docker
        .create_container(
            Some(CreateContainerOptionsBuilder::default().name(rebound_name).build()),
            ContainerCreateBody {
                image: Some("nginx:alpine".to_string()),
                host_config: Some(HostConfig {
                    port_bindings: Some(port_bindings),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .expect("create rebound nginx container")
        .id;

    docker
        .start_container(&rebound_id, None::<StartContainerOptions>)
        .await
        .expect("start rebound nginx");
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_published_http(host_port).await;

    docker
        .remove_container(
            rebound_name,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await
        .expect("remove rebound nginx container");
}

/// Verify that DNS resolves inside a container, confirming that Speck's host-resolver
/// path is used rather than a backend-managed DNS that ignores macOS scoped resolvers.
///
/// With WARP/VPN active this same test with a VPN-internal hostname is the critical gate.
#[tokio::test]
#[ignore = "requires signed binary + spk up running + external network access"]
async fn test_dns_resolution_inside_container() {
    let docker = speck_docker();
    let config = ContainerCreateBody {
        image: Some("alpine".to_string()),
        cmd: Some(vec!["nslookup".to_string(), "cloudflare.com".to_string()]),
        ..Default::default()
    };
    let options = CreateContainerOptionsBuilder::default()
        .name("test-dns-backend")
        .build();
    let response = docker
        .create_container(Some(options), config)
        .await
        .expect("create dns-test container");
    let container_id = response.id;

    docker
        .start_container(&container_id, None::<StartContainerOptions>)
        .await
        .expect("start container");

    let wait_result = docker
        .wait_container(&container_id, None::<WaitContainerOptions>)
        .try_collect::<Vec<_>>()
        .await
        .expect("wait container");
    let exit_code = wait_result.first().map(|r| r.status_code).unwrap_or(1);

    docker
        .remove_container(
            &container_id,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await
        .expect("remove container");

    assert_eq!(
        exit_code, 0,
        "nslookup cloudflare.com should succeed inside container (exit 0)"
    );
}

/// Testcontainers smoke test: the Ryuk resource reaper bind-mounts the Docker socket at
/// `/var/run/docker.sock` inside a container and issues Docker API requests through it.
///
/// This test verifies that:
///   1. Speck's Unix socket can be bind-mounted into a container as `/var/run/docker.sock`.
///   2. The mounted path is a valid Unix domain socket (`test -S`), which Ryuk checks
///      before opening the connection.
///
/// For a full Testcontainers run, set these env vars and run your language SDK's test suite:
///   DOCKER_HOST=unix://$HOME/.local/share/speck/speck.sock
///   TESTCONTAINERS_DOCKER_SOCKET_OVERRIDE=$HOME/.local/share/speck/speck.sock
#[tokio::test]
#[ignore = "requires signed binary + spk up running"]
async fn test_ryuk_socket_bind_mount() {
    let docker = speck_docker();
    let sock = speck_sock();

    let config = ContainerCreateBody {
        image: Some("alpine".to_string()),
        cmd: Some(vec![
            "sh".to_string(),
            "-c".to_string(),
            "test -S /var/run/docker.sock && echo ryuk-sock-ok".to_string(),
        ]),
        host_config: Some(HostConfig {
            binds: Some(vec![format!("{}:/var/run/docker.sock", sock.display())]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let options = CreateContainerOptionsBuilder::default()
        .name("test-ryuk-socket-backend")
        .build();
    let response = docker
        .create_container(Some(options), config)
        .await
        .expect("create ryuk-style container");
    let container_id = response.id;

    docker
        .start_container(&container_id, None::<StartContainerOptions>)
        .await
        .expect("start container");

    docker
        .wait_container(&container_id, None::<WaitContainerOptions>)
        .try_collect::<Vec<_>>()
        .await
        .expect("wait container");

    let logs = docker
        .logs(
            &container_id,
            Some(LogsOptions {
                stdout: true,
                stderr: true,
                ..Default::default()
            }),
        )
        .try_collect::<Vec<_>>()
        .await
        .expect("get ryuk logs");
    let output: String = logs
        .iter()
        .flat_map(|l| l.as_ref())
        .map(|&b| b as char)
        .collect();

    docker
        .remove_container(
            &container_id,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await
        .expect("remove container");

    assert!(
        output.contains("ryuk-sock-ok"),
        "Speck socket bind-mounted as /var/run/docker.sock should be a socket inside the container, got: {output:?}"
    );
}
