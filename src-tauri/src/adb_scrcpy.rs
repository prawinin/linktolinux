use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::time::{sleep, timeout};

use crate::models::AdbDevice;

pub const SCRCPY_VERSION: &str = "5.0.1";
pub const SERVER_JAR_NAME: &str = "scrcpy-server.jar";
pub const SERVER_REMOTE_PATH: &str = "/data/local/tmp/scrcpy-server.jar";
const DEVICE_NAME_FIELD_LENGTH: usize = 64;
const PACKET_HEADER_SIZE: usize = 12;
const MAX_PACKET_SIZE: usize = 64 * 1024 * 1024;
const ADB_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_ATTEMPTS: usize = 150;
const CONNECT_DELAY: Duration = Duration::from_millis(100);

pub const CODEC_H264: u32 = 0x6832_3634;
pub const CODEC_H265: u32 = 0x6832_3635;
pub const CODEC_AV1: u32 = 0x0061_7631;
pub const CODEC_VP8: u32 = 0x0076_7038;
pub const CODEC_VP9: u32 = 0x0076_7039;
pub const CODEC_OPUS: u32 = 0x6f70_7573;
pub const CODEC_AAC: u32 = 0x0061_6163;
pub const CODEC_FLAC: u32 = 0x666c_6163;
pub const CODEC_RAW: u32 = 0x0072_6177;

const FLAG_SESSION: u8 = 0x80;
const FLAG_CONFIG: u64 = 1 << 62;
const FLAG_KEYFRAME: u64 = 1 << 61;
const PTS_MASK: u64 = (1 << 61) - 1;

pub fn codec_name(codec_id: u32) -> &'static str {
    match codec_id {
        CODEC_H264 => "h264",
        CODEC_H265 => "h265",
        CODEC_AV1 => "av1",
        CODEC_VP8 => "vp8",
        CODEC_VP9 => "vp9",
        CODEC_OPUS => "opus",
        CODEC_AAC => "aac",
        CODEC_FLAC => "flac",
        CODEC_RAW => "raw",
        _ => "unknown",
    }
}

pub fn codec_id_for(name: &str) -> Result<u32> {
    match name {
        "h264" => Ok(CODEC_H264),
        "h265" | "hevc" => Ok(CODEC_H265),
        "av1" => Ok(CODEC_AV1),
        "vp8" => Ok(CODEC_VP8),
        "vp9" => Ok(CODEC_VP9),
        "opus" => Ok(CODEC_OPUS),
        "aac" => Ok(CODEC_AAC),
        "flac" => Ok(CODEC_FLAC),
        "raw" => Ok(CODEC_RAW),
        other => bail!("unsupported codec {other}"),
    }
}

fn executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// Resolves an executable from an override, PATH, installed tools, or a configured Android SDK.
pub fn resolve_binary(command: &str, env_var: &str) -> Option<PathBuf> {
    if let Ok(value) = std::env::var(env_var) {
        let path = PathBuf::from(value);
        if executable_file(&path) {
            return Some(path);
        }
    }
    let candidate = Path::new(command);
    if candidate.is_absolute() {
        return executable_file(candidate).then(|| candidate.to_path_buf());
    }
    if let Some(search) = std::env::var_os("PATH") {
        if let Some(found) = std::env::split_paths(&search)
            .map(|directory| directory.join(command))
            .find(|path| executable_file(path))
        {
            return Some(found);
        }
    }
    let mut candidates = Vec::new();
    if let Some(binary_home) = std::env::var_os("XDG_BIN_HOME") {
        candidates.push(PathBuf::from(binary_home));
    } else if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".local/bin"));
    }
    if let Some(data_home) = dirs::data_dir() {
        candidates.push(data_home.join("linktolinux/bin"));
    }
    if command == "adb" {
        for variable in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
            if let Some(sdk) = std::env::var_os(variable) {
                candidates.push(PathBuf::from(sdk).join("platform-tools"));
            }
        }
    }
    candidates
        .into_iter()
        .map(|directory| directory.join(command))
        .find(|path| executable_file(path))
}

async fn command_output(mut command: Command, label: &str) -> Result<std::process::Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = command
        .spawn()
        .with_context(|| format!("spawning {label}"))?;
    let output = timeout(ADB_TIMEOUT, child.wait_with_output())
        .await
        .with_context(|| format!("{label} timed out after {}s", ADB_TIMEOUT.as_secs()))?
        .with_context(|| format!("waiting for {label}"))?;
    Ok(output)
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_string()
}

