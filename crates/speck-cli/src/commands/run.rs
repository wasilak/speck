use std::collections::HashMap;
use std::path::Path;

use indicatif::ProgressBar;
use serde_json::json;

use crate::RunArgs;
use crate::docker_client::DockerClient;
use crate::theme::NEON_CYAN;

pub async fn run_run(args: RunArgs, speck_home: &Path) -> anyhow::Result<()> {
    let sock_path = speck_home.join("speck.sock");
    let client = DockerClient::new(&sock_path);

    let image = &args.image;
    let spinner = ProgressBar::new_spinner();
    spinner.set_message(format!("Pulling {image}..."));
    spinner.enable_steady_tick(std::time::Duration::from_millis(100));

    let pull_path = format!("/images/create?fromImage={image}");
    client.post_empty(&pull_path).await?;
    spinner.finish_and_clear();

    let mut env: Vec<String> = args.env.clone();
    env.push(format!("SPECK_CONTAINER=true"));

    let mut exposed_ports = serde_json::Map::new();
    let mut port_bindings = serde_json::Map::new();
    for port_spec in &args.port {
        if let Some((host, container)) = port_spec.split_once(':') {
            let proto = "tcp";
            let cport: u16 = container.parse()?;
            exposed_ports.insert(format!("{cport}/{proto}"), serde_json::Map::new().into());
            let binding = json!([{
                "HostIp": "0.0.0.0",
                "HostPort": host.to_string()
            }]);
            port_bindings.insert(format!("{cport}/{proto}"), binding);
        }
    }

    let mut binds: Vec<String> = Vec::new();
    for vol in &args.volume {
        if let Some((host_path, container_path)) = vol.split_once(':') {
            let metadata = std::fs::metadata(host_path);
            if metadata.is_err() {
                anyhow::bail!("volume host path does not exist: {host_path}");
            }
            if host_path.contains("..") {
                anyhow::bail!("volume host path must not contain '..': {host_path}");
            }
            binds.push(format!("{host_path}:{container_path}"));
        }
    }

    let create_body = json!({
        "Image": image,
        "Cmd": args.cmd,
        "Env": env,
        "ExposedPorts": exposed_ports,
        "HostConfig": {
            "Binds": binds,
            "PortBindings": port_bindings,
        }
    });

    let create_resp = client.post_json("/containers/create", &create_body).await?;
    let container_id = create_resp["Id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing container ID in create response"))?;
    let container_id = container_id.to_string();

    client
        .post_empty(&format!("/containers/{container_id}/start"))
        .await?;

    if args.detach {
        let short = &container_id[..container_id.len().min(12)];
        println!("{NEON_CYAN}{short}{}", "\x1b[0m");
        return Ok(());
    }

    let attach_path = format!("/containers/{container_id}/attach");
    let attach_body = json!({
        "stdin": true,
        "stdout": true,
        "stderr": true,
        "stream": true,
    });
    let _ = client
        .post_json(
            &format!("{attach_path}?stream=1&stdout=1&stderr=1"),
            &attach_body,
        )
        .await;

    let wait_path = format!("/containers/{container_id}/wait");
    let _wait_resp = client.post_empty(&wait_path).await?;

    Ok(())
}
