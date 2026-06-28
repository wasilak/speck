use std::path::Path;

use indicatif::ProgressBar;

use crate::BuildArgs;
use crate::docker_client::DockerClient;

pub async fn run_build(args: BuildArgs, speck_home: &Path) -> anyhow::Result<()> {
    let sock_path = speck_home.join("speck.sock");
    let client = DockerClient::new(&sock_path);

    let spinner = ProgressBar::new_spinner();
    spinner.set_message("Creating build context...");
    spinner.enable_steady_tick(std::time::Duration::from_millis(100));

    let mut tar_builder = tar::Builder::new(Vec::new());
    let context_dir = &args.context;
    if context_dir.is_dir() {
        for entry in walkdir::WalkDir::new(context_dir) {
            let entry = entry?;
            if entry.file_type().is_file() {
                let relative = entry.path().strip_prefix(context_dir)?;
                let mut file = std::fs::File::open(entry.path())?;
                tar_builder.append_file(relative, &mut file)?;
            }
        }
    } else if context_dir.is_file() {
        let mut file = std::fs::File::open(context_dir)?;
        tar_builder.append_file(context_dir.file_name().unwrap(), &mut file)?;
    }
    let tar_bytes = tar_builder.into_inner()?;

    spinner.set_message("Sending build to server...");

    let mut build_path = "/build".to_string();
    let mut params: Vec<String> = Vec::new();
    if let Some(tag) = &args.tag {
        params.push(format!("t={tag}"));
    }
    if let Some(df) = &args.dockerfile {
        params.push(format!("dockerfile={df}"));
    }
    if let Some(target) = &args.target {
        params.push(format!("target={target}"));
    }
    if args.no_cache {
        params.push("nocache=1".into());
    }
    if !params.is_empty() {
        build_path = format!("/build?{}", params.join("&"));
    }

    let _resp = client
        .post_body_raw(
            &build_path,
            http_body_util::Full::new(hyper::body::Bytes::from(tar_bytes)),
        )
        .await?;

    spinner.finish_with_message("Build complete");

    Ok(())
}