#[derive(Debug, Clone)]
pub struct Adb {
    executable: PathBuf,
}

impl Adb {
    pub fn locate() -> Result<Self> {
        let executable = resolve_binary("adb", "LINKTOLINUX_ADB").ok_or_else(|| {
            anyhow!("adb not found; install android-tools or set LINKTOLINUX_ADB")
        })?;
        Ok(Self { executable })
    }

    pub fn path(&self) -> &Path {
        &self.executable
    }

    fn base(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command.kill_on_drop(true);
        command
    }

    pub async fn run(&self, args: &[&str]) -> Result<String> {
        let mut command = self.base();
        command.args(args);
        let output = command_output(command, "adb").await?;
        if !output.status.success() {
            let stderr = lossy(&output.stderr);
            bail!("adb {} failed: {stderr}", args.join(" "));
        }
        Ok(lossy(&output.stdout))
    }

    pub async fn devices(&self) -> Result<Vec<AdbDevice>> {
        let raw = self.run(&["devices", "-l"]).await?;
        let mut devices = Vec::new();
        for line in raw.lines().skip(1) {
            let line = line.trim();
            if line.is_empty() || line.starts_with('*') {
                continue;
            }
            let mut fields = line.split_whitespace();
            let Some(serial) = fields.next() else {
                continue;
            };
            let Some(state) = fields.next() else { continue };
            let mut device = AdbDevice {
                serial: serial.to_string(),
                state: state.to_string(),
                model: None,
                product: None,
                device: None,
                transport_id: None,
                connection: if serial.contains(':') {
                    "wireless".to_string()
                } else {
                    "usb".to_string()
                },
            };
            for field in fields {
                let Some((key, value)) = field.split_once(':') else {
                    continue;
                };
                match key {
                    "model" => device.model = Some(value.replace('_', " ")),
                    "product" => device.product = Some(value.to_string()),
                    "device" => device.device = Some(value.to_string()),
                    "transport_id" => device.transport_id = Some(value.to_string()),
                    _ => {}
                }
            }
            devices.push(device);
        }
        devices.sort_by(|left, right| left.serial.cmp(&right.serial));
        Ok(devices)
    }

    pub async fn pair(&self, address: &str, code: &str) -> Result<String> {
        let code = code.trim();
        if code.is_empty() || !code.chars().all(|character| character.is_ascii_digit()) {
            bail!("pairing code must be numeric");
        }
        let output = self.run(&["pair", address, code]).await?;
        Ok(output)
    }

    pub async fn connect(&self, address: &str) -> Result<String> {
        let output = self.run(&["connect", address]).await?;
        if output.contains("failed") || output.contains("cannot") {
            bail!("{output}");
        }
        Ok(output)
    }

    pub async fn shell(&self, serial: &str, command: &str) -> Result<String> {
        self.run(&["-s", serial, "shell", command]).await
    }

    pub async fn exec_out(&self, serial: &str, command: &str) -> Result<Vec<u8>> {
        let mut adb = self.base();
        adb.args(["-s", serial, "exec-out", command]);
        adb.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = adb.spawn().context("spawning adb exec-out")?;
        let output = timeout(Duration::from_secs(120), child.wait_with_output())
            .await
            .context("adb exec-out timed out")?
            .context("waiting for adb exec-out")?;
        if !output.status.success() {
            bail!("adb exec-out failed: {}", lossy(&output.stderr));
        }
        Ok(output.stdout)
    }

    pub async fn push(&self, serial: &str, local: &Path, remote: &str) -> Result<()> {
        let local = local
            .to_str()
            .ok_or_else(|| anyhow!("server jar path is not valid UTF-8"))?;
        self.run(&["-s", serial, "push", local, remote]).await?;
        Ok(())
    }

    pub async fn forward(&self, serial: &str, port: u16, socket: &str) -> Result<()> {
        self.run(&[
            "-s",
            serial,
            "forward",
            &format!("tcp:{port}"),
            &format!("localabstract:{socket}"),
        ])
        .await?;
        Ok(())
    }

    pub async fn forward_remove(&self, serial: &str, port: u16) -> Result<()> {
        self.run(&["-s", serial, "forward", "--remove", &format!("tcp:{port}")])
            .await?;
        Ok(())
    }
}

