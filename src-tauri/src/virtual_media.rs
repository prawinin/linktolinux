use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::adb_scrcpy::resolve_binary;
use crate::models::VideoDevice;

pub const MIC_SINK_NAME: &str = "linktolinux_mic_sink";
pub const MIC_SOURCE_NAME: &str = "linktolinux_mic";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_BUFFERED_FRAME: usize = 16 * 1024 * 1024;

pub fn binary_available(command: &str) -> bool {
    resolve_binary(command, "").is_some()
}

pub fn video_devices() -> Vec<VideoDevice> {
    let mut devices = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/class/video4linux") else {
        return devices;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with("video") {
            continue;
        }
        let label = std::fs::read_to_string(path.join("name"))
            .map(|value| value.trim().to_string())
            .unwrap_or_else(|_| name.to_string());
        devices.push(VideoDevice {
            path: format!("/dev/{name}"),
            label: label.clone(),
            loopback: is_loopback(&label),
        });
    }
    devices.sort_by(|left, right| left.path.cmp(&right.path));
    devices
}

fn is_loopback(label: &str) -> bool {
    let lower = label.to_ascii_lowercase();
    lower.contains("dummy video") || lower.contains("loopback") || lower.starts_with("linktolinux")
}

pub fn loopback_devices() -> Vec<VideoDevice> {
    video_devices()
        .into_iter()
        .filter(|device| device.loopback)
        .collect()
}

pub fn default_loopback() -> Option<VideoDevice> {
    loopback_devices().into_iter().next()
}

pub fn v4l2loopback_loaded() -> bool {
    Path::new("/sys/module/v4l2loopback").exists()
}

pub fn loopback_hint() -> String {
    if v4l2loopback_loaded() {
        "v4l2loopback is loaded but exposes no video node; create one with \
         sudo modprobe v4l2loopback exclusive_caps=1 video_nr=10 card_label=LinkToLinux"
            .to_string()
    } else {
        "v4l2loopback is not loaded; install v4l2loopback-dkms and run \
         sudo modprobe v4l2loopback exclusive_caps=1 video_nr=10 card_label=LinkToLinux"
            .to_string()
    }
}

fn sink_exists(name: &str) -> bool {
    list_short("sinks")
        .map(|value| {
            value
                .lines()
                .any(|line| line.split_whitespace().nth(1) == Some(name))
        })
        .unwrap_or(false)
}

fn source_exists(name: &str) -> bool {
    list_short("sources")
        .map(|value| {
            value
                .lines()
                .any(|line| line.split_whitespace().nth(1) == Some(name))
        })
        .unwrap_or(false)
}

