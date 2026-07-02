use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use tracing::Instrument;

use crate::buildkit::LocalBuildContext;
use crate::state::AppState;

pub const MAX_BUILD_CONTEXT_BYTES: usize = 256 * 1024 * 1024;

static BUILD_CONTEXT_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub struct BuildQuery {
    dockerfile: Option<String>,
    t: Option<String>,
    remote: Option<String>,
    q: Option<bool>,
    nocache: Option<bool>,
    cachefrom: Option<String>,
    pull: Option<bool>,
    rm: Option<bool>,
    forcerm: Option<bool>,
    labels: Option<String>,
    buildargs: Option<String>,
    shmsize: Option<i64>,
    ulimits: Option<String>,
    networkmode: Option<String>,
    platform: Option<String>,
    target: Option<String>,
    outputs: Option<String>,
}

pub async fn build(
    State(state): State<AppState>,
    Query(query): Query<BuildQuery>,
    body: Bytes,
) -> impl IntoResponse {
    let build_context = match BuildContext::from_bytes(body) {
        Ok(context) => context,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"message": e})),
            );
        }
    };

    let buildkit = match state.buildkit_client().await {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"message": format!("BuildKit unavailable: {e}")})),
            );
        }
    };

    let tag = query.t.clone().unwrap_or_else(|| "latest".into());
    let frontend = if query.remote.is_some() {
        "gateway.v0"
    } else {
        "dockerfile.v0"
    };

    let mut frontend_attrs = frontend_attrs_from_query(&query);
    frontend_attrs.insert(
        "context:local".into(),
        build_context.staged_tar_path().display().to_string(),
    );
    if build_context.has_copy_or_add_source(query.dockerfile.as_deref().unwrap_or("Dockerfile")) {
        frontend_attrs.insert("speck.context.has-copy-add".into(), "true".into());
    }

    let local_context = LocalBuildContext {
        session_id: format!(
            "speck-build-{}-{}",
            std::process::id(),
            BUILD_CONTEXT_COUNTER.fetch_add(1, Ordering::Relaxed)
        ),
        context_name: "context".into(),
        dockerfile_name: "dockerfile".into(),
        tar_path: build_context.staged_tar_path().to_path_buf(),
    };

    let span = tracing::info_span!("buildkit_solve", ref_ = %tag);
    match buildkit
        .solve_with_local_context(tag.clone(), frontend.into(), frontend_attrs, &local_context)
        .instrument(span)
        .await
    {
        Ok(_) => {
            let body = serde_json::json!({"stream": format!("Build complete for {tag}\n")});
            (StatusCode::OK, Json(body))
        }
        Err(e) => {
            let body = serde_json::json!({"message": format!("Build failed: {e}")});
            (StatusCode::INTERNAL_SERVER_ERROR, Json(body))
        }
    }
}

fn frontend_attrs_from_query(query: &BuildQuery) -> HashMap<String, String> {
    let mut frontend_attrs: HashMap<String, String> = HashMap::new();
    if let Some(df) = &query.dockerfile {
        frontend_attrs.insert("filename".into(), df.clone());
    }
    if let Some(target) = &query.target {
        frontend_attrs.insert("target".into(), target.clone());
    }
    if let Some(buildargs) = &query.buildargs {
        if let Ok(args) = serde_json::from_str::<HashMap<String, String>>(buildargs) {
            for (k, v) in args {
                frontend_attrs.insert(format!("build-arg:{k}"), v);
            }
        }
    }
    if let Some(labels) = &query.labels {
        if let Ok(labs) = serde_json::from_str::<HashMap<String, String>>(labels) {
            for (k, v) in labs {
                frontend_attrs.insert(format!("label:{k}"), v);
            }
        }
    }
    if let Some(platform) = &query.platform {
        frontend_attrs.insert("platform".into(), platform.clone());
    }
    if let Some(nocache) = query.nocache {
        if nocache {
            frontend_attrs.insert("no-cache".into(), "".into());
        }
    }
    if let Some(cachefrom) = &query.cachefrom {
        frontend_attrs.insert("cache-from".into(), cachefrom.clone());
    }

    frontend_attrs
}

#[derive(Debug)]
struct BuildContext {
    staged_tar_path: PathBuf,
    entries: Vec<TarEntrySummary>,
}

#[derive(Debug)]
struct TarEntrySummary {
    path: String,
    data: Vec<u8>,
}

impl BuildContext {
    fn from_bytes(bytes: Bytes) -> Result<Self, String> {
        if bytes.len() > MAX_BUILD_CONTEXT_BYTES {
            return Err(format!(
                "build context exceeds {} byte limit",
                MAX_BUILD_CONTEXT_BYTES
            ));
        }

        let entries = summarize_tar_entries(&bytes)?;
        let staged_tar_path = stage_context_tar(&bytes)?;

        Ok(Self {
            staged_tar_path,
            entries,
        })
    }

    fn staged_tar_path(&self) -> &Path {
        &self.staged_tar_path
    }

    fn has_copy_or_add_source(&self, dockerfile_path: &str) -> bool {
        let Some(dockerfile) = self
            .entries
            .iter()
            .find(|entry| entry.path == dockerfile_path)
        else {
            return false;
        };
        let Ok(text) = std::str::from_utf8(&dockerfile.data) else {
            return false;
        };

        text.lines().any(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("COPY ") || trimmed.starts_with("ADD ")
        })
    }
}

fn stage_context_tar(bytes: &[u8]) -> Result<PathBuf, String> {
    let mut path = std::env::temp_dir();
    let id = BUILD_CONTEXT_COUNTER.fetch_add(1, Ordering::Relaxed);
    path.push(format!(
        "speck-build-context-{}-{id}.tar",
        std::process::id()
    ));
    std::fs::write(&path, bytes).map_err(|e| format!("stage build context: {e}"))?;
    Ok(path)
}

