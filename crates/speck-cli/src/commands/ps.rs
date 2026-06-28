use std::path::Path;

use crate::docker_client::DockerClient;
use crate::theme::{NEON_CYAN, RESET, format_header, format_id, format_status};

pub async fn run_ps(speck_home: &Path) -> anyhow::Result<()> {
    let sock_path = speck_home.join("speck.sock");
    let client = DockerClient::new(&sock_path);

    let resp = client.get("/containers/json?all=true").await?;
    let containers = resp
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("unexpected response format from /containers/json"))?;

    if containers.is_empty() {
        println!("{NEON_CYAN}No containers{RESET}");
        return Ok(());
    }

    println!(
        "{}  {}  {}  {}",
        format_header("CONTAINER ID"),
        format_header("IMAGE"),
        format_header("STATUS"),
        format_header("CREATED"),
    );

    for c in containers {
        let id = c["Id"].as_str().unwrap_or("");
        let image = c["Image"].as_str().unwrap_or("");
        let status = c["State"].as_str().unwrap_or("");
        let created = c["Created"].as_str().unwrap_or("");

        println!(
            "{}  {}  {}  {}",
            format_id(id),
            image,
            format_status(status),
            created,
        );
    }

    Ok(())
}