fn list_short(kind: &str) -> Result<String> {
    let output = std::process::Command::new("pactl")
        .args(["list", "short", kind])
        .output()
        .context("running pactl list")?;
    if !output.status.success() {
        bail!("pactl list {kind} failed");
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn run_command(program: &str, args: &[&str]) -> Result<String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = command
        .spawn()
        .with_context(|| format!("spawning {program}"))?;
    let output = timeout(COMMAND_TIMEOUT, child.wait_with_output())
        .await
        .with_context(|| format!("{program} timed out"))?
        .with_context(|| format!("waiting for {program}"))?;
    if !output.status.success() {
        bail!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// A PipeWire/PulseAudio route that exposes the phone microphone as a real source.
pub struct AudioRoute {
    sink_name: String,
    source_name: String,
    modules: Vec<String>,
}

impl AudioRoute {
    pub async fn ensure() -> Result<Self> {
        if !binary_available("pactl") {
            bail!("pactl is required to create the virtual microphone");
        }
        let mut modules = Vec::new();
        if !sink_exists(MIC_SINK_NAME) {
            let id = run_command(
                "pactl",
                &[
                    "load-module",
                    "module-null-sink",
                    &format!("sink_name={MIC_SINK_NAME}"),
                    "sink_properties=device.description=LinkToLinux_Phone_Mic",
                ],
            )
            .await?;
            modules.push(id);
        }
        if !source_exists(MIC_SOURCE_NAME) {
            let id = run_command(
                "pactl",
                &[
                    "load-module",
                    "module-remap-source",
                    &format!("master={MIC_SINK_NAME}.monitor"),
                    &format!("source_name={MIC_SOURCE_NAME}"),
                    "source_properties=device.description=LinkToLinux_Phone_Mic",
                ],
            )
            .await?;
            modules.push(id);
        }
        Ok(Self {
            sink_name: MIC_SINK_NAME.to_string(),
            source_name: MIC_SOURCE_NAME.to_string(),
            modules,
        })
    }

    pub fn sink_name(&self) -> &str {
        &self.sink_name
    }

    pub fn source_name(&self) -> &str {
        &self.source_name
    }

    pub async fn teardown(&self) {
        for module in self.modules.iter().rev() {
            let _ = run_command("pactl", &["unload-module", module]).await;
        }
    }
}

pub struct MjpegPreview {
    pub port: u16,
    _child: Child,
    tasks: Vec<JoinHandle<()>>,
    shutdown: watch::Sender<bool>,
}

impl MjpegPreview {
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/stream", self.port)
    }
    pub fn check(&mut self) -> Result<()> {
        child_health(&mut self._child, "Camera preview")
    }
}

impl Drop for MjpegPreview {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        for task in &self.tasks {
            task.abort();
        }
    }
}

fn codec_input_format(codec: &str) -> Result<&'static str> {
    match codec {
        "h264" => Ok("h264"),
        "h265" => Ok("hevc"),
        "av1" => Ok("av1"),
        "vp8" => Ok("vp8"),
        "vp9" => Ok("vp9"),
        other => Err(anyhow!("unsupported video codec {other}")),
    }
}

fn spawn_decoder(ffmpeg: &Path, codec: &str, fps: u32, sink: Vec<String>) -> Result<Child> {
    let format = codec_input_format(codec)?;
    let mut command = Command::new(ffmpeg);
    command.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-fflags",
        "nobuffer",
        "-flags",
        "low_delay",
        "-probesize",
        "32768",
        "-analyzeduration",
        "100000",
    ]);
    if fps > 0 {
        // Raw phone streams may advertise a codec clock instead of the capture rate.
        command.args(["-r", &fps.to_string()]);
    }
    command
        .args(["-f", format, "-i", "pipe:0", "-an"])
        .args(&sink)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command.spawn().context("starting ffmpeg decoder")
}

fn scale_filter(width: u32, height: u32) -> Option<String> {
    if width == 0 || height == 0 {
        None
    } else {
        Some(format!(
            "scale={width}:{height}:force_original_aspect_ratio=decrease"
        ))
    }
}

fn orientation_filter(rotation: u32) -> Result<Option<&'static str>> {
    match rotation {
        0 => Ok(None),
        90 => Ok(Some("transpose=clock")),
        180 => Ok(Some("hflip,vflip")),
        270 => Ok(Some("transpose=cclock")),
        _ => bail!("Invalid camera rotation"),
    }
}

