use std::path::Path;

use crate::StopArgs;
use crate::docker_client::DockerClient;
use crate::theme::NEON_CYAN;

pub async fn run_stop(args: StopArgs, speck_home: &Path) -> anyhow::Result<()> {
    let sock_path = speck_home.join("speck.sock");
    let client = DockerClient::new(&sock_path);

    client
        .post_empty(&format!("/containers/{}/stop", args.container))
        .await?;

    println!("{NEON_CYAN}Container {} stopped\x1b[0m", args.container);

    Ok(())
}