fn summarize_tar_entries(bytes: &[u8]) -> Result<Vec<TarEntrySummary>, String> {
    let mut entries = Vec::new();
    let mut offset = 0_usize;
    while offset + 512 <= bytes.len() {
        let header = &bytes[offset..offset + 512];
        if header.iter().all(|b| *b == 0) {
            break;
        }
        let path = parse_tar_path(header)?;
        validate_tar_path(&path)?;
        let size = parse_tar_size(header)?;
        let data_start = offset + 512;
        let data_end = data_start
            .checked_add(size)
            .ok_or_else(|| "build context tar entry size overflow".to_string())?;
        if data_end > bytes.len() {
            return Err("build context tar entry extends past archive".into());
        }
        entries.push(TarEntrySummary {
            path,
            data: bytes[data_start..data_end].to_vec(),
        });
        let padded_size = size.div_ceil(512) * 512;
        offset = data_start
            .checked_add(padded_size)
            .ok_or_else(|| "build context tar offset overflow".to_string())?;
    }

    Ok(entries)
}

fn parse_tar_path(header: &[u8]) -> Result<String, String> {
    let end = header[..100].iter().position(|b| *b == 0).unwrap_or(100);
    std::str::from_utf8(&header[..end])
        .map(|s| s.to_string())
        .map_err(|_| "build context tar path is not UTF-8".into())
}

fn parse_tar_size(header: &[u8]) -> Result<usize, String> {
    let raw = &header[124..136];
    let end = raw
        .iter()
        .position(|b| *b == 0 || *b == b' ')
        .unwrap_or(raw.len());
    let text = std::str::from_utf8(&raw[..end]).map_err(|_| "invalid tar size".to_string())?;
    usize::from_str_radix(text.trim(), 8).map_err(|_| "invalid tar size".to_string())
}

fn validate_tar_path(path: &str) -> Result<(), String> {
    let path = Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("build context tar contains unsafe path".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_context_rejects_inputs_over_explicit_limit() {
        let too_large = vec![0_u8; MAX_BUILD_CONTEXT_BYTES + 1];

        let err =
            BuildContext::from_bytes(too_large.into()).expect_err("oversized context rejected");

        assert!(err.contains("build context exceeds"));
    }

    #[test]
    fn build_context_accepts_tar_with_copy_source() {
        let tar = test_tar(&[
            (
                "Dockerfile",
                b"FROM scratch\nCOPY hello.txt /hello.txt\n".as_slice(),
            ),
            ("hello.txt", b"hello from context\n".as_slice()),
        ]);

        let context = BuildContext::from_bytes(tar.into()).expect("valid context");

        assert!(context.has_copy_or_add_source("Dockerfile"));
        assert!(context.staged_tar_path().exists());
    }

    #[test]
    fn frontend_attrs_preserve_existing_build_query_mapping() {
        let query = BuildQuery {
            dockerfile: Some("Containerfile".into()),
            t: Some("example:latest".into()),
            target: Some("prod".into()),
            platform: Some("linux/arm64".into()),
            nocache: Some(true),
            buildargs: Some(r#"{"FEATURE":"on"}"#.into()),
            labels: Some(r#"{"org.opencontainers.image.title":"speck"}"#.into()),
            cachefrom: Some("type=registry,ref=example/cache".into()),
            ..Default::default()
        };

        let attrs = frontend_attrs_from_query(&query);

        assert_eq!(attrs.get("filename"), Some(&"Containerfile".to_string()));
        assert_eq!(attrs.get("target"), Some(&"prod".to_string()));
        assert_eq!(attrs.get("platform"), Some(&"linux/arm64".to_string()));
        assert_eq!(attrs.get("no-cache"), Some(&String::new()));
        assert_eq!(attrs.get("build-arg:FEATURE"), Some(&"on".to_string()));
        assert_eq!(
            attrs.get("label:org.opencontainers.image.title"),
            Some(&"speck".to_string())
        );
        assert_eq!(
            attrs.get("cache-from"),
            Some(&"type=registry,ref=example/cache".to_string())
        );
    }

    fn test_tar(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar = Vec::new();
        for (path, data) in entries {
            let mut header = [0_u8; 512];
            write_bytes(&mut header[0..100], path.as_bytes());
            write_octal(&mut header[100..108], 0o644);
            write_octal(&mut header[108..116], 0);
            write_octal(&mut header[116..124], 0);
            write_octal(&mut header[124..136], data.len() as u64);
            write_octal(&mut header[136..148], 0);
            for byte in &mut header[148..156] {
                *byte = b' ';
            }
            header[156] = b'0';
            write_bytes(&mut header[257..263], b"ustar\0");
            write_bytes(&mut header[263..265], b"00");
            let checksum: u32 = header.iter().map(|b| u32::from(*b)).sum();
            write_checksum(&mut header[148..156], checksum);

            tar.extend_from_slice(&header);
            tar.extend_from_slice(data);
            let padding = (512 - (data.len() % 512)) % 512;
            tar.extend(std::iter::repeat_n(0, padding));
        }
        tar.extend_from_slice(&[0_u8; 1024]);
        tar
    }

    fn write_bytes(dst: &mut [u8], src: &[u8]) {
        let len = src.len().min(dst.len());
        dst[..len].copy_from_slice(&src[..len]);
    }

    fn write_octal(dst: &mut [u8], value: u64) {
        let text = format!("{:0width$o}\0", value, width = dst.len() - 1);
        write_bytes(dst, text.as_bytes());
    }

    fn write_checksum(dst: &mut [u8], value: u32) {
        let text = format!("{:06o}\0 ", value);
        write_bytes(dst, text.as_bytes());
    }
}
