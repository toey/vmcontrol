use serde::{Deserialize, Serialize};

fn default_vnc_port() -> u16 { 12001 }

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct VmStartConfig {
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub features: Features,
    pub network_adapters: Vec<NetworkAdapter>,
    pub disks: Vec<DiskInfo>,
    #[serde(default = "default_vnc_port")]
    pub vnc_port: u16,
}

fn default_one() -> String { "1".into() }
fn default_zero() -> String { "0".into() }

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CpuInfo {
    /// Number of vCPUs — if set (>0), sockets/cores/threads are auto-computed
    #[serde(default = "default_zero")]
    pub vcpus: String,
    #[serde(default = "default_one")]
    pub sockets: String,
    #[serde(default = "default_one")]
    pub cores: String,
    #[serde(default = "default_one")]
    pub threads: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MemoryInfo {
    pub size: String,
}

fn default_net_mode() -> String { "nat".into() }
fn default_switch_name() -> String { String::new() }
fn default_bridge_iface() -> String { String::new() }
fn default_arch() -> String { "x86_64".into() }
fn default_cloudinit() -> String { "1".into() }

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Features {
    pub is_windows: String,
    #[serde(default = "default_arch")]
    pub arch: String,
    #[serde(default = "default_cloudinit")]
    pub cloudinit: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct NetworkAdapter {
    pub netid: String,
    pub mac: String,
    pub vlan: String,
    #[serde(default = "default_net_mode")]
    pub mode: String,
    #[serde(default = "default_switch_name")]
    pub switch_name: String,
    #[serde(default = "default_bridge_iface")]
    pub bridge_iface: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DiskInfo {
    pub diskid: String,
    pub diskname: String,
    #[serde(rename = "iops-total")]
    pub iops_total: String,
    #[serde(rename = "iops-total-max")]
    pub iops_total_max: String,
    #[serde(rename = "iops-total-max-length")]
    pub iops_total_max_length: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SimpleCmd {
    pub smac: String,
}

fn default_cd0() -> String { "cd0".into() }

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MountIsoCmd {
    pub smac: String,
    pub isoname: String,
    #[serde(default = "default_cd0")]
    pub drive: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct UnmountIsoCmd {
    pub smac: String,
    #[serde(default = "default_cd0")]
    pub drive: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LiveMigrateCmd {
    pub smac: String,
    pub to_node_ip: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct VncCmd {
    pub smac: String,
    pub novncport: String,
}

#[derive(Debug, Serialize)]
pub struct ApiResponse {
    pub success: bool,
    pub message: String,
    pub output: Option<String>,
}
