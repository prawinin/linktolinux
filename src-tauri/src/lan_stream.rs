use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use base64::Engine;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{Emitter, Manager};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex, RwLock};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::timeout;

use crate::adb_scrcpy::resolve_binary;
use crate::models::{BatteryState, DeviceView};
use crate::virtual_media::{
    default_loopback, start_mic_feed, start_mjpeg_preview_oriented, start_v4l2_sink_oriented,
    AudioRoute, MicFeed, MjpegPreview, V4l2Sink, VideoOrientation,
};

pub const LAN_PORT: u16 = 1718;
const MAX_MESSAGE: usize = 32 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(30);
type Reply = Result<Value, String>;
static LAN_DEVICES: LazyLock<RwLock<HashMap<String, DeviceView>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn configured_port() -> u16 {
    std::env::var("LINKTOLINUX_LAN_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|p| *p > 0)
        .unwrap_or(LAN_PORT)
}

pub async fn get_lan_devices() -> Vec<DeviceView> {
    LAN_DEVICES.read().await.values().cloned().collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanStatusView {
    pub listening: bool,
    pub port: u16,
    pub peers: usize,
    pub preview_url: Option<String>,
    pub v4l2_device: Option<String>,
    pub audio_source: Option<String>,
    pub download_url: String,
    pub lan_ip: String,
    pub pairing_code: String,
    pub capture_mode: Option<String>,
    pub capture_device_id: Option<String>,
    pub last_error: Option<String>,
}

struct Peer {
    generation: String,
    tx: mpsc::Sender<Value>,
}
struct Pending {
    device_id: String,
    tx: oneshot::Sender<Reply>,
}
#[derive(Default)]
struct Media {
    owner: Option<String>,
    mode: Option<String>,
    preview: Option<MjpegPreview>,
    orientation: Option<VideoOrientation>,
    sink: Option<V4l2Sink>,
    mic: Option<MicFeed>,
    route: Option<AudioRoute>,
    error: Option<String>,
}
impl Media {
    async fn stop(&mut self) {
        self.preview.take();
        self.orientation = None;
        self.sink.take();
        self.mic.take();
        if let Some(route) = self.route.take() {
            route.teardown().await;
        }
        self.mode = None;
        self.owner = None;
    }
}
struct Inner {
    peers: Mutex<HashMap<String, Peer>>,
    pending: Mutex<HashMap<String, Pending>>,
    media: Mutex<Media>,
    video: broadcast::Sender<Arc<Vec<u8>>>,
    audio: broadcast::Sender<Arc<Vec<u8>>>,
    token: String,
    clipboard_sync: Arc<AtomicBool>,
    app: Option<tauri::AppHandle>,
    files: Arc<crate::file_transfer::FileTransfers>,
}
impl Inner {
    fn clipboard_text<'a>(&self, message: &'a Value) -> Option<&'a str> {
        if message["automatic"].as_bool().unwrap_or(true)
            && !self.clipboard_sync.load(Ordering::SeqCst)
        {
            return None;
        }
        message["text"]
            .as_str()
            .filter(|text| text.len() <= 1024 * 1024)
    }
    async fn send(&self, device: &str, message: Value) -> Result<(), String> {
        let tx = self
            .peers
            .lock()
            .await
            .get(device)
            .map(|p| p.tx.clone())
            .ok_or("Phone disconnected. Open the companion and reconnect.")?;
        timeout(Duration::from_secs(5), tx.send(message))
            .await
            .map_err(|_| "Phone command queue timed out".to_string())?
            .map_err(|_| "Phone disconnected".to_string())
    }
    async fn emit_devices(&self) {
        if let Some(app) = &self.app {
            let _ = app.emit("companion:devices", get_lan_devices().await);
        }
    }
    async fn disconnect(&self, device: &str, generation: &str) {
        let mut peers = self.peers.lock().await;
        if peers.get(device).is_none_or(|p| p.generation != generation) {
            return;
        }
        self.files.disconnected(device, generation);
        peers.remove(device);
        drop(peers);
        self.pending
            .lock()
            .await
            .retain(|_, p| p.device_id != device);
        if let Some(device) = LAN_DEVICES.write().await.get_mut(device) {
            device.connected = false;
        }
        let mut media = self.media.lock().await;
        if media.owner.as_deref() == Some(device) {
            media.stop().await;
        }
        drop(media);
        self.emit_devices().await;
    }
}