/// Locates the scrcpy server jar bundled by the application or the distribution.
pub fn resolve_server_jar(explicit: Option<&Path>, data_dir: &Path) -> Option<PathBuf> {
    if let Some(path) = explicit {
        if path.is_file() {
            return Some(path.to_path_buf());
        }
    }
    if let Ok(value) = std::env::var("LINKTOLINUX_SCRCPY_SERVER") {
        let path = PathBuf::from(value);
        if path.is_file() {
            return Some(path);
        }
    }
    let mut candidates = vec![
        data_dir.join(SERVER_JAR_NAME),
        PathBuf::from("/usr/share/scrcpy").join(SERVER_JAR_NAME),
        PathBuf::from("/usr/local/share/scrcpy").join(SERVER_JAR_NAME),
    ];
    if let Ok(executable) = std::env::current_exe() {
        if let Some(directory) = executable.parent() {
            candidates.push(directory.join(SERVER_JAR_NAME));
        }
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn reserve_port() -> Result<u16> {
    let listener =
        std::net::TcpListener::bind(("127.0.0.1", 0)).context("reserving a local tunnel port")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

#[derive(Debug, Clone)]
pub struct ScrcpyOptions {
    pub video: bool,
    pub audio: bool,
    pub control: bool,
    pub video_source: String,
    pub audio_source: String,
    pub video_codec: String,
    pub audio_codec: String,
    pub max_size: u32,
    pub max_fps: u32,
    pub video_bit_rate: u32,
    pub audio_bit_rate: u32,
    pub display_id: Option<u32>,
    pub camera_id: Option<String>,
    pub camera_size: Option<String>,
    pub camera_facing: Option<String>,
    pub stay_awake: bool,
}

impl Default for ScrcpyOptions {
    fn default() -> Self {
        Self {
            video: true,
            audio: true,
            control: true,
            video_source: "display".to_string(),
            audio_source: "output".to_string(),
            video_codec: "h264".to_string(),
            audio_codec: "opus".to_string(),
            max_size: 0,
            max_fps: 0,
            video_bit_rate: 8_000_000,
            audio_bit_rate: 128_000,
            display_id: None,
            camera_id: None,
            camera_size: None,
            camera_facing: None,
            stay_awake: true,
        }
    }
}

impl ScrcpyOptions {
    fn arguments(&self, scid: u32) -> Vec<String> {
        let mut args = vec![
            format!("scid={scid:08x}"),
            "log_level=info".to_string(),
            format!("video={}", self.video),
            format!("audio={}", self.audio),
            format!("control={}", self.control),
            format!("video_codec={}", self.video_codec),
            format!("audio_codec={}", self.audio_codec),
            format!("video_source={}", self.video_source),
            format!("audio_source={}", self.audio_source),
            "tunnel_forward=true".to_string(),
            "send_device_meta=true".to_string(),
            "send_frame_meta=true".to_string(),
            "send_codec_meta=true".to_string(),
            "cleanup=true".to_string(),
            format!("stay_awake={}", self.stay_awake),
        ];
        if self.max_size > 0 {
            args.push(format!("max_size={}", self.max_size));
        }
        if self.max_fps > 0 {
            args.push(format!("max_fps={}", self.max_fps));
        }
        if self.video_bit_rate > 0 {
            args.push(format!("video_bit_rate={}", self.video_bit_rate));
        }
        if self.audio_bit_rate > 0 {
            args.push(format!("audio_bit_rate={}", self.audio_bit_rate));
        }
        if let Some(display_id) = self.display_id {
            args.push(format!("display_id={display_id}"));
        }
        if let Some(camera_id) = &self.camera_id {
            args.push(format!("camera_id={}", camera_id.replace(',', "|")));
        }
        if let Some(camera_size) = &self.camera_size {
            args.push(format!("camera_size={camera_size}"));
        }
        if let Some(camera_facing) = &self.camera_facing {
            args.push(format!("camera_facing={camera_facing}"));
        }
        args
    }
}

#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub pts: u64,
    pub keyframe: bool,
    pub config: bool,
    pub data: Vec<u8>,
}

pub struct VideoStream {
    stream: TcpStream,
    pub codec_id: u32,
    pub width: u32,
    pub height: u32,
}

impl VideoStream {
    async fn open(mut stream: TcpStream) -> Result<Self> {
        let codec_id = read_codec_id(&mut stream).await?;
        let mut video = Self {
            stream,
            codec_id,
            width: 0,
            height: 0,
        };
        video.await_session().await?;
        Ok(video)
    }

    async fn await_session(&mut self) -> Result<()> {
        let mut header = [0u8; PACKET_HEADER_SIZE];
        for _ in 0..64 {
            self.stream
                .read_exact(&mut header)
                .await
                .context("reading scrcpy session header")?;
            if header[0] & FLAG_SESSION != 0 {
                self.width = u32::from_be_bytes([header[4], header[5], header[6], header[7]]);
                self.height = u32::from_be_bytes([header[8], header[9], header[10], header[11]]);
                return Ok(());
            }
            let size = u32::from_be_bytes([header[8], header[9], header[10], header[11]]) as usize;
            let mut discard = vec![0u8; size.min(MAX_PACKET_SIZE)];
            self.stream.read_exact(&mut discard).await?;
        }
        bail!("scrcpy never sent a video session header");
    }

    pub async fn next_frame(&mut self) -> Result<Option<VideoFrame>> {
        let mut header = [0u8; PACKET_HEADER_SIZE];
        loop {
            match self.stream.read_exact(&mut header).await {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
                Err(error) => return Err(error).context("reading scrcpy frame header"),
            }
            if header[0] & FLAG_SESSION != 0 {
                self.width = u32::from_be_bytes([header[4], header[5], header[6], header[7]]);
                self.height = u32::from_be_bytes([header[8], header[9], header[10], header[11]]);
                continue;
            }
            let pts_flags = u64::from_be_bytes([
                header[0], header[1], header[2], header[3], header[4], header[5], header[6],
                header[7],
            ]);
            let size = u32::from_be_bytes([header[8], header[9], header[10], header[11]]) as usize;
            if size > MAX_PACKET_SIZE {
                bail!("scrcpy frame of {size} bytes exceeds the safety limit");
            }
            let mut data = vec![0u8; size];
            self.stream
                .read_exact(&mut data)
                .await
                .context("reading scrcpy frame payload")?;
            return Ok(Some(VideoFrame {
                pts: pts_flags & PTS_MASK,
                keyframe: pts_flags & FLAG_KEYFRAME != 0,
                config: pts_flags & FLAG_CONFIG != 0,
                data,
            }));
        }
    }
}

pub struct AudioStream {
    stream: TcpStream,
    pub codec_id: u32,
}

impl AudioStream {
    async fn open(mut stream: TcpStream) -> Result<Self> {
        let codec_id = read_codec_id(&mut stream).await?;
        Ok(Self { stream, codec_id })
    }

    pub async fn next_frame(&mut self) -> Result<Option<Vec<u8>>> {
        let mut header = [0u8; PACKET_HEADER_SIZE];
        match self.stream.read_exact(&mut header).await {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(error) => return Err(error).context("reading scrcpy audio header"),
        }
        if header[0] & FLAG_SESSION != 0 {
            return Ok(Some(Vec::new()));
        }
        let size = u32::from_be_bytes([header[8], header[9], header[10], header[11]]) as usize;
        if size > MAX_PACKET_SIZE {
            bail!("scrcpy audio packet of {size} bytes exceeds the safety limit");
        }
        let mut data = vec![0u8; size];
        self.stream
            .read_exact(&mut data)
            .await
            .context("reading scrcpy audio payload")?;
        Ok(Some(data))
    }
}

async fn read_codec_id(stream: &mut TcpStream) -> Result<u32> {
    let mut raw = [0u8; 4];
    stream
        .read_exact(&mut raw)
        .await
        .context("reading scrcpy codec id")?;
    let codec_id = u32::from_be_bytes(raw);
    if codec_name(codec_id) == "unknown" {
        bail!("scrcpy announced unknown codec 0x{codec_id:08x}");
    }
    Ok(codec_id)
}

/// Mirrors the scrcpy control message set so the wire format stays complete even
/// when a surface (torch, zoom, app launch) is not exposed by the current UI.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum ControlMessage {
    InjectKeycode {
        action: u8,
        keycode: u32,
        repeat: u32,
        metastate: u32,
    },
    InjectText(String),
    InjectTouch {
        action: u8,
        pointer_id: u64,
        x: i32,
        y: i32,
        screen_width: u16,
        screen_height: u16,
        pressure: f32,
        action_button: u32,
        buttons: u32,
    },
    InjectScroll {
        x: i32,
        y: i32,
        screen_width: u16,
        screen_height: u16,
        hscroll: f32,
        vscroll: f32,
        buttons: u32,
    },
    BackOrScreenOn {
        action: u8,
    },
    ExpandNotificationPanel,
    ExpandSettingsPanel,
    CollapsePanels,
    GetClipboard {
        copy_key: u8,
    },
    SetClipboard {
        sequence: u64,
        text: String,
        paste: bool,
    },
    SetDisplayPower {
        on: bool,
    },
    RotateDevice,
    StartApp(String),
    ResetVideo,
    CameraSetTorch(bool),
    CameraZoomIn,
    CameraZoomOut,
    ResizeDisplay {
        width: u16,
        height: u16,
    },
    ScanFile(String),
}

#[cfg(test)]
pub const KEYCODE_HOME: u32 = 3;
pub const ACTION_DOWN: u8 = 0;
pub const ACTION_UP: u8 = 1;
pub const BUTTON_PRIMARY: u32 = 1;

fn push_string(buffer: &mut Vec<u8>, text: &str, length_bytes: usize) {
    let max = if length_bytes == 4 {
        256 * 1024
    } else {
        u8::MAX as usize
    };
    let trimmed: String = text.chars().take(max).collect();
    let bytes = trimmed.as_bytes();
    let len = bytes.len().min(max);
    if length_bytes == 4 {
        buffer.extend_from_slice(&(len as u32).to_be_bytes());
    } else {
        buffer.push(len as u8);
    }
    buffer.extend_from_slice(&bytes[..len]);
}

impl ControlMessage {
    pub fn serialize(&self) -> Vec<u8> {
        let mut buffer = Vec::with_capacity(32);
        match self {
            ControlMessage::InjectKeycode {
                action,
                keycode,
                repeat,
                metastate,
            } => {
                buffer.push(0);
                buffer.push(*action);
                buffer.extend_from_slice(&keycode.to_be_bytes());
                buffer.extend_from_slice(&repeat.to_be_bytes());
                buffer.extend_from_slice(&metastate.to_be_bytes());
            }
            ControlMessage::InjectText(text) => {
                buffer.push(1);
                push_string(&mut buffer, text, 4);
            }
            ControlMessage::InjectTouch {
                action,
                pointer_id,
                x,
                y,
                screen_width,
                screen_height,
                pressure,
                action_button,
                buttons,
            } => {
                buffer.push(2);
                buffer.push(*action);
                buffer.extend_from_slice(&pointer_id.to_be_bytes());
                buffer.extend_from_slice(&x.to_be_bytes());
                buffer.extend_from_slice(&y.to_be_bytes());
                buffer.extend_from_slice(&screen_width.to_be_bytes());
                buffer.extend_from_slice(&screen_height.to_be_bytes());
                buffer.extend_from_slice(&float_to_u16fp(*pressure).to_be_bytes());
                buffer.extend_from_slice(&action_button.to_be_bytes());
                buffer.extend_from_slice(&buttons.to_be_bytes());
            }
            ControlMessage::InjectScroll {
                x,
                y,
                screen_width,
                screen_height,
                hscroll,
                vscroll,
                buttons,
            } => {
                buffer.push(3);
                buffer.extend_from_slice(&x.to_be_bytes());
                buffer.extend_from_slice(&y.to_be_bytes());
                buffer.extend_from_slice(&screen_width.to_be_bytes());
                buffer.extend_from_slice(&screen_height.to_be_bytes());
                buffer.extend_from_slice(&float_to_i16fp(*hscroll / 16.0).to_be_bytes());
                buffer.extend_from_slice(&float_to_i16fp(*vscroll / 16.0).to_be_bytes());
                buffer.extend_from_slice(&buttons.to_be_bytes());
            }
            ControlMessage::BackOrScreenOn { action } => {
                buffer.push(4);
                buffer.push(*action);
            }
            ControlMessage::ExpandNotificationPanel => buffer.push(5),
            ControlMessage::ExpandSettingsPanel => buffer.push(6),
            ControlMessage::CollapsePanels => buffer.push(7),
            ControlMessage::GetClipboard { copy_key } => {
                buffer.push(8);
                buffer.push(*copy_key);
            }
            ControlMessage::SetClipboard {
                sequence,
                text,
                paste,
            } => {
                buffer.push(9);
                buffer.extend_from_slice(&sequence.to_be_bytes());
                buffer.push(u8::from(*paste));
                push_string(&mut buffer, text, 4);
            }
            ControlMessage::SetDisplayPower { on } => {
                buffer.push(10);
                buffer.push(u8::from(*on));
            }
            ControlMessage::RotateDevice => buffer.push(11),
            ControlMessage::StartApp(name) => {
                buffer.push(16);
                push_string(&mut buffer, name, 1);
            }
            ControlMessage::ResetVideo => buffer.push(17),
            ControlMessage::CameraSetTorch(on) => {
                buffer.push(18);
                buffer.push(u8::from(*on));
            }
            ControlMessage::CameraZoomIn => buffer.push(19),
            ControlMessage::CameraZoomOut => buffer.push(20),
            ControlMessage::ResizeDisplay { width, height } => {
                buffer.push(21);
                buffer.extend_from_slice(&width.to_be_bytes());
                buffer.extend_from_slice(&height.to_be_bytes());
            }
            ControlMessage::ScanFile(path) => {
                buffer.push(22);
                push_string(&mut buffer, path, 4);
            }
        }
        buffer
    }
}

fn float_to_u16fp(value: f32) -> u16 {
    let clamped = value.clamp(0.0, 1.0);
    (clamped * u16::MAX as f32).round() as u16
}

fn float_to_i16fp(value: f32) -> i16 {
    let clamped = value.clamp(-1.0, 1.0);
    (clamped * i16::MAX as f32).round() as i16
}

pub struct ControlChannel {
    write: tokio::io::WriteHalf<TcpStream>,
    sequence: u64,
}

impl ControlChannel {
    pub fn next_sequence(&mut self) -> u64 {
        self.sequence = self.sequence.wrapping_add(1);
        self.sequence
    }

    pub async fn send(&mut self, message: &ControlMessage) -> Result<()> {
        let payload = message.serialize();
        self.write
            .write_all(&payload)
            .await
            .context("writing scrcpy control message")?;
        self.write.flush().await?;
        Ok(())
    }

    pub async fn tap(&mut self, x: i32, y: i32, width: u16, height: u16) -> Result<()> {
        let pointer_id = self.next_sequence() & 0x7fff_ffff;
        self.send(&ControlMessage::InjectTouch {
            action: ACTION_DOWN,
            pointer_id,
            x,
            y,
            screen_width: width,
            screen_height: height,
            pressure: 1.0,
            action_button: BUTTON_PRIMARY,
            buttons: BUTTON_PRIMARY,
        })
        .await?;
        self.send(&ControlMessage::InjectTouch {
            action: ACTION_UP,
            pointer_id,
            x,
            y,
            screen_width: width,
            screen_height: height,
            pressure: 0.0,
            action_button: BUTTON_PRIMARY,
            buttons: 0,
        })
        .await
    }

    pub async fn key(&mut self, keycode: u32) -> Result<()> {
        self.send(&ControlMessage::InjectKeycode {
            action: ACTION_DOWN,
            keycode,
            repeat: 0,
            metastate: 0,
        })
        .await?;
        self.send(&ControlMessage::InjectKeycode {
            action: ACTION_UP,
            keycode,
            repeat: 0,
            metastate: 0,
        })
        .await
    }
}

/// Device-to-host control messages. UHID output and clipboard acknowledgements
/// are decoded for protocol completeness; the UI only consumes clipboard text.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum DeviceMessage {
    Clipboard(String),
    ClipboardAck { sequence: u64 },
    UhidOutput { id: u16, data: Vec<u8> },
}

