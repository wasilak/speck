#[cfg(test)]
mod tests {
    use super::{PortMapConfig, PortPublishBridge};

    #[test]
    fn test_portmap_config_serialization() {
        let config = PortMapConfig {
            host_port: 8080,
            container_port: 80,
        };

        let encoded = serde_json::to_string(&config).unwrap();
        let decoded: PortMapConfig = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, config);
    }

    #[test]
    fn test_portpublish_bridge_new() {
        let bridge = PortPublishBridge::new(1500);

        assert_eq!(bridge.bridge_count(), 0);
        assert_eq!(bridge.port_map_count(), 0);
        assert_eq!(bridge.next_ephemeral_port(), 49152);
    }
}
