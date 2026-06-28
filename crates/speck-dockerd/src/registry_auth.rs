use std::fs;
use std::path::PathBuf;

use base64::Engine;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryCredentials {
    pub username: String,
    pub password: String, // secret field, not logged
    pub server: String,
}

#[derive(Deserialize)]
struct HeaderAuth {
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String, // secret field, not logged
    #[serde(default, rename = "serveraddress")]
    server_address: String,
}

#[derive(Deserialize)]
struct DockerConfig {
    #[serde(default)]
    auths: std::collections::HashMap<String, DockerAuthEntry>,
}

#[derive(Deserialize)]
struct DockerAuthEntry {
    auth: Option<String>,
    username: Option<String>,
    password: Option<String>, // secret field, not logged
}

pub fn decode_registry_auth_header(header_value: &str) -> Option<RegistryCredentials> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(header_value)
        .ok()?;
    let auth: HeaderAuth = serde_json::from_slice(&decoded).ok()?;
    if auth.username.is_empty() || auth.password.is_empty() {
        // secret field, not logged
        return None;
    }

    tracing::debug!(username = %auth.username, server = %auth.server_address, "decoded registry credentials from X-Registry-Auth");
    Some(RegistryCredentials {
        username: auth.username,
        password: auth.password, // secret field, not logged
        server: auth.server_address,
    })
}

pub fn parse_docker_config(server: &str) -> Option<RegistryCredentials> {
    let path = docker_config_path()?;
    let contents = fs::read(path).ok()?;
    let config: DockerConfig = serde_json::from_slice(&contents).ok()?;
    let entry = config
        .auths
        .get(server)
        .or_else(|| config.auths.get(&normalize_docker_hub_server(server)))?;

    if let (Some(username), Some(password)) = (&entry.username, &entry.password) {
        // secret field, not logged
        tracing::debug!(username = %username, server = %server, "loaded registry credentials from Docker config");
        return Some(RegistryCredentials {
            username: username.clone(),
            password: password.clone(), // secret field, not logged
            server: server.to_owned(),
        });
    }

    let encoded = entry.auth.as_deref()?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (username, password) = decoded.split_once(':')?; // secret field, not logged
    if username.is_empty() || password.is_empty() {
        // secret field, not logged
        return None;
    }

    tracing::debug!(username = %username, server = %server, "loaded registry auth token from Docker config");
    Some(RegistryCredentials {
        username: username.to_owned(),
        password: password.to_owned(), // secret field, not logged
        server: server.to_owned(),
    })
}

fn docker_config_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var("HOME").ok()?).join(".docker/config.json"))
}

fn normalize_docker_hub_server(server: &str) -> String {
    if server == "registry-1.docker.io" || server == "docker.io" || server == "index.docker.io" {
        "https://index.docker.io/v1/".to_owned()
    } else {
        server.to_owned()
    }
}
