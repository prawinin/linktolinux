mod adb_scrcpy;
mod clipboard;
mod config;
mod file_transfer;
mod lan_stream;
mod models;
mod virtual_media;

use adb_scrcpy::{Adb, ControlChannel, DeviceMessage, ScrcpyOptions, ScrcpySession};
use anyhow::{anyhow, bail, Context, Result};
use models::{AdbDevice, CaptureTelemetry, DeviceView};
use serde::Serialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;
use virtual_media::{AudioRoute, MicFeed, MjpegPreview, V4l2Sink};

const PREVIEW_FPS_FALLBACK: u32 = 24;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub serial: String,
    pub device_name: String,
    pub mode: String,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub width: u32,
    pub height: u32,
    pub preview_url: Option<String>,
    pub v4l2_device: Option<String>,
    pub audio_source: Option<String>,
    pub frames: u64,
}

#[derive(Default)]
struct MediaRuntime {
    session: Option<ScrcpySession>,
    video_pump: Option<JoinHandle<()>>,
    audio_pump: Option<JoinHandle<()>>,
    control_task: Option<JoinHandle<()>>,
    preview: Option<MjpegPreview>,
    sink: Option<V4l2Sink>,
    mic: Option<MicFeed>,
    route: Option<AudioRoute>,
}

pub struct AppState {
    app: AppHandle,
    data_dir: PathBuf,
    media: Mutex<MediaRuntime>,
    files: Arc<file_transfer::FileTransfers>,
    lan: Mutex<Option<Arc<lan_stream::LanServer>>>,
    capture: Arc<StdMutex<CaptureTelemetry>>,
    frames: Arc<AtomicU64>,
    shutting_down: AtomicBool,
    clipboard_sync: Arc<AtomicBool>,
    clipboard_sync_write: StdMutex<()>,
}

impl AppState {
    fn new(app: AppHandle, data_dir: PathBuf) -> Self {
        Self {
            clipboard_sync: Arc::new(AtomicBool::new(config::load_clipboard_sync(&data_dir))),
            clipboard_sync_write: StdMutex::new(()),
            files: file_transfer::FileTransfers::new(
                dirs::download_dir()
                    .unwrap_or_else(|| data_dir.clone())
                    .join("LinkToLinux"),
            ),
            app,
            data_dir,
            media: Mutex::new(MediaRuntime::default()),
            lan: Mutex::new(None),
            capture: Arc::new(StdMutex::new(CaptureTelemetry::default())),
            frames: Arc::new(AtomicU64::new(0)),
            shutting_down: AtomicBool::new(false),
        }
    }

    fn telemetry(&self) -> CaptureTelemetry {
        self.capture
            .lock()
            .map(|capture| capture.clone())
            .unwrap_or_default()
    }

    fn update_capture(&self, update: impl FnOnce(&mut CaptureTelemetry)) {
        if let Ok(mut capture) = self.capture.lock() {
            update(&mut capture);
        }
    }

    fn downloads(&self) -> PathBuf {
        dirs::download_dir()
            .unwrap_or_else(|| self.data_dir.clone())
            .join("LinkToLinux")
    }
}

fn install_server_jar(data_dir: &Path) -> Result<()> {
    let target = data_dir.join(adb_scrcpy::SERVER_JAR_NAME);
    let bundled = include_bytes!("../resources/scrcpy-server.jar");
    if std::fs::read(&target).ok().as_deref() != Some(bundled.as_slice()) {
        std::fs::write(&target, bundled).context("Installing bundled Android screen server")?;
    }
    Ok(())
}

#[tauri::command]
async fn list_devices() -> Vec<DeviceView> {
    lan_stream::get_lan_devices().await
}