pub struct ControlReader {
    read: tokio::io::ReadHalf<TcpStream>,
}

impl ControlReader {
    pub async fn next_message(&mut self) -> Result<Option<DeviceMessage>> {
        let mut kind = [0u8; 1];
        match self.read.read_exact(&mut kind).await {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(error) => return Err(error).context("reading scrcpy device message"),
        }
        match kind[0] {
            0 => {
                let mut raw = [0u8; 4];
                self.read.read_exact(&mut raw).await?;
                let length = u32::from_be_bytes(raw) as usize;
                if length > MAX_PACKET_SIZE {
                    bail!("scrcpy clipboard message too large");
                }
                let mut data = vec![0u8; length];
                self.read.read_exact(&mut data).await?;
                Ok(Some(DeviceMessage::Clipboard(
                    String::from_utf8_lossy(&data).to_string(),
                )))
            }
            1 => {
                let mut raw = [0u8; 8];
                self.read.read_exact(&mut raw).await?;
                Ok(Some(DeviceMessage::ClipboardAck {
                    sequence: u64::from_be_bytes(raw),
                }))
            }
            2 => {
                let mut id = [0u8; 2];
                self.read.read_exact(&mut id).await?;
                let mut length = [0u8; 2];
                self.read.read_exact(&mut length).await?;
                let size = u16::from_be_bytes(length) as usize;
                let mut data = vec![0u8; size];
                self.read.read_exact(&mut data).await?;
                Ok(Some(DeviceMessage::UhidOutput {
                    id: u16::from_be_bytes(id),
                    data,
                }))
            }
            other => Err(anyhow!("unknown scrcpy device message type {other}")),
        }
    }
}

