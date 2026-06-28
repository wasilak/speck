use std::fmt;

use base64::Engine;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageRef(pub String);

impl ImageRef {
    pub fn parse(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl fmt::Display for ImageRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImageSummary {
    pub id: String,
    pub repo_tags: Vec<String>,
    pub size: i64,
    pub created: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImageInspect {
    pub id: String,
    pub repo_tags: Vec<String>,
    pub size: i64,
    pub created: i64,
    pub architecture: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryAuth {
    pub username: String,
    pub password: String,
    pub server_address: String,
}

impl RegistryAuth {
    pub fn to_base64(&self) -> String {
        #[derive(Serialize)]
        struct DockerAuth<'a> {
            username: &'a str,
            password: &'a str,
            serveraddress: &'a str,
        }

        let auth = DockerAuth {
            username: &self.username,
            password: &self.password,
            serveraddress: &self.server_address,
        };
        let json = serde_json::to_vec(&auth).expect("registry auth serialization cannot fail");

        base64::engine::general_purpose::STANDARD.encode(json)
    }
}

impl fmt::Debug for RegistryAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegistryAuth")
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .field("server_address", &self.server_address)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_ref_parse_with_tag() {
        let image = ImageRef::parse("alpine:3.20");

        assert_eq!(image.to_string(), "alpine:3.20");
    }

    #[test]
    fn test_image_ref_parse_without_tag() {
        let image = ImageRef::parse("alpine");

        assert_eq!(image.to_string(), "alpine");
    }

    #[test]
    fn test_registry_auth_base64_is_valid_base64() {
        let auth = RegistryAuth {
            username: "user".into(),
            password: "secret".into(),
            server_address: "registry.example.com".into(),
        };

        let encoded = auth.to_base64();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        let decoded = String::from_utf8(decoded).unwrap();

        assert!(decoded.contains("\"username\":\"user\""));
        assert!(decoded.contains("\"password\":\"secret\""));
        assert!(decoded.contains("\"serveraddress\":\"registry.example.com\""));
    }

    #[test]
    fn registry_auth_debug_masks_password() {
        let auth = RegistryAuth {
            username: "user".into(),
            password: "secret".into(),
            server_address: "registry.example.com".into(),
        };

        let formatted = format!("{auth:?}");

        assert!(formatted.contains("[REDACTED]"));
        assert!(!formatted.contains("secret"));
    }
}
