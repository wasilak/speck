use std::net::Ipv4Addr;

#[derive(Debug, Clone)]
pub struct NetworkConfig {
    pub subnet_prefix: u8,
    pub gateway: Ipv4Addr,
    pub guest_ip: Ipv4Addr,
    pub mtu: u16,
    pub mac: [u8; 6],
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            subnet_prefix: 24,
            gateway: Ipv4Addr::new(172, 16, 0, 1),
            guest_ip: Ipv4Addr::new(172, 16, 0, 2),
            mtu: 1500,
            mac: [0x02, 0x00, 0x00, 0x00, 0x00, 0x01],
        }
    }
}

impl NetworkConfig {
    pub fn builder() -> NetworkConfigBuilder {
        NetworkConfigBuilder::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct NetworkConfigBuilder {
    subnet_prefix: Option<u8>,
    gateway: Option<Ipv4Addr>,
    guest_ip: Option<Ipv4Addr>,
    mtu: Option<u16>,
    mac: Option<[u8; 6]>,
}

impl NetworkConfigBuilder {
    pub fn subnet_prefix(mut self, prefix: u8) -> Self {
        self.subnet_prefix = Some(prefix);
        self
    }

    pub fn gateway(mut self, ip: Ipv4Addr) -> Self {
        self.gateway = Some(ip);
        self
    }

    pub fn guest_ip(mut self, ip: Ipv4Addr) -> Self {
        self.guest_ip = Some(ip);
        self
    }

    pub fn mtu(mut self, mtu: u16) -> Self {
        self.mtu = Some(mtu.max(1500));
        self
    }

    pub fn mac(mut self, mac: [u8; 6]) -> Self {
        self.mac = Some(mac);
        self
    }

    pub fn build(self) -> NetworkConfig {
        let defaults = NetworkConfig::default();
        NetworkConfig {
            subnet_prefix: self.subnet_prefix.unwrap_or(defaults.subnet_prefix),
            gateway: self.gateway.unwrap_or(defaults.gateway),
            guest_ip: self.guest_ip.unwrap_or(defaults.guest_ip),
            mtu: self.mtu.unwrap_or(defaults.mtu),
            mac: self.mac.unwrap_or(defaults.mac),
        }
    }
}