fn failure(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[tauri::command]
async fn list_adb_devices() -> Result<Vec<AdbDevice>, String> {
    let adb = Adb::locate().map_err(failure)?;
    adb.devices().await.map_err(failure)
}

fn validate_endpoint(address: &str) -> Result<String> {
    let address = address.trim();
    if address.is_empty() || address.contains(char::is_whitespace) {
        bail!("device address must look like 192.168.1.20:37000");
    }
    let mut parts = address.rsplitn(2, ':');
    let port = parts
        .next()
        .and_then(|port| port.parse::<u16>().ok())
        .ok_or_else(|| anyhow!("device address must include a port"))?;
    if port == 0 {
        bail!("device address must include a valid port");
    }
    let host = parts.next().unwrap_or_default();
    if host.is_empty() || host.contains(':') {
        bail!("use an IPv4 address or hostname for adb pairing");
    }
    Ok(address.to_string())
}

#[tauri::command]
async fn adb_pair(address: String, code: String) -> Result<String, String> {
    let address = validate_endpoint(&address).map_err(failure)?;
    let adb = Adb::locate().map_err(failure)?;
    adb.pair(&address, &code).await.map_err(failure)
}

#[tauri::command]
async fn adb_connect(address: String) -> Result<String, String> {
    let address = validate_endpoint(&address).map_err(failure)?;
    let adb = Adb::locate().map_err(failure)?;
    adb.connect(&address).await.map_err(failure)
}

#[tauri::command]
async fn start_capture(
    state: State<'_, AppState>,
    serial: String,
    mode: String,
    max_size: Option<u32>,
    max_fps: Option<u32>,
    video_device: Option<String>,
    preview: Option<bool>,
) -> Result<SessionView, String> {
    let mode = mode.to_ascii_lowercase();
    let mut runtime = state.media.lock().await;
    stop_capture_locked(&state, &mut runtime).await;

    let result = start_capture_locked(
        &state,
        &mut runtime,
        &serial,
        &mode,
        CaptureOptions {
            max_size: max_size.unwrap_or(1280),
            max_fps: max_fps.unwrap_or(24),
            video_device,
            preview: preview.unwrap_or(true),
        },
    )
    .await;

    if result.is_err() {
        stop_capture_locked(&state, &mut runtime).await;
    }
    result
}

struct CaptureOptions {
    max_size: u32,
    max_fps: u32,
    video_device: Option<String>,
    preview: bool,
}

async fn start_capture_locked(
    state: &AppState,
    runtime: &mut MediaRuntime,
    serial: &str,
    mode: &str,
    capture_options: CaptureOptions,
) -> Result<SessionView, String> {
    let CaptureOptions {
        max_size,
        max_fps,
        video_device,
        preview,
    } = capture_options;
    let adb = Adb::locate().map_err(failure)?;
    let mut options = ScrcpyOptions {
        max_size,
        max_fps,
        ..ScrcpyOptions::default()
    };

    match mode {
        "mirror" => {
            options.video = true;
            options.audio = true;
            options.control = true;
            options.video_source = "display".to_string();
            options.audio_source = "output".to_string();
        }
        "webcam" => {
            if virtual_media::default_loopback().is_none() {
                return Err(
                    "no v4l2loopback device found; load the module with: sudo modprobe v4l2loopback exclusive_caps=1 video_nr=10 card_label=LinkToLinux"
                        .to_string(),
                );
            }
            options.video = true;
            options.audio = false;
            options.control = true;
            options.video_source = "camera".to_string();
            options.camera_facing = Some("back".to_string());
            options.audio_codec = "raw".to_string();
        }
        "mic" | "studio" => {
            let sdk = adb
                .shell(serial, "getprop ro.build.version.sdk")
                .await
                .unwrap_or_default()
                .trim()
                .parse::<u32>()
                .unwrap_or(0);
            if sdk < 30 {
                return Err("phone microphone capture requires Android 11 or newer".to_string());
            }
            if !virtual_media::binary_available("pw-cat") {
                return Err("pw-cat is required for the virtual microphone".to_string());
            }
            options.audio = true;
            options.audio_source = "mic".to_string();
            options.audio_codec = "opus".to_string();
            if mode == "mic" {
                options.video = false;
                options.control = false;
            } else {
                options.video = true;
                options.control = true;
                options.video_source = "camera".to_string();
                options.audio_source = "mic".to_string();
            }
        }
        other => return Err(format!("unknown capture mode {other}")),
    }

    let session = ScrcpySession::start(adb, serial, &options, &state.data_dir)
        .await
        .map_err(failure)?;
    let device_name = session.device_name.clone();
    let mut session = session;

    if let Some(mut reader) = session.take_control_reader() {
        let app = state.app.clone();
        runtime.control_task = Some(tokio::spawn(async move {
            loop {
                match reader.next_message().await {
                    Ok(Some(DeviceMessage::Clipboard(text))) => {
                        if let Err(error) = clipboard::write_system_clipboard(&text).await {
                            log::debug!("scrcpy clipboard write failed: {error}");
                        }
                        let _ = app.emit("scrcpy:clipboard", json!({ "content": text }));
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(error) => {
                        log::debug!("scrcpy control reader stopped: {error}");
                        break;
                    }
                }
            }
        }));
    }

    let video = session.take_video();
    let audio = session.take_audio();
    let width = video.as_ref().map(|stream| stream.width).unwrap_or(0);
    let height = video.as_ref().map(|stream| stream.height).unwrap_or(0);
    let video_codec = video
        .as_ref()
        .map(|stream| adb_scrcpy::codec_name(stream.codec_id).to_string());
    let audio_codec = audio
        .as_ref()
        .map(|stream| adb_scrcpy::codec_name(stream.codec_id).to_string());
    let fps = if max_fps > 0 {
        max_fps
    } else {
        PREVIEW_FPS_FALLBACK
    };

    state.update_capture(|capture| {
        *capture = CaptureTelemetry {
            active: true,
            mode: mode.to_string(),
            serial: Some(serial.to_string()),
            codec: video_codec.clone(),
            width,
            height,
            fps,
            v4l2_device: None,
            audio_sink: None,
            preview: false,
            started_at: config::unix_seconds(),
            frames: 0,
            dropped: 0,
        };
    });
    state.frames.store(0, Ordering::SeqCst);

    let mut preview_url = None;
    let mut sink_device = None;
    let mut audio_source = None;

    if let Some(mut stream) = video {
        let codec = adb_scrcpy::codec_name(stream.codec_id).to_string();
        let (video_tx, _) = broadcast::channel::<Arc<Vec<u8>>>(24);
        let sender = video_tx.clone();
        let frames = state.frames.clone();
        let capture = state.capture.clone();
        runtime.video_pump = Some(tokio::spawn(async move {
            let mut last_pts = 0u64;
            let mut keyframes = 0u64;
            let mut configs = 0u64;
            loop {
                match stream.next_frame().await {
                    Ok(Some(frame)) => {
                        last_pts = frame.pts;
                        if frame.config {
                            configs = configs.saturating_add(1);
                        }
                        if frame.keyframe {
                            keyframes = keyframes.saturating_add(1);
                        }
                        if !frame.config {
                            frames.fetch_add(1, Ordering::Relaxed);
                            if let Ok(mut capture) = capture.lock() {
                                capture.frames = capture.frames.saturating_add(1);
                            }
                        }
                        let _ = sender.send(Arc::new(frame.data));
                    }
                    Ok(None) => {
                        log::debug!(
                            "scrcpy video stream closed at pts {last_pts} after {keyframes} keyframes and {configs} config packets"
                        );
                        break;
                    }
                    Err(error) => {
                        log::debug!("scrcpy video stream ended: {error}");
                        if let Ok(mut capture) = capture.lock() {
                            capture.dropped = capture.dropped.saturating_add(1);
                        }
                        break;
                    }
                }
            }
        }));

        let ffmpeg = adb_scrcpy::resolve_binary("ffmpeg", "LINKTOLINUX_FFMPEG")
            .ok_or_else(|| "ffmpeg is required for video capture".to_string())?;

        if preview && mode != "webcam" {
            match virtual_media::start_mjpeg_preview(
                &ffmpeg,
                &codec,
                video_tx.subscribe(),
                width,
                height,
                fps,
            ) {
                Ok(server) => {
                    preview_url = Some(server.url());
                    runtime.preview = Some(server);
                }
                Err(error) => log::warn!("preview pipeline unavailable: {error}"),
            }
        }

        if mode == "webcam" || mode == "studio" {
            let device = match video_device {
                Some(device) if device.starts_with("/dev/video") => device,
                _ => virtual_media::default_loopback()
                    .map(|device| device.path)
                    .ok_or_else(virtual_media::loopback_hint)?,
            };
            match virtual_media::start_v4l2_sink(
                &ffmpeg,
                &device,
                &codec,
                video_tx.subscribe(),
                width,
                height,
                fps,
            ) {
                Ok(sink) => {
                    sink_device = Some(device.clone());
                    runtime.sink = Some(sink);
                }
                Err(error) => {
                    if mode == "webcam" {
                        return Err(format!("failed to open {device}: {error}"));
                    }
                    log::warn!("v4l2 sink unavailable: {error}");
                }
            }
        }
    }

    if let Some(mut stream) = audio {
        let codec = adb_scrcpy::codec_name(stream.codec_id).to_string();
        let (audio_tx, _) = broadcast::channel::<Arc<Vec<u8>>>(24);
        let sender = audio_tx.clone();
        runtime.audio_pump = Some(tokio::spawn(async move {
            while let Ok(Some(frame)) = stream.next_frame().await {
                if !frame.is_empty() {
                    let _ = sender.send(Arc::new(frame));
                }
            }
        }));

        if mode == "mic" || mode == "studio" {
            let route = AudioRoute::ensure().await.map_err(failure)?;
            let ffmpeg = adb_scrcpy::resolve_binary("ffmpeg", "LINKTOLINUX_FFMPEG")
                .ok_or_else(|| "ffmpeg is required for microphone capture".to_string())?;
            let channels = 1;
            match virtual_media::start_mic_feed(
                &ffmpeg,
                &route,
                &codec,
                audio_tx.subscribe(),
                channels,
            ) {
                Ok(feed) => {
                    audio_source = Some(route.source_name().to_string());
                    runtime.route = Some(route);
                    runtime.mic = Some(feed);
                }
                Err(error) => {
                    route.teardown().await;
                    return Err(format!("failed to start the virtual microphone: {error}"));
                }
            }
        }
    }

    if mode == "mic" && runtime.mic.is_none() {
        return Err("the phone did not expose a microphone stream".to_string());
    }
    if mode == "webcam" && runtime.sink.is_none() {
        return Err("the phone did not expose a video stream".to_string());
    }

    state.update_capture(|capture| {
        capture.preview = preview_url.is_some();
        capture.v4l2_device = sink_device.clone();
        capture.audio_sink = audio_source.clone();
    });

    let view = SessionView {
        serial: serial.to_string(),
        device_name,
        mode: mode.to_string(),
        video_codec,
        audio_codec,
        width,
        height,
        preview_url,
        v4l2_device: sink_device,
        audio_source,
        frames: 0,
    };
    runtime.session = Some(session);
    Ok(view)
}

async fn stop_capture_locked(state: &AppState, runtime: &mut MediaRuntime) {
    runtime.preview = None;
    runtime.sink = None;
    runtime.mic = None;
    if let Some(task) = runtime.video_pump.take() {
        task.abort();
    }
    if let Some(task) = runtime.audio_pump.take() {
        task.abort();
    }
    if let Some(task) = runtime.control_task.take() {
        task.abort();
    }
    if let Some(session) = runtime.session.take() {
        session.stop().await;
    }
    if let Some(route) = runtime.route.take() {
        route.teardown().await;
    }
    state.update_capture(|capture| {
        *capture = CaptureTelemetry::default();
    });
}

#[tauri::command]
async fn stop_capture(state: State<'_, AppState>) -> Result<(), String> {
    let mut runtime = state.media.lock().await;
    stop_capture_locked(&state, &mut runtime).await;
    Ok(())
}

#[tauri::command]
async fn capture_status(state: State<'_, AppState>) -> Result<CaptureTelemetry, String> {
    let mut telemetry = state.telemetry();
    telemetry.frames = state.frames.load(Ordering::Relaxed);
    Ok(telemetry)
}

fn control_mut(runtime: &mut MediaRuntime) -> Result<&mut ControlChannel, String> {
    runtime
        .session
        .as_mut()
        .ok_or_else(|| "no active phone session".to_string())?
        .control
        .as_mut()
        .ok_or_else(|| "this session has no control channel".to_string())
}

#[tauri::command]
async fn mirror_tap(
    state: State<'_, AppState>,
    x: i32,
    y: i32,
    width: u16,
    height: u16,
) -> Result<(), String> {
    let mut runtime = state.media.lock().await;
    control_mut(&mut runtime)?
        .tap(x, y, width, height)
        .await
        .map_err(failure)
}

#[tauri::command]
async fn mirror_key(state: State<'_, AppState>, keycode: u32) -> Result<(), String> {
    let mut runtime = state.media.lock().await;
    control_mut(&mut runtime)?
        .key(keycode)
        .await
        .map_err(failure)
}

#[tauri::command]
async fn screenshot(state: State<'_, AppState>, serial: String) -> Result<String, String> {
    let adb = Adb::locate().map_err(failure)?;
    let name = format!("screenshot-{}.png", config::unix_seconds());
    let destination = adb_scrcpy::capture_screenshot(
        &adb,
        &serial,
        &state.downloads().join("Screenshots").join(name),
    )
    .await
    .map_err(failure)?;
    Ok(destination.display().to_string())
}

async fn shutdown(app: AppHandle) {
    let state = app.state::<AppState>();
    if state.shutting_down.swap(true, Ordering::SeqCst) {
        return;
    }
    let server = state.lan.lock().await.take();
    if let Some(server) = server {
        if let Err(error) = server.stop().await {
            log::warn!("Companion shutdown failed: {error}");
        }
    }
    let mut media = state.media.lock().await;
    stop_capture_locked(&state, &mut media).await;
    drop(media);
    app.exit(0);
}

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let context = tauri::generate_context!();
    let instance = dirs::data_dir()
        .ok_or_else(|| "Cannot resolve the application data directory".to_string())
        .and_then(|directory| {
            config::claim_instance(&directory.join(&context.config().identifier))
        });
    let instance = match instance {
        Ok(instance) => instance,
        Err(error) => {
            log::error!("Cannot launch LinkToLinux: {error}");
            std::process::exit(1);
        }
    };
    if let Err(error) = tauri::Builder::default()
        .manage(instance)
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let app = window.app_handle().clone();
                tauri::async_runtime::spawn(async move {
                    shutdown(app).await;
                });
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|error| anyhow!("resolving the data directory: {error}"))?;
            std::fs::create_dir_all(&data_dir)
                .with_context(|| format!("creating {}", data_dir.display()))?;

            install_server_jar(&data_dir)?;

            app.manage(AppState::new(handle.clone(), data_dir));

            let signal_handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                #[cfg(unix)]
                if let Ok(mut terminate) =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                {
                    tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
                    shutdown(signal_handle).await;
                    return;
                }
                if tokio::signal::ctrl_c().await.is_ok() {
                    shutdown(signal_handle).await;
                }
            });

            let handle3 = handle.clone();
            tauri::async_runtime::spawn(async move {
                let state = handle3.state::<AppState>();
                let mut lan_guard = state.lan.lock().await;
                if lan_guard.is_none() {
                    let token = match config::pairing_code(&state.data_dir, false) {
                        Ok(token) => token,
                        Err(error) => {
                            log::error!("Cannot initialize desktop pairing: {error}");
                            return;
                        }
                    };
                    let server = Arc::new(lan_stream::LanServer::with_handle(
                        lan_stream::configured_port(),
                        Some(handle3.clone()),
                        token,
                    ));
                    match server.start().await {
                        Ok(status) => {
                            log::info!(
                                "LAN Streaming & APK Server listening on TCP {}",
                                status.port
                            );
                            *lan_guard = Some(server);
                        }
                        Err(err) => {
                            log::warn!(
                                "Could not start LAN server on TCP {}: {}",
                                lan_stream::LAN_PORT,
                                err
                            );
                        }
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            file_transfer::files_choose,
            file_transfer::files_send,
            file_transfer::files_list,
            file_transfer::files_cancel,
            file_transfer::files_open_folder,
            list_devices,
            list_adb_devices,
            adb_pair,
            adb_connect,
            lan_stream::lan_start,
            lan_stream::lan_stop,
            lan_stream::lan_reset_pairing,
            lan_stream::lan_status,
            lan_stream::companion_request,
            lan_stream::companion_save_photo,
            lan_stream::companion_clipboard_sync,
            lan_stream::companion_set_clipboard_sync,
            lan_stream::companion_read_clipboard,
            lan_stream::companion_write_clipboard,
            lan_stream::companion_enable_webcam,
            start_capture,
            stop_capture,
            capture_status,
            mirror_tap,
            mirror_key,
            screenshot,
        ])
        .run(context)
    {
        log::error!("Cannot launch LinkToLinux: {error}");
        std::process::exit(1);
    }
}