fn oriented_dimensions(width: u32, height: u32, rotation: u32) -> Result<(u32, u32)> {
    orientation_filter(rotation)?;
    Ok(if rotation == 90 || rotation == 270 {
        (height, width)
    } else {
        (width, height)
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoOrientation {
    pub width: u32,
    pub height: u32,
    pub rotation: u32,
}

pub fn start_mjpeg_preview(
    ffmpeg: &Path,
    codec: &str,
    frames: broadcast::Receiver<Arc<Vec<u8>>>,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<MjpegPreview> {
    start_mjpeg_preview_oriented(
        ffmpeg,
        codec,
        frames,
        VideoOrientation {
            width,
            height,
            rotation: 0,
        },
        fps,
    )
}

pub fn start_mjpeg_preview_oriented(
    ffmpeg: &Path,
    codec: &str,
    mut frames: broadcast::Receiver<Arc<Vec<u8>>>,
    orientation: VideoOrientation,
    fps: u32,
) -> Result<MjpegPreview> {
    let (width, height) =
        oriented_dimensions(orientation.width, orientation.height, orientation.rotation)?;
    let listener =
        std::net::TcpListener::bind(("127.0.0.1", 0)).context("binding the preview server")?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();

    let mut sink = vec![
        "-f".to_string(),
        "mjpeg".to_string(),
        "-q:v".to_string(),
        "5".to_string(),
    ];
    let mut filters = Vec::new();
    if let Some(filter) = orientation_filter(orientation.rotation)? {
        filters.push(filter.to_string());
    }
    if let Some(filter) = scale_filter(width, height) {
        filters.push(filter);
    }
    if !filters.is_empty() {
        sink.push("-vf".to_string());
        sink.push(filters.join(","));
    }
    if fps > 0 {
        sink.push("-r".to_string());
        sink.push(fps.to_string());
    }
    sink.push("pipe:1".to_string());

    let mut child = spawn_decoder(ffmpeg, codec, fps, sink)?;
    let mut stdin = child.stdin.take().context("ffmpeg stdin missing")?;
    let mut stdout = child.stdout.take().context("ffmpeg stdout missing")?;

    let (shutdown, _) = watch::channel(false);
    let (jpeg_tx, _) = broadcast::channel::<Arc<Vec<u8>>>(8);
    let mut tasks = Vec::new();

    tasks.push(tokio::spawn(async move {
        loop {
            match frames.recv().await {
                Ok(frame) => {
                    if stdin.write_all(&frame).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
        let _ = stdin.shutdown().await;
    }));

    let parser = jpeg_tx.clone();
    tasks.push(tokio::spawn(async move {
        let mut buffer: Vec<u8> = Vec::with_capacity(256 * 1024);
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let read = match stdout.read(&mut chunk).await {
                Ok(0) => break,
                Ok(read) => read,
                Err(_) => break,
            };
            buffer.extend_from_slice(&chunk[..read]);
            loop {
                let Some(start) = find_marker(&buffer, 0, &[0xFF, 0xD8]) else {
                    if buffer.len() > 2 {
                        let keep = buffer.len() - 1;
                        buffer.drain(..keep);
                    }
                    break;
                };
                if start > 0 {
                    buffer.drain(..start);
                }
                let Some(end) = find_marker(&buffer, 2, &[0xFF, 0xD9]) else {
                    break;
                };
                let frame = buffer[..end + 2].to_vec();
                buffer.drain(..end + 2);
                let _ = parser.send(Arc::new(frame));
            }
            if buffer.len() > MAX_BUFFERED_FRAME {
                buffer.clear();
            }
        }
    }));

    let accept_shutdown = shutdown.subscribe();
    let accept_sender = shutdown.clone();
    tasks.push(tokio::spawn(async move {
        let mut accept_shutdown = accept_shutdown;
        let server = match TcpListener::from_std(listener) {
            Ok(server) => server,
            Err(_) => return,
        };
        loop {
            tokio::select! {
                _ = accept_shutdown.changed() => break,
                accepted = server.accept() => {
                    let Ok((socket, _)) = accepted else { continue };
                    let subscriber = jpeg_tx.subscribe();
                    let mut client_shutdown = accept_sender.subscribe();
                    tokio::spawn(async move {
                        serve_mjpeg(socket, subscriber, &mut client_shutdown).await;
                    });
                }
            }
        }
    }));

    Ok(MjpegPreview {
        port,
        _child: child,
        tasks,
        shutdown,
    })
}

fn find_marker(buffer: &[u8], from: usize, marker: &[u8; 2]) -> Option<usize> {
    if buffer.len() < from + 2 {
        return None;
    }
    buffer[from..]
        .windows(2)
        .position(|window| window == marker)
        .map(|index| index + from)
}

async fn serve_mjpeg(
    mut socket: tokio::net::TcpStream,
    mut frames: broadcast::Receiver<Arc<Vec<u8>>>,
    shutdown: &mut watch::Receiver<bool>,
) {
    let header = "HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=frame\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n";
    if socket.write_all(header.as_bytes()).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            received = frames.recv() => {
                let frame = match received {
                    Ok(frame) => frame,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let mut part = Vec::with_capacity(frame.len() + 96);
                part.extend_from_slice(b"--frame\r\nContent-Type: image/jpeg\r\nContent-Length: ");
                part.extend_from_slice(frame.len().to_string().as_bytes());
                part.extend_from_slice(b"\r\n\r\n");
                part.extend_from_slice(&frame);
                part.extend_from_slice(b"\r\n");
                if socket.write_all(&part).await.is_err() {
                    break;
                }
            }
        }
    }
}

pub struct V4l2Sink {
    _child: Child,
    feeder: JoinHandle<()>,
}
impl V4l2Sink {
    pub fn check(&mut self) -> Result<()> {
        child_health(
            &mut self._child,
            "Virtual webcam; check v4l2loopback permissions and whether another app is using it",
        )
    }
}

impl Drop for V4l2Sink {
    fn drop(&mut self) {
        self.feeder.abort();
    }
}

pub fn start_v4l2_sink(
    ffmpeg: &Path,
    device: &str,
    codec: &str,
    frames: broadcast::Receiver<Arc<Vec<u8>>>,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<V4l2Sink> {
    start_v4l2_sink_oriented(
        ffmpeg,
        device,
        codec,
        frames,
        VideoOrientation {
            width,
            height,
            rotation: 0,
        },
        fps,
    )
}

pub fn start_v4l2_sink_oriented(
    ffmpeg: &Path,
    device: &str,
    codec: &str,
    mut frames: broadcast::Receiver<Arc<Vec<u8>>>,
    orientation: VideoOrientation,
    fps: u32,
) -> Result<V4l2Sink> {
    let (width, height) =
        oriented_dimensions(orientation.width, orientation.height, orientation.rotation)?;
    if !device.starts_with("/dev/video") {
        bail!("v4l2 sink must target a /dev/video node");
    }
    if width == 0 || height == 0 {
        bail!("the v4l2 sink needs the negotiated capture size");
    }
    let mut sink = vec![
        "-pix_fmt".to_string(),
        "yuyv422".to_string(),
        "-s".to_string(),
        format!("{width}x{height}"),
    ];
    if let Some(filter) = orientation_filter(orientation.rotation)? {
        sink.push("-vf".to_string());
        sink.push(filter.to_string());
    }
    if fps > 0 {
        sink.push("-r".to_string());
        sink.push(fps.to_string());
    }
    sink.push("-f".to_string());
    sink.push("v4l2".to_string());
    sink.push(device.to_string());

    let mut child = spawn_decoder(ffmpeg, codec, fps, sink)?;
    let mut stdin = child.stdin.take().context("ffmpeg stdin missing")?;
    let feeder = tokio::spawn(async move {
        loop {
            match frames.recv().await {
                Ok(frame) => {
                    if stdin.write_all(&frame).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
        let _ = stdin.shutdown().await;
    });
    Ok(V4l2Sink {
        _child: child,
        feeder,
    })
}

pub struct MicFeed {
    _decoder: Child,
    _player: Child,
    pump: JoinHandle<()>,
    feeder: JoinHandle<()>,
}
impl MicFeed {
    pub fn check(&mut self) -> Result<()> {
        child_health(&mut self._decoder, "Microphone decoder")?;
        child_health(
            &mut self._player,
            "Microphone output; check the PipeWire audio session",
        )
    }
}
fn child_health(child: &mut Child, label: &str) -> Result<()> {
    if let Some(status) = child
        .try_wait()
        .with_context(|| format!("Checking {label}"))?
    {
        bail!("{label} stopped ({status}). Stop capture and retry.");
    }
    Ok(())
}

impl Drop for MicFeed {
    fn drop(&mut self) {
        self.pump.abort();
        self.feeder.abort();
    }
}

pub fn start_mic_feed(
    ffmpeg: &Path,
    route: &AudioRoute,
    codec: &str,
    mut frames: broadcast::Receiver<Arc<Vec<u8>>>,
    channels: u32,
) -> Result<MicFeed> {
    if !binary_available("pw-cat") {
        bail!("pw-cat is required to feed the virtual microphone");
    }
    let channels = channels.clamp(1, 2);
    let channel_arg = channels.to_string();
    let input = match codec {
        "opus" => vec!["-f", "opus", "-i", "pipe:0"],
        "aac" => vec!["-f", "aac", "-i", "pipe:0"],
        "flac" => vec!["-f", "flac", "-i", "pipe:0"],
        "raw" => vec![
            "-f",
            "s16le",
            "-ar",
            "48000",
            "-ac",
            channel_arg.as_str(),
            "-i",
            "pipe:0",
        ],
        other => return Err(anyhow!("unsupported audio codec {other}")),
    };

    let mut decoder = Command::new(ffmpeg);
    decoder
        .args(["-hide_banner", "-loglevel", "error", "-fflags", "nobuffer"])
        .args(&input)
        .args([
            "-f",
            "s16le",
            "-ar",
            "48000",
            "-ac",
            channel_arg.as_str(),
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut decoder = decoder.spawn().context("starting the audio decoder")?;
    let mut decoder_stdin = decoder.stdin.take().context("decoder stdin missing")?;
    let mut decoder_stdout = decoder.stdout.take().context("decoder stdout missing")?;

    let mut player = Command::new("pw-cat");
    player
        .args([
            "--playback",
            "--raw",
            "--target",
            route.sink_name(),
            "--format",
            "s16",
            "--rate",
            "48000",
            "--channels",
            channel_arg.as_str(),
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut player = player.spawn().context("starting pw-cat")?;
    let mut player_stdin = player.stdin.take().context("pw-cat stdin missing")?;

    let feeder = tokio::spawn(async move {
        loop {
            match frames.recv().await {
                Ok(frame) => {
                    if decoder_stdin.write_all(&frame).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
        let _ = decoder_stdin.shutdown().await;
    });

    let pump = tokio::spawn(async move {
        let mut chunk = vec![0u8; 32 * 1024];
        loop {
            match decoder_stdout.read(&mut chunk).await {
                Ok(0) => break,
                Ok(read) => {
                    if player_stdin.write_all(&chunk[..read]).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = player_stdin.shutdown().await;
    });

    Ok(MicFeed {
        _decoder: decoder,
        _player: player,
        pump,
        feeder,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_loopback_labels() {
        assert!(is_loopback("Dummy video device (0x0000)"));
        assert!(is_loopback("v4l2 loopback device"));
        assert!(is_loopback("LinkToLinux Camera"));
        assert!(!is_loopback("Integrated Camera"));
    }

    #[test]
    fn finds_jpeg_markers() {
        let buffer = vec![0x00, 0xFF, 0xD8, 1, 2, 0xFF, 0xD9, 0x00];
        let start = find_marker(&buffer, 0, &[0xFF, 0xD8]).unwrap();
        let end = find_marker(&buffer, 2, &[0xFF, 0xD9]).unwrap();
        assert_eq!(start, 1);
        assert_eq!(end, 5);
    }

    #[test]
    fn builds_scale_filter() {
        assert!(scale_filter(0, 0).is_none());
        assert_eq!(
            scale_filter(1280, 720).unwrap(),
            "scale=1280:720:force_original_aspect_ratio=decrease"
        );
    }

    #[test]
    fn camera_orientation_rotates_geometry_and_builds_filters() {
        for (rotation, dimensions, filter) in [
            (0, (640, 480), None),
            (90, (480, 640), Some("transpose=clock")),
            (180, (640, 480), Some("hflip,vflip")),
            (270, (480, 640), Some("transpose=cclock")),
        ] {
            assert_eq!(oriented_dimensions(640, 480, rotation).unwrap(), dimensions);
            assert_eq!(orientation_filter(rotation).unwrap(), filter);
        }
        assert!(orientation_filter(45).is_err());
    }

    #[tokio::test]
    async fn camera_rotation_changes_preview_pixels_and_dimensions() {
        let ffmpeg = resolve_binary("ffmpeg", "LINKTOLINUX_FFMPEG").expect("ffmpeg required");
        let encoded = Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=blue:size=64x48:rate=24,drawbox=x=0:y=0:w=32:h=48:color=red:t=fill",
                "-frames:v",
                "48",
                "-c:v",
                "libx264",
                "-threads",
                "1",
                "-preset",
                "ultrafast",
                "-tune",
                "zerolatency",
                "-g",
                "4",
                "-f",
                "h264",
                "pipe:1",
            ])
            .kill_on_drop(true)
            .output()
            .await
            .unwrap();
        assert!(encoded.status.success());
        let (tx, _) = broadcast::channel(32);
        let preview = start_mjpeg_preview_oriented(
            &ffmpeg,
            "h264",
            tx.subscribe(),
            VideoOrientation {
                width: 64,
                height: 48,
                rotation: 90,
            },
            24,
        )
        .unwrap();
        let mut client = tokio::net::TcpStream::connect(("127.0.0.1", preview.port))
            .await
            .unwrap();
        let sender = tokio::spawn(async move {
            for _ in 0..40 {
                let _ = tx.send(Arc::new(encoded.stdout.clone()));
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        });
        let jpeg = timeout(Duration::from_secs(3), async {
            let mut response = Vec::new();
            let mut chunk = [0; 65536];
            loop {
                let count = client.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                response.extend_from_slice(&chunk[..count]);
                if let Some(start) = find_marker(&response, 0, &[0xff, 0xd8]) {
                    if let Some(end) = find_marker(&response, start + 2, &[0xff, 0xd9]) {
                        break response[start..end + 2].to_vec();
                    }
                }
            }
        })
        .await;
        sender.abort();
        let jpeg = jpeg.expect("rotated preview must produce a frame");
        let sof = jpeg
            .windows(2)
            .position(|marker| marker == [0xff, 0xc0])
            .expect("JPEG dimensions");
        assert_eq!(u16::from_be_bytes([jpeg[sof + 5], jpeg[sof + 6]]), 64);
        assert_eq!(u16::from_be_bytes([jpeg[sof + 7], jpeg[sof + 8]]), 48);
        let mut decoder = Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "mjpeg",
                "-i",
                "pipe:0",
                "-frames:v",
                "1",
                "-threads",
                "1",
                "-pix_fmt",
                "rgb24",
                "-f",
                "rawvideo",
                "pipe:1",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        decoder
            .stdin
            .take()
            .unwrap()
            .write_all(&jpeg)
            .await
            .unwrap();
        let pixels = timeout(Duration::from_secs(3), decoder.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert!(pixels.status.success());
        let top = (8 * 48 + 24) * 3;
        let bottom = (56 * 48 + 24) * 3;
        assert!(
            pixels.stdout[top] > pixels.stdout[top + 2] + 100,
            "red left half must rotate to top"
        );
        assert!(
            pixels.stdout[bottom + 2] > pixels.stdout[bottom] + 100,
            "blue right half must rotate to bottom"
        );
    }

    #[tokio::test]
    async fn preview_decodes_live_h264_without_waiting_for_eof() {
        let ffmpeg = resolve_binary("ffmpeg", "LINKTOLINUX_FFMPEG")
            .expect("ffmpeg required for media regression test");
        let encoded = Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=blue:size=64x48:rate=10",
                "-frames:v",
                "24",
                "-c:v",
                "libx264",
                "-threads",
                "1",
                "-preset",
                "ultrafast",
                "-tune",
                "zerolatency",
                "-g",
                "4",
                "-f",
                "h264",
                "pipe:1",
            ])
            .kill_on_drop(true)
            .output()
            .await
            .unwrap();
        assert!(encoded.status.success());
        let (tx, _) = broadcast::channel(32);
        let preview = start_mjpeg_preview(&ffmpeg, "h264", tx.subscribe(), 64, 48, 10).unwrap();
        let mut client = tokio::net::TcpStream::connect(("127.0.0.1", preview.port))
            .await
            .unwrap();
        client
            .write_all(b"GET /stream HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let sender = tokio::spawn(async move {
            for _ in 0..30 {
                let _ = tx.send(Arc::new(encoded.stdout.clone()));
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let mut response = Vec::new();
        let result = timeout(Duration::from_secs(3), async {
            let mut buffer = [0; 4096];
            while find_marker(&response, 0, &[0xff, 0xd8]).is_none() {
                let count = client.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                response.extend_from_slice(&buffer[..count]);
            }
        })
        .await;
        sender.abort();
        result.expect("live preview must emit JPEG without the phone closing its stream");
    }

    #[tokio::test]
    async fn camera_preview_advances_with_phone_encoder_clock_metadata() {
        let ffmpeg = resolve_binary("ffmpeg", "LINKTOLINUX_FFMPEG").expect("ffmpeg required");
        let encoded = Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x240:rate=24",
                "-frames:v",
                "72",
                "-c:v",
                "libx264",
                "-threads",
                "1",
                "-preset",
                "ultrafast",
                "-tune",
                "zerolatency",
                "-g",
                "24",
                "-bsf:v",
                "h264_metadata=tick_rate=2000000/1:fixed_frame_rate_flag=1",
                "-f",
                "h264",
                "pipe:1",
            ])
            .kill_on_drop(true)
            .output()
            .await
            .unwrap();
        assert!(encoded.status.success());
        let (tx, _) = broadcast::channel(32);
        let preview = start_mjpeg_preview(&ffmpeg, "h264", tx.subscribe(), 320, 240, 24).unwrap();
        let mut client = tokio::net::TcpStream::connect(("127.0.0.1", preview.port))
            .await
            .unwrap();
        client
            .write_all(b"GET /stream HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let sender = tokio::spawn(async move {
            for _ in 0..60 {
                let _ = tx.send(Arc::new(encoded.stdout.clone()));
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let result = timeout(Duration::from_secs(2), async {
            let mut response = Vec::new();
            let mut chunk = [0; 65536];
            loop {
                let count = client.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                response.extend_from_slice(&chunk[..count]);
                if response
                    .windows(2)
                    .filter(|marker| *marker == [0xff, 0xd8])
                    .count()
                    >= 12
                {
                    break;
                }
            }
        })
        .await;
        sender.abort();
        result.expect("phone encoder clock metadata must not freeze preview after the first frame");
    }

    #[tokio::test]
    #[ignore = "requires a running PipeWire desktop session"]
    async fn microphone_routes_mono_pcm_into_pipewire() {
        let ffmpeg = resolve_binary("ffmpeg", "LINKTOLINUX_FFMPEG").expect("ffmpeg required");
        let route = AudioRoute::ensure().await.unwrap();
        let (tx, _) = broadcast::channel(64);
        let feed = start_mic_feed(&ffmpeg, &route, "raw", tx.subscribe(), 1).unwrap();
        let sender = tokio::spawn(async move {
            for frame in 0..500 {
                let mut data = Vec::with_capacity(960);
                for sample in 0..480 {
                    let phase =
                        ((frame * 480 + sample) as f64) * 440.0 * std::f64::consts::TAU / 48000.0;
                    data.extend_from_slice(&((phase.sin() * 8000.0) as i16).to_le_bytes());
                }
                let _ = tx.send(Arc::new(data));
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        tokio::time::sleep(Duration::from_millis(250)).await;
        let mut capture = Command::new("pw-cat")
            .args([
                "--record",
                "--raw",
                "--target",
                route.source_name(),
                "--rate",
                "48000",
                "--channels",
                "1",
                "--format",
                "s16",
                "--properties",
                "node.dont-fallback=true",
                "-",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut samples = vec![0; 96000];
        let output = timeout(
            Duration::from_secs(6),
            capture.stdout.as_mut().unwrap().read_exact(&mut samples),
        )
        .await;
        capture.kill().await.unwrap();
        let _ = capture.wait().await;
        sender.abort();
        drop(feed);
        route.teardown().await;
        output
            .expect("recording from virtual microphone timed out")
            .unwrap();
        assert!(
            samples
                .chunks_exact(2)
                .any(|s| i16::from_le_bytes([s[0], s[1]]).unsigned_abs() > 1000),
            "virtual microphone must contain the transmitted tone"
        );
    }
}
