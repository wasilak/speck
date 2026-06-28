use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ContainerState {
    Created,
    Running,
    Paused,
    Restarting,
    Removing,
    Exited,
    Dead,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortBinding {
    pub host_ip: Option<String>,
    pub host_port: u16,
    pub container_port: u16,
    pub proto: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountSpec {
    pub host_path: PathBuf,
    pub container_path: PathBuf,
    pub read_only: bool,
    pub volume_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerSpec {
    pub image: String,
    pub cmd: Vec<String>,
    pub env: Vec<String>,
    pub port_bindings: Vec<PortBinding>,
    pub mounts: Vec<MountSpec>,
    pub name: Option<String>,
    pub restart_policy: Option<String>,
    pub memory_limit: Option<u64>,
    pub cpu_shares: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ContainerSummary {
    pub id: String,
    pub names: Vec<String>,
    pub image: String,
    pub state: ContainerState,
    pub status: String,
    pub created: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ContainerInspect {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: ContainerState,
    pub status: String,
    pub port_bindings: Vec<PortBinding>,
    pub mounts: Vec<MountSpec>,
    pub env: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_container_state_serialize() {
        let serialized = serde_json::to_string(&ContainerState::Running).unwrap();

        assert_eq!(serialized, "\"Running\"");
    }

    #[test]
    fn test_port_binding_roundtrip() {
        let binding = PortBinding {
            host_ip: Some("127.0.0.1".into()),
            host_port: 8080,
            container_port: 80,
            proto: "tcp".into(),
        };

        let serialized = serde_json::to_string(&binding).unwrap();
        let deserialized: PortBinding = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized, binding);
    }
}