pub struct ScrcpySession {
    pub serial: String,
    pub device_name: String,
    pub local_port: u16,
    pub video: Option<VideoStream>,
    pub audio: Option<AudioStream>,
    pub control: Option<ControlChannel>,
    pub control_reader: Option<ControlReader>,
    adb: Adb,
    child: Child,
}

impl ScrcpySession {
    pub async fn start(
        adb: Adb,
        serial: &str,
        options: &ScrcpyOptions,
        data_dir: &Path,
    ) -> Result<Self> {
        if serial.trim().is_empty() {
            bail!("no Android serial selected");
        }
        if !options.video && !options.audio && !options.control {
            bail!("scrcpy refuses to start without video, audio or control");
        }
        if options.video {
            codec_id_for(&options.video_codec)
                .with_context(|| format!("unsupported video codec {}", options.video_codec))?;
        }
        if options.audio {
            codec_id_for(&options.audio_codec)
                .with_context(|| format!("unsupported audio codec {}", options.audio_codec))?;
        }
        let jar = resolve_server_jar(None, data_dir).ok_or_else(|| {
            anyhow!(
                "scrcpy-server.jar not found; place it in the app data directory or set LINKTOLINUX_SCRCPY_SERVER"
            )
        })?;
        adb.push(serial, &jar, SERVER_REMOTE_PATH).await?;

        let scid = uuid::Uuid::new_v4().as_u128() as u32;
        let socket_name = format!("scrcpy_{scid:08x}");
        let port = reserve_port()?;
        adb.forward(serial, port, &socket_name).await?;

        let arguments = options.arguments(scid);
        let command = format!(
            "CLASSPATH={} app_process / com.genymobile.scrcpy.Server {} {}",
            SERVER_REMOTE_PATH,
            SCRCPY_VERSION,
            arguments.join(" ")
        );
        let mut child = Command::new(adb.path())
            .args(["-s", serial, "shell", &command])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("starting the scrcpy server")?;

        let logs = Arc::new(Mutex::new(VecDeque::with_capacity(200)));
        if let Some(stdout) = child.stdout.take() {
            spawn_log_reader(stdout, "stdout", logs.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_log_reader(stderr, "stderr", logs.clone());
        }

        let connecting = async {
            let video = if options.video {
                Some(connect_tunnel(port).await?)
            } else {
                None
            };
            let audio = if options.audio {
                Some(connect_tunnel(port).await?)
            } else {
                None
            };
            let control = if options.control {
                Some(connect_tunnel(port).await?)
            } else {
                None
            };
            Ok::<_, anyhow::Error>((video, audio, control))
        };

        let (mut video_socket, mut audio_socket, mut control_socket) =
            match timeout(Duration::from_secs(30), connecting).await {
                Ok(result) => result?,
                Err(_) => {
                    let _ = child.start_kill();
                    let _ = adb.forward_remove(serial, port).await;
                    bail!("timed out waiting for the scrcpy server tunnel");
                }
            };

        let device_name = {
            let meta_socket = video_socket
                .as_mut()
                .or(audio_socket.as_mut())
                .or(control_socket.as_mut());
            match meta_socket {
                Some(socket) => read_device_name(socket).await,
                None => Err(anyhow!("scrcpy did not open any stream socket")),
            }
        };
        let device_name = match device_name {
            Ok(name) => name,
            Err(error) => {
                let _ = child.start_kill();
                let _ = adb.forward_remove(serial, port).await;
                return Err(error);
            }
        };

        let video = match video_socket {
            Some(stream) => Some(VideoStream::open(stream).await?),
            None => None,
        };
        let audio = match audio_socket {
            Some(stream) => Some(AudioStream::open(stream).await?),
            None => None,
        };
        let (control, control_reader) = match control_socket {
            Some(stream) => {
                let (read, write) = tokio::io::split(stream);
                (
                    Some(ControlChannel { write, sequence: 0 }),
                    Some(ControlReader { read }),
                )
            }
            None => (None, None),
        };

        Ok(Self {
            serial: serial.to_string(),
            device_name,
            local_port: port,
            video,
            audio,
            control,
            control_reader,
            adb,
            child,
        })
    }

    pub fn take_video(&mut self) -> Option<VideoStream> {
        self.video.take()
    }

    pub fn take_audio(&mut self) -> Option<AudioStream> {
        self.audio.take()
    }

    pub fn take_control_reader(&mut self) -> Option<ControlReader> {
        self.control_reader.take()
    }

    pub async fn stop(mut self) {
        let _ = self.child.start_kill();
        let _ = timeout(Duration::from_secs(5), self.child.wait()).await;
        let _ = self.adb.forward_remove(&self.serial, self.local_port).await;
    }
}

fn spawn_log_reader<R>(stream: R, tag: &'static str, logs: Arc<Mutex<VecDeque<String>>>)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(mut logs) = logs.lock() {
                if logs.len() >= 200 {
                    logs.pop_front();
                }
                logs.push_back(format!("[{tag}] {line}"));
            }
        }
    });
}

