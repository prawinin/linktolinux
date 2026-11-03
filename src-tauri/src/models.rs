use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatteryState {
    pub charge: i32,
    pub charging: bool,
    pub threshold_event: i32,
    pub battery_quantity: i32,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    pub device_type: String,
    pub protocol_version: i32,
    pub incoming_capabilities: Vec<String>,
    pub outgoing_capabilities: Vec<String>,
    pub address: Option<String>,
    pub tcp_port: Option<u16>,
    pub paired: bool,
    pub connected: bool,
    pub fingerprint: Option<String>,
    pub fingerprint_display: Option<String>,
    pub battery: BatteryState,
    pub model: Option<String>,
    pub last_seen: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbDevice {
    pub serial: String,
    pub state: String,
    pub model: Option<String>,
    pub product: Option<String>,
    pub device: Option<String>,
    pub transport_id: Option<String>,
    pub connection: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureTelemetry {
    pub active: bool,
    pub mode: String,
    pub serial: Option<String>,
    pub codec: Option<String>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub v4l2_device: Option<String>,
    pub audio_sink: Option<String>,
    pub preview: bool,
    pub started_at: i64,
    pub frames: u64,
    pub dropped: u64,
}

impl Default for CaptureTelemetry {
    fn default() -> Self {
        Self {
            active: false,
            mode: "idle".into(),
            serial: None,
            codec: None,
            width: 0,
            height: 0,
            fps: 0,
            v4l2_device: None,
            audio_sink: None,
            preview: false,
            started_at: 0,
            frames: 0,
            dropped: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoDevice {
    pub path: String,
    pub label: String,
    pub loopback: bool,
}