pub struct LanServer {
    port: AtomicU16,
    inner: Arc<Inner>,
    handle: Mutex<Option<JoinHandle<()>>>,
}
impl LanServer {
    #[allow(dead_code)]
    pub fn new(port: u16) -> Self {
        Self::with_handle(
            port,
            None,
            uuid::Uuid::new_v4().simple().to_string()[..12].to_uppercase(),
        )
    }
    pub fn with_handle(port: u16, app: Option<tauri::AppHandle>, token: String) -> Self {
        let (video, _) = broadcast::channel(32);
        let (audio, _) = broadcast::channel(64);
        let clipboard_sync = app
            .as_ref()
            .map(|handle| handle.state::<crate::AppState>().clipboard_sync.clone())
            .unwrap_or_else(|| Arc::new(AtomicBool::new(true)));
        let files = app
            .as_ref()
            .map(|handle| handle.state::<crate::AppState>().files.clone())
            .unwrap_or_else(|| {
                crate::file_transfer::FileTransfers::new(
                    dirs::download_dir()
                        .unwrap_or_else(std::env::temp_dir)
                        .join("LinkToLinux"),
                )
            });
        Self {
            port: AtomicU16::new(port),
            handle: Mutex::new(None),
            inner: Arc::new(Inner {
                peers: Mutex::new(HashMap::new()),
                pending: Mutex::new(HashMap::new()),
                media: Mutex::new(Media::default()),
                video,
                audio,
                clipboard_sync,
                app,
                token,
                files,
            }),
        }
    }
    pub fn port(&self) -> u16 {
        self.port.load(Ordering::Relaxed)
    }
    pub fn token(&self) -> &str {
        &self.inner.token
    }
    pub async fn start(&self) -> Result<LanStatusView, String> {
        let mut handle = self.handle.lock().await;
        if handle.is_some() {
            drop(handle);
            return Ok(self.status().await);
        }
        let host = std::env::var("LINKTOLINUX_LAN_HOST").unwrap_or_else(|_| "0.0.0.0".into());
        let listener = TcpListener::bind((host.as_str(), self.port()))
            .await
            .map_err(|e| format!("Cannot listen for phone connections: {e}"))?;
        self.port.store(
            listener.local_addr().map_err(|e| e.to_string())?.port(),
            Ordering::Relaxed,
        );
        let inner = self.inner.clone();
        *handle = Some(tokio::spawn(async move {
            let mut tasks = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => match accepted {
                        Ok((stream, address)) => {
                            if tasks.len() >= 16 { drop(stream); continue; }
                            let inner = inner.clone();
                            tasks.spawn(async move {
                                if let Err(error) = handle_peer(stream, address, inner).await {
                                    log::debug!("Companion session ended: {error}");
                                }
                            });
                        }
                        Err(error) => { log::warn!("Companion listener failed: {error}"); break; }
                    },
                    _ = tasks.join_next(), if !tasks.is_empty() => {}
                }
            }
        }));
        drop(handle);
        Ok(self.status().await)
    }
    pub async fn stop(&self) -> Result<(), String> {
        if let Some(handle) = self.handle.lock().await.take() {
            handle.abort();
            let _ = handle.await;
        }
        self.inner.files.stop();
        self.inner.peers.lock().await.clear();
        self.inner.pending.lock().await.clear();
        self.inner.media.lock().await.stop().await;
        for device in LAN_DEVICES.write().await.values_mut() {
            device.connected = false;
        }
        self.inner.emit_devices().await;
        Ok(())
    }
    pub async fn status(&self) -> LanStatusView {
        verify_media(&self.inner).await;
        let listening = self
            .handle
            .lock()
            .await
            .as_ref()
            .is_some_and(|h| !h.is_finished());
        let peers = self.inner.peers.lock().await.len();
        let media = self.inner.media.lock().await;
        let lan_ip = detect_lan_ip();
        LanStatusView {
            listening,
            port: self.port(),
            peers,
            preview_url: media.preview.as_ref().map(|p| p.url()),
            v4l2_device: media
                .sink
                .as_ref()
                .and_then(|_| default_loopback().map(|d| d.path)),
            audio_source: media.route.as_ref().map(|r| r.source_name().to_string()),
            download_url: format!("http://{lan_ip}:{}/download/companion.apk", self.port()),
            lan_ip,
            pairing_code: self.token().to_string(),
            capture_mode: media.mode.clone(),
            capture_device_id: media.owner.clone(),
            last_error: media.error.clone(),
        }
    }
    pub async fn request(&self, device: &str, command: &str, args: Value) -> Reply {
        if !matches!(
            command,
            "gallery_list"
                | "gallery_thumbnail"
                | "gallery_photo"
                | "clipboard_get"
                | "clipboard_set"
                | "capture_start"
                | "capture_stop"
        ) {
            return Err("Unsupported companion command".into());
        }
        if !args.is_object()
            || serde_json::to_vec(&args).map_err(|e| e.to_string())?.len() > 1024 * 1024 + 4096
        {
            return Err("Invalid companion command arguments".into());
        }
        if command == "capture_stop" {
            let media = self.inner.media.lock().await;
            if media.owner.as_deref().is_some_and(|owner| owner != device) {
                return Err(
                    "Select the phone that owns the active capture before stopping it".into(),
                );
            }
        }
        if command == "capture_start" {
            let mode = args["mode"].as_str().ok_or("Capture mode is required")?;
            if !matches!(mode, "webcam" | "mic" | "studio") {
                return Err("Use ADB for screen mirroring".into());
            }
            let mut media = self.inner.media.lock().await;
            if media.owner.as_deref().is_some_and(|owner| owner != device) {
                return Err("Another phone is using the camera or microphone".into());
            }
            if media.mode.is_some() {
                return Err("Stop the current capture before starting another".into());
            }
            media.owner = Some(device.into());
            media.mode = Some(mode.into());
            media.error = None;
        }
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.inner.pending.lock().await;
            if pending.len() >= 32 {
                drop(pending);
                if command == "capture_start" {
                    self.inner.media.lock().await.stop().await;
                }
                return Err("Too many pending phone requests".into());
            }
            pending.insert(
                id.clone(),
                Pending {
                    device_id: device.to_string(),
                    tx,
                },
            );
        }
        let result = match self
            .inner
            .send(
                device,
                json!({"type":"request", "id":id, "command":command, "args":args}),
            )
            .await
        {
            Ok(()) => match timeout(IO_TIMEOUT, rx).await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err("Phone disconnected before replying".into()),
                Err(_) => {
                    Err("Phone did not respond. Open the companion and check permissions.".into())
                }
            },
            Err(error) => Err(error),
        };
        self.inner.pending.lock().await.remove(&id);
        if (command == "capture_stop" && result.is_ok())
            || (command == "capture_start" && result.is_err())
        {
            if result.is_err() && command == "capture_start" {
                let _ = self.inner.send(device, json!({"type":"request","command":"capture_stop","id":uuid::Uuid::new_v4().to_string(),"args":{}})).await;
            }
            let mut media = self.inner.media.lock().await;
            if media.owner.as_deref() == Some(device) {
                media.stop().await;
            }
        }
        result
    }
}
impl Drop for LanServer {
    fn drop(&mut self) {
        if let Ok(mut handle) = self.handle.try_lock() {
            if let Some(handle) = handle.take() {
                handle.abort();
            }
        }
    }
}