async fn connect_tunnel(port: u16) -> Result<TcpStream> {
    let mut last_error = None;
    for _ in 0..CONNECT_ATTEMPTS {
        match TcpStream::connect(("127.0.0.1", port)).await {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                return Ok(stream);
            }
            Err(error) => {
                last_error = Some(error);
                sleep(CONNECT_DELAY).await;
            }
        }
    }
    Err(anyhow!(
        "could not reach the scrcpy tunnel on port {port}: {}",
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "unknown error".to_string())
    ))
}

async fn read_device_name(stream: &mut TcpStream) -> Result<String> {
    let mut buffer = [0u8; DEVICE_NAME_FIELD_LENGTH];
    stream
        .read_exact(&mut buffer)
        .await
        .context("reading the scrcpy device name")?;
    let end = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    Ok(String::from_utf8_lossy(&buffer[..end]).to_string())
}

pub async fn capture_screenshot(adb: &Adb, serial: &str, destination: &Path) -> Result<PathBuf> {
    let bytes = adb.exec_out(serial, "screencap -p").await?;
    if bytes.len() < 8 || &bytes[1..4] != b"PNG" {
        bail!("device returned an invalid screenshot");
    }
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    tokio::fs::write(destination, &bytes)
        .await
        .with_context(|| format!("writing {}", destination.display()))?;
    Ok(destination.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_keycode_message() {
        let payload = ControlMessage::InjectKeycode {
            action: ACTION_DOWN,
            keycode: KEYCODE_HOME,
            repeat: 0,
            metastate: 0,
        }
        .serialize();
        assert_eq!(payload.len(), 14);
        assert_eq!(payload[0], 0);
        assert_eq!(&payload[2..6], &KEYCODE_HOME.to_be_bytes());
    }

    #[test]
    fn serializes_touch_message() {
        let payload = ControlMessage::InjectTouch {
            action: ACTION_DOWN,
            pointer_id: 7,
            x: 100,
            y: 200,
            screen_width: 1080,
            screen_height: 2400,
            pressure: 1.0,
            action_button: 1,
            buttons: 1,
        }
        .serialize();
        assert_eq!(payload.len(), 32);
        assert_eq!(payload[0], 2);
        assert_eq!(&payload[10..14], &100i32.to_be_bytes());
    }

    #[test]
    fn maps_codecs() {
        assert_eq!(codec_name(codec_id_for("h264").unwrap()), "h264");
        assert_eq!(codec_name(codec_id_for("opus").unwrap()), "opus");
        assert!(codec_id_for("mpeg2").is_err());
    }
    #[test]
    fn binary_resolver_rejects_non_executable_files() {
        use std::os::unix::fs::PermissionsExt;
        let root =
            std::env::temp_dir().join(format!("linktolinux-executable-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let binary = root.join("helper");
        std::fs::write(&binary, b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            super::resolve_binary(binary.to_str().unwrap(), "LINKTOLINUX_TEST_UNUSED").is_none()
        );
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            super::resolve_binary(binary.to_str().unwrap(), "LINKTOLINUX_TEST_UNUSED"),
            Some(binary)
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
