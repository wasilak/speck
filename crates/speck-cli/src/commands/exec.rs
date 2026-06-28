use std::path::Path;

use serde_json::json;

use crate::ExecArgs;
use crate::docker_client::DockerClient;

pub async fn run_exec(args: ExecArgs, speck_home: &Path) -> anyhow::Result<()> {
    let sock_path = speck_home.join("speck.sock");
    let client = DockerClient::new(&sock_path);

    let exec_body = json!({
        "Cmd": args.cmd,
        "AttachStdin": true,
        "AttachStdout": true,
        "AttachStderr": true,
        "Tty": args.tty,
    });

    let exec_resp = client
        .post_json(&format!("/containers/{}/exec", args.container), &exec_body)
        .await?;
    let exec_id = exec_resp["Id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing exec ID"))?;
    let exec_id = exec_id.to_string();

    let start_body = json!({
        "Detach": false,
        "Tty": args.tty,
    });

    let _ = client
        .post_json(&format!("/exec/{exec_id}/start"), &start_body)
        .await?;

    Ok(())
}