async fn read_json<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Value, String> {
    read_json_limit(reader, MAX_MESSAGE).await
}
async fn read_json_limit<R: AsyncRead + Unpin>(
    reader: &mut R,
    maximum: usize,
) -> Result<Value, String> {
    let len = timeout(IO_TIMEOUT, reader.read_u32())
        .await
        .map_err(|_| "Phone read timed out")?
        .map_err(|e| e.to_string())? as usize;
    if len == 0 || len > maximum {
        return Err("Invalid companion frame length".into());
    }
    let mut payload = vec![0; len];
    timeout(IO_TIMEOUT, reader.read_exact(&mut payload))
        .await
        .map_err(|_| "Incomplete companion frame")?
        .map_err(|e| e.to_string())?;
    let value: Value = serde_json::from_slice(&payload).map_err(|e| e.to_string())?;
    if !value.is_object() || value["type"].as_str().is_none() {
        return Err("Invalid companion message".into());
    }
    Ok(value)
}
async fn write_json<W: AsyncWrite + Unpin>(writer: &mut W, value: &Value) -> Result<(), String> {
    let data = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if data.len() > MAX_MESSAGE {
        return Err("Companion message exceeds size limit".into());
    }
    timeout(IO_TIMEOUT, async {
        writer.write_u32(data.len() as u32).await?;
        writer.write_all(&data).await
    })
    .await
    .map_err(|_| "Phone write timed out")?
    .map_err(|e: std::io::Error| e.to_string())
}
async fn handle_peer(
    mut stream: TcpStream,
    address: SocketAddr,
    inner: Arc<Inner>,
) -> Result<(), String> {
    stream.set_nodelay(true).map_err(|e| e.to_string())?;
    let mut prefix = [0; 4];
    let n = timeout(Duration::from_secs(5), stream.peek(&mut prefix))
        .await
        .map_err(|_| "Handshake timed out")?
        .map_err(|e| e.to_string())?;
    if n > 0 && (prefix[0] == b'G' || prefix[0] == b'H') {
        return serve_http(stream).await;
    }
    let hello = timeout(Duration::from_secs(5), read_json_limit(&mut stream, 4096))
        .await
        .map_err(|_| "Handshake timed out")??;
    if hello["type"] == "file_stream" {
        return inner.files.stream(stream, hello).await;
    }
    let device = hello["deviceId"]
        .as_str()
        .filter(|v| !v.is_empty() && v.len() <= 128)
        .ok_or("Invalid device identity")?
        .to_string();
    if hello["type"] != "hello"
        || hello["token"].as_str() != Some(&inner.token)
        || hello["protocolVersion"] != 2
    {
        write_json(
            &mut stream,
            &json!({"type":"error", "code":"pairing_rejected"}),
        )
        .await?;
        return Err("Pairing rejected. Enter the code shown on Linux.".into());
    }
    let name = hello["deviceName"]
        .as_str()
        .unwrap_or("Android phone")
        .chars()
        .take(80)
        .collect::<String>();
    let generation = uuid::Uuid::new_v4().to_string();
    let (tx, mut rx) = mpsc::channel(32);
    let files_supported = hello["features"]
        .as_array()
        .is_some_and(|features| features.iter().any(|feature| feature == "files-v1"));
    inner
        .files
        .connected(&device, &generation, files_supported, tx.clone());
    inner.peers.lock().await.insert(
        device.clone(),
        Peer {
            generation: generation.clone(),
            tx,
        },
    );
    LAN_DEVICES.write().await.insert(
        device.clone(),
        DeviceView {
            id: device.clone(),
            name: name.clone(),
            device_type: "phone".into(),
            protocol_version: 2,
            incoming_capabilities: {
                let mut capabilities = vec![
                    "camera".into(),
                    "microphone".into(),
                    "photos".into(),
                    "clipboard".into(),
                ];
                if files_supported {
                    capabilities.push("files-v1".into());
                }
                capabilities
            },
            outgoing_capabilities: vec![],
            address: Some(address.ip().to_string()),
            tcp_port: Some(address.port()),
            paired: true,
            connected: true,
            fingerprint: None,
            fingerprint_display: None,
            battery: BatteryState {
                charge: hello["battery"].as_i64().unwrap_or(-1).clamp(-1, 100) as i32,
                charging: hello["charging"].as_bool().unwrap_or(false),
                threshold_event: 0,
                battery_quantity: 1,
                updated_at: crate::config::unix_millis(),
            },
            model: Some(name),
            last_seen: crate::config::unix_millis(),
        },
    );
    inner.emit_devices().await;
    let (mut reader, mut writer) = stream.into_split();
    let result = async {
        write_json(&mut writer, &json!({"type":"hello_ack", "protocolVersion":2, "features":["files-v1"]})).await?;
        let read_loop = async {
            loop {
                let message = read_json(&mut reader).await?;
                match message["type"].as_str().unwrap_or("") {
                    "file_offer" | "file_cancel" | "file_reject" | "file_result" => {
                        if !inner.peers.lock().await.get(&device).is_some_and(|peer| peer.generation == generation) { return Err("Phone session replaced".into()); }
                        if let Err(error) = inner.files.control(&device, &generation, &message).await {
                            if let Some(id) = message["id"].as_str() {
                                inner.send(&device, json!({"type":"file_reject","id":id,"error":error})).await?;
                            }
                        }
                    }
                    "ping" => {
                        if let Some(dev) = LAN_DEVICES.write().await.get_mut(&device) {
                            dev.battery.charge = message["battery"].as_i64().unwrap_or(-1).clamp(-1,100) as i32;
                            dev.battery.charging = message["charging"].as_bool().unwrap_or(false);
                            dev.last_seen = crate::config::unix_millis();
                        }
                        inner.send(&device, json!({"type":"pong"})).await?;
                    }
                    "response" => {
                        let id = message["id"].as_str().unwrap_or("");
                        let mut pending = inner.pending.lock().await;
                        if pending.get(id).is_some_and(|p| p.device_id == device) {
                            if let Some(pending) = pending.remove(id) {
                                let result = match message["error"].as_str() {
                                    Some(error) => Err(error.chars().take(500).collect()),
                                    None => Ok(message["result"].clone()),
                                };
                                let _ = pending.tx.send(result);
                            }
                        }
                    }
                    "video" | "audio" => {
                        if let Err(error) = process_media(&inner, &device, &message).await {
                            let mut media = inner.media.lock().await;
                            if media.owner.as_deref() == Some(&device) {
                                media.stop().await;
                                media.error = Some(error);
                                drop(media);
                                let _ = inner.send(&device, json!({"type":"request","id":uuid::Uuid::new_v4().to_string(),"command":"capture_stop","args":{}})).await;
                            }
                        }
                    }
                    "clipboard" => {
                        if let Some(text) = inner.clipboard_text(&message) {
                            if let Err(error) = crate::clipboard::write_system_clipboard(text).await {
                                log::warn!("Cannot update Linux clipboard: {error}");
                            }
                            if let Some(app) = &inner.app { let _ = app.emit("companion:clipboard", json!({"deviceId":device,"text":text})); }
                        }
                    }
                    "capture_error" => {
                        let mut media = inner.media.lock().await;
                        if media.owner.as_deref() == Some(&device) {
                            media.stop().await;
                            media.error = Some(message["error"].as_str().unwrap_or("Phone capture stopped").chars().take(500).collect());
                        }
                    }
                    "capture_stopped" => {
                        let mut media = inner.media.lock().await;
                        if media.owner.as_deref() == Some(&device) { media.stop().await; }
                    }
                    _ => {}
                }
            }
            #[allow(unreachable_code)]
            Ok::<(), String>(())
        };
        let write_loop = async {
            while let Some(message) = rx.recv().await { write_json(&mut writer, &message).await?; }
            Ok::<(), String>(())
        };
        tokio::select! { result = read_loop => result, result = write_loop => result }
    }.await;
    inner.disconnect(&device, &generation).await;
    result
}
async fn process_media(inner: &Inner, device: &str, message: &Value) -> Result<(), String> {
    let mut media = inner.media.lock().await;
    if media.owner.as_deref() != Some(device) || media.mode.is_none() {
        return Ok(());
    }
    let data = base64::engine::general_purpose::STANDARD
        .decode(message["data"].as_str().ok_or("Missing media data")?)
        .map_err(|_| "Invalid media data")?;
    if data.is_empty() || data.len() > 2 * 1024 * 1024 {
        return Err("Invalid media packet size".into());
    }
    let ffmpeg = resolve_binary("ffmpeg", "LINKTOLINUX_FFMPEG")
        .ok_or("Install ffmpeg to use phone camera and microphone")?;
    if message["type"] == "video" {
        if message["codec"] != "h264" {
            return Err("Expected H.264 camera stream".into());
        }
        let width = message["width"]
            .as_u64()
            .filter(|v| (16..=1920).contains(v))
            .ok_or("Invalid camera width")? as u32;
        let height = message["height"]
            .as_u64()
            .filter(|v| (16..=1920).contains(v))
            .ok_or("Invalid camera height")? as u32;
        let rotation = match message.get("rotation") {
            None => 0,
            Some(value) => value
                .as_u64()
                .filter(|v| matches!(v, 0 | 90 | 180 | 270))
                .ok_or("Invalid camera rotation")? as u32,
        };
        let orientation = VideoOrientation {
            width,
            height,
            rotation,
        };
        if media.preview.is_none() || media.orientation != Some(orientation) {
            media.preview.take();
            media.sink.take();
            media.preview = Some(
                start_mjpeg_preview_oriented(
                    &ffmpeg,
                    "h264",
                    inner.video.subscribe(),
                    orientation,
                    24,
                )
                .map_err(|e| e.to_string())?,
            );
            media.orientation = Some(orientation);
            if let Some(device) = default_loopback() {
                match start_v4l2_sink_oriented(
                    &ffmpeg,
                    &device.path,
                    "h264",
                    inner.video.subscribe(),
                    orientation,
                    24,
                ) {
                    Ok(sink) => media.sink = Some(sink),
                    Err(error) => {
                        media.error = Some(format!(
                            "Camera preview is available; virtual webcam failed: {error}"
                        ))
                    }
                }
            }
        }
        let _ = inner.video.send(Arc::new(data));
    } else {
        if message["codec"] != "pcm_s16le"
            || message["sampleRate"] != 48000
            || message["channels"] != 1
        {
            return Err("Expected 48 kHz mono PCM audio".into());
        }
        if media.mic.is_none() {
            let route = AudioRoute::ensure().await.map_err(|e| e.to_string())?;
            match start_mic_feed(&ffmpeg, &route, "raw", inner.audio.subscribe(), 1) {
                Ok(feed) => {
                    media.mic = Some(feed);
                    media.route = Some(route);
                }
                Err(error) => {
                    route.teardown().await;
                    return Err(error.to_string());
                }
            }
        }
        let _ = inner.audio.send(Arc::new(data));
    }
    Ok(())
}

