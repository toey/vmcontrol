use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct VmStartConfig {
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub features: Features,
    pub network_adapters: Vec<NetworkAdapter>,
    pub disks: Vec<DiskInfo>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CpuInfo {
    pub sockets: String,
    pub cores: String,
    pub threads: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MemoryInfo {
    pub size: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Features {
    pub is_windows: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct NetworkAdapter {
    pub netid: String,
    pub mac: String,
    pub vlan: String,
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

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MountIsoCmd {
    pub smac: String,
    pub isoname: String,
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
