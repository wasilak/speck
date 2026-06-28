use bollard::exec::StartExecResults;
use bollard::query_parameters::CreateImageOptionsBuilder;
use bollard::models::{
    ContainerCreateBody, ExecConfig, NetworkCreateRequest, VolumeCreateRequest,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, ListContainersOptions, LogsOptions, RemoveContainerOptions,
    StartContainerOptions, WaitContainerOptions,
};
use bollard::Docker;
use futures_util::StreamExt;
use futures_util::TryStreamExt;
use std::path::PathBuf;
use std::time::Duration;

fn speck_sock() -> PathBuf {
    if let Ok(sock) = std::env::var("SPECK_SOCK") {
        return PathBuf::from(sock);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".local/share/speck/speck.sock")
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
    assert!(!results.is_empty(), "should get at least one progress event");
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
            c.id
                .as_deref()
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
        cmd: Some(vec!["sh".to_string(), "-c".to_string(), "exit 0".to_string()]),
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
#[ignore = "requires signed binary + spk up running"]
async fn test_container_logs() {
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
    let output: String = logs.iter().flat_map(|l| l.as_ref()).map(|&b| b as char).collect();
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

    let output = docker
        .start_exec(&exec.id, None)
        .await
        .expect("start exec");
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
        networks.iter().any(|n| n.name.as_deref() == Some("test-net-conformance")),
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
        .remove_volume("test-vol-conformance", None::<bollard::query_parameters::RemoveVolumeOptions>)
        .await
        .expect("remove volume");
}