async fn verify_media(inner: &Inner) {
    let mut media = inner.media.lock().await;
    if let Some(sink) = media.sink.as_mut() {
        if let Err(error) = sink.check() {
            media.sink.take();
            media.error = Some(error.to_string());
        }
    }
    let error = media
        .preview
        .as_mut()
        .and_then(|preview| preview.check().err())
        .or_else(|| media.mic.as_mut().and_then(|mic| mic.check().err()));
    if let Some(error) = error {
        let owner = media.owner.clone();
        media.stop().await;
        media.error = Some(error.to_string());
        drop(media);
        if let Some(owner) = owner {
            let _ = inner.send(&owner, json!({"type":"request","id":uuid::Uuid::new_v4().to_string(),"command":"capture_stop","args":{}})).await;
        }
    }
}

pub fn detect_lan_ip() -> String {
    if let Ok(ip) = std::env::var("LINKTOLINUX_LAN_IP") {
        if let Ok(ip) = ip.parse::<IpAddr>() {
            return ip.to_string();
        }
    }
    let output = std::process::Command::new("ip")
        .args(["-j", "-4", "route", "get", "1.1.1.1"])
        .output();
    if let Ok(output) = output {
        if let Ok(value) = serde_json::from_slice::<Value>(&output.stdout) {
            if let Some(ip) = value[0]["prefsrc"]
                .as_str()
                .and_then(|s| s.parse::<IpAddr>().ok())
            {
                return ip.to_string();
            }
        }
    }
    "127.0.0.1".into()
}
pub fn find_companion_apk() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("LINKTOLINUX_APK") {
        candidates.push(PathBuf::from(path));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("LinkToLinux-Companion.apk"));
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}
async fn serve_http(mut stream: TcpStream) -> Result<(), String> {
    let mut request = [0; 4096];
    let len = timeout(Duration::from_secs(5), stream.read(&mut request))
        .await
        .map_err(|_| "HTTP request timed out")?
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&request[..len]);
    let mut parts = text.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    let (status, mime, body) = if !matches!(method, "GET" | "HEAD") {
        ("405 Method Not Allowed", "text/plain", Vec::new())
    } else if path == "/download/companion.apk" {
        match find_companion_apk() {
            Some(path) => (
                "200 OK",
                "application/vnd.android.package-archive",
                tokio::fs::read(path).await.map_err(|e| e.to_string())?,
            ),
            None => (
                "404 Not Found",
                "text/plain",
                b"Android companion is missing. Reinstall LinkToLinux.".to_vec(),
            ),
        }
    } else if path == "/" {
        ("200 OK", "text/html; charset=utf-8", b"<!doctype html><html lang=\"en\"><meta name=\"viewport\" content=\"width=device-width\"><title>LinkToLinux</title><body style=\"font:18px system-ui;max-width:36em;margin:4em auto;padding:1em\"><h1>LinkToLinux for Android</h1><p>Install the companion, then enter the desktop address, port and pairing code shown in the Linux app.</p><p>Use a trusted local Wi-Fi network. This connection is not encrypted.</p><a href=\"/download/companion.apk\">Download companion APK</a></body></html>".to_vec())
    } else {
        ("404 Not Found", "text/plain", b"Not found".to_vec())
    };
    let header = format!("HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n\r\n", body.len());
    timeout(IO_TIMEOUT, async {
        stream.write_all(header.as_bytes()).await?;
        if method != "HEAD" {
            stream.write_all(&body).await?;
        }
        stream.shutdown().await
    })
    .await
    .map_err(|_| "HTTP download timed out")?
    .map_err(|e: std::io::Error| e.to_string())
}

