use std::path::Path;

use crate::RmArgs;
use crate::docker_client::DockerClient;
use crate::theme::NEON_CYAN;

pub async fn run_rm(args: RmArgs, speck_home: &Path) -> anyhow::Result<()> {
    let sock_path = speck_home.join("speck.sock");
    let client = DockerClient::new(&sock_path);

    client
        .delete(&format!("/containers/{}", args.container))
        .await?;

    println!("{NEON_CYAN}Container {} removed\x1b[0m", args.container);

    Ok(())
}