#[tauri::command]
pub async fn lan_start(state: tauri::State<'_, crate::AppState>) -> Result<LanStatusView, String> {
    let mut lan = state.lan.lock().await;
    if let Some(server) = lan.as_ref() {
        return Ok(server.status().await);
    }
    let server = Arc::new(LanServer::with_handle(
        configured_port(),
        Some(state.app.clone()),
        crate::config::pairing_code(&state.data_dir, false)?,
    ));
    let status = server.start().await?;
    *lan = Some(server);
    Ok(status)
}
#[tauri::command]
pub async fn lan_reset_pairing(
    state: tauri::State<'_, crate::AppState>,
) -> Result<LanStatusView, String> {
    let mut lan = state.lan.lock().await;
    let token = crate::config::pairing_code(&state.data_dir, true)?;
    let port = lan
        .as_ref()
        .map_or_else(configured_port, |server| server.port());
    if let Some(server) = lan.take() {
        server.stop().await?;
    }
    let server = Arc::new(LanServer::with_handle(port, Some(state.app.clone()), token));
    let status = server.start().await?;
    *lan = Some(server);
    Ok(status)
}
#[tauri::command]
pub async fn lan_stop(state: tauri::State<'_, crate::AppState>) -> Result<(), String> {
    if let Some(server) = state.lan.lock().await.take() {
        server.stop().await?;
    }
    Ok(())
}
#[tauri::command]
pub async fn lan_status(state: tauri::State<'_, crate::AppState>) -> Result<LanStatusView, String> {
    match state.lan.lock().await.as_ref() {
        Some(server) => Ok(server.status().await),
        None => Ok(LanStatusView {
            listening: false,
            port: configured_port(),
            peers: 0,
            preview_url: None,
            v4l2_device: None,
            audio_source: None,
            download_url: format!(
                "http://{}:{}/download/companion.apk",
                detect_lan_ip(),
                configured_port()
            ),
            lan_ip: detect_lan_ip(),
            pairing_code: String::new(),
            capture_mode: None,
            capture_device_id: None,
            last_error: None,
        }),
    }
}
#[tauri::command]
pub async fn companion_request(
    state: tauri::State<'_, crate::AppState>,
    device_id: String,
    command: String,
    args: Value,
) -> Reply {
    let server = state
        .lan
        .lock()
        .await
        .clone()
        .ok_or("Start the phone connection server first")?;
    server.request(&device_id, &command, args).await
}
#[tauri::command]
pub async fn companion_save_photo(
    state: tauri::State<'_, crate::AppState>,
    device_id: String,
    photo_id: String,
) -> Result<String, String> {
    let result = companion_request(
        state.clone(),
        device_id,
        "gallery_photo".into(),
        json!({"photoId":photo_id}),
    )
    .await?;
    let data = base64::engine::general_purpose::STANDARD
        .decode(result["data"].as_str().ok_or("Missing photo data")?)
        .map_err(|_| "Invalid photo encoding")?;
    let name = result["name"].as_str().unwrap_or("photo.jpg");
    let safe: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .take(160)
        .collect();
    let name = if safe.is_empty() || safe == "." || safe == ".." {
        "photo.jpg"
    } else {
        &safe
    };
    let directory = state.downloads();
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|e| e.to_string())?;
    let target = directory.join(format!("{}-{name}", uuid::Uuid::new_v4()));
    tokio::fs::write(&target, data)
        .await
        .map_err(|e| e.to_string())?;
    Ok(target.display().to_string())
}
#[tauri::command]
pub fn companion_clipboard_sync(state: tauri::State<'_, crate::AppState>) -> Result<bool, String> {
    Ok(state.clipboard_sync.load(Ordering::SeqCst))
}

#[tauri::command]
pub fn companion_set_clipboard_sync(
    state: tauri::State<'_, crate::AppState>,
    enabled: bool,
) -> Result<bool, String> {
    let _write = state
        .clipboard_sync_write
        .lock()
        .map_err(|_| "Cannot update clipboard synchronization preference")?;
    crate::config::save_clipboard_sync(&state.data_dir, enabled)?;
    state.clipboard_sync.store(enabled, Ordering::SeqCst);
    Ok(enabled)
}

#[tauri::command]
pub async fn companion_read_clipboard() -> Result<String, String> {
    crate::clipboard::read_system_clipboard()
        .await
        .ok_or_else(|| {
            "Cannot read the Linux clipboard. Install wl-clipboard on Wayland or xclip on X11."
                .into()
        })
}
#[tauri::command]
pub async fn companion_write_clipboard(text: String) -> Result<(), String> {
    if text.len() > 1024 * 1024 {
        return Err("Clipboard text exceeds 1 MiB".into());
    }
    crate::clipboard::write_system_clipboard(&text)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn companion_enable_webcam() -> Result<String, String> {
    if default_loopback().is_some() {
        return Ok("Virtual webcam is available. Start or restart camera capture.".into());
    }
    let mut command = tokio::process::Command::new("pkexec");
    command
        .args([
            "modprobe",
            "v4l2loopback",
            "exclusive_caps=1",
            "card_label=LinkToLinux Camera",
            "devices=1",
        ])
        .kill_on_drop(true);
    let output = timeout(Duration::from_secs(90), command.output()).await
        .map_err(|_| "Administrator authorization timed out. Retry Enable virtual webcam and complete the desktop password dialog.".to_string())?
        .map_err(|e| format!("Cannot enable virtual webcam: {e}"))?;
    if !output.status.success() {
        return Err(format!("Virtual webcam setup failed: {}. Install v4l2loopback for the running kernel and authorize the desktop password dialog.", String::from_utf8_lossy(&output.stderr).trim().chars().take(400).collect::<String>()));
    }
    for _ in 0..10 {
        if default_loopback().is_some() {
            return Ok("Virtual webcam enabled. Start or restart camera capture.".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(
        "Driver loaded but no virtual camera appeared. Check v4l2loopback device configuration."
            .into(),
    )
}

#[cfg(test)]
#[path = "lan_tests.rs"]
mod companion_tests;

#[cfg(test)]
mod file_lane_tests {
    use super::*;
    #[tokio::test]
    async fn file_lane_does_not_block_control_ping() {
        let directory = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let mut server = LanServer::new(0);
        Arc::get_mut(&mut server.inner).unwrap().files =
            crate::file_transfer::FileTransfers::new(directory.clone());
        let status = server.start().await.unwrap();
        let mut control = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, status.port))
            .await
            .unwrap();
        write_json(&mut control,&json!({"type":"hello","deviceId":"file-lane-test","deviceName":"Test","token":server.token(),"protocolVersion":2,"features":["files-v1"]})).await.unwrap();
        assert_eq!(read_json(&mut control).await.unwrap()["type"], "hello_ack");
        let id = uuid::Uuid::new_v4().to_string();
        write_json(&mut control,&json!({"type":"file_offer","id":id,"name":"test.bin","size":3,"mime":"application/octet-stream"})).await.unwrap();
        let accept = read_json(&mut control).await.unwrap();
        assert_eq!(accept["type"], "file_accept");
        let mut data = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, status.port))
            .await
            .unwrap();
        write_json(
            &mut data,
            &json!({"type":"file_stream","id":id,"token":accept["token"]}),
        )
        .await
        .unwrap();
        assert_eq!(read_json(&mut data).await.unwrap()["type"], "file_ready");
        data.write_u32(3).await.unwrap();
        data.write_all(b"a").await.unwrap();
        write_json(&mut control, &json!({"type":"ping","battery":80}))
            .await
            .unwrap();
        let pong = timeout(Duration::from_secs(1), read_json(&mut control))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pong["type"], "pong");
        server.stop().await.unwrap();
        drop(data);
        drop(control);
        for _ in 0..50 {
            if !directory.exists() || std::fs::read_dir(&directory).unwrap().next().is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if directory.exists() {
            assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 0);
            std::fs::remove_dir_all(directory).unwrap();
        }
    }
}
