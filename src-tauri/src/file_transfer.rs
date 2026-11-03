use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch, Semaphore};
use tokio::time::timeout;

const MAX_SIZE: u64 = 8 * 1024 * 1024 * 1024;
const CHUNK: usize = 65536;
const IDLE: Duration = Duration::from_secs(30);
const OFFER: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTransferView {
    pub id: String,
    pub device_id: String,
    pub name: String,
    pub direction: String,
    pub size: Option<u64>,
    pub transferred: u64,
    pub status: String,
    pub error: Option<String>,
    pub saved_path: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}
struct Entry {
    view: FileTransferView,
    generation: String,
    token: Option<String>,
    expires: Instant,
    path: Option<PathBuf>,
    signal: watch::Sender<bool>,
}
#[derive(Clone)]
struct Session {
    generation: String,
    tx: mpsc::Sender<Value>,
    supported: bool,
}
#[derive(Default)]
struct State {
    entries: HashMap<String, Entry>,
    sessions: HashMap<String, Session>,
}
pub struct FileTransfers {
    state: Mutex<State>,
    outgoing: Arc<Semaphore>,
    directory: PathBuf,
}

fn valid_name(name: &str) -> bool {
    !name.trim().is_empty()
        && name != "."
        && name != ".."
        && name.len() <= 240
        && !name
            .chars()
            .any(|c| c == '/' || c == '\\' || c.is_ascii_control())
}
fn valid_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|v| v.to_string() == id)
}
fn valid_mime(mime: &str) -> bool {
    let parts: Vec<_> = mime.split('/').collect();
    mime.len() <= 127
        && parts.len() == 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
        })
}
fn io_error(_: std::io::Error) -> String {
    "File I/O failed; check storage and connection".into()
}
fn random_token() -> Result<String, String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| "Cannot create secure transfer ticket")?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn terminal(status: &str) -> bool {
    matches!(status, "complete" | "failed" | "cancelled")
}

impl FileTransfers {
    pub fn new(directory: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State::default()),
            outgoing: Arc::new(Semaphore::new(1)),
            directory,
        })
    }
    pub fn connected(
        &self,
        device: &str,
        generation: &str,
        supported: bool,
        tx: mpsc::Sender<Value>,
    ) {
        let mut state = self.state.lock().unwrap();
        for entry in state
            .entries
            .values_mut()
            .filter(|e| e.view.device_id == device && !terminal(&e.view.status))
        {
            Self::end(entry, "failed", Some("Phone reconnected".into()));
        }
        state.sessions.insert(
            device.into(),
            Session {
                generation: generation.into(),
                tx,
                supported,
            },
        );
    }
    pub fn disconnected(&self, device: &str, generation: &str) {
        let mut state = self.state.lock().unwrap();
        if state
            .sessions
            .get(device)
            .is_some_and(|s| s.generation == generation)
        {
            state.sessions.remove(device);
        }
        for entry in state.entries.values_mut().filter(|e| {
            e.view.device_id == device && e.generation == generation && !terminal(&e.view.status)
        }) {
            Self::end(entry, "failed", Some("Phone disconnected".into()));
        }
    }
    pub fn stop(&self) {
        let mut state = self.state.lock().unwrap();
        state.sessions.clear();
        for entry in state
            .entries
            .values_mut()
            .filter(|e| !terminal(&e.view.status))
        {
            Self::end(entry, "cancelled", None);
        }
    }
    fn end(entry: &mut Entry, status: &str, error: Option<String>) {
        entry.view.status = status.into();
        entry.view.error = error;
        entry.view.finished_at = Some(crate::config::unix_millis());
        entry.token = None;
        let _ = entry.signal.send(true);
    }
    fn finish(&self, id: &str, result: Result<Option<PathBuf>, String>) {
        let mut state = self.state.lock().unwrap();
        if let Some(entry) = state.entries.get_mut(id) {
            if !terminal(&entry.view.status) {
                match result {
                    Ok(path) => {
                        entry.view.saved_path = path.map(|p| p.to_string_lossy().into_owned());
                        Self::end(entry, "complete", None);
                    }
                    Err(error) => Self::end(entry, "failed", Some(error)),
                }
            } else {
                let _ = entry.signal.send(true);
            }
        }
    }
    fn publish_received(&self, view: &FileTransferView, temp: &Path) -> Result<PathBuf, String> {
        let mut state = self.state.lock().unwrap();
        let entry = state
            .entries
            .get_mut(&view.id)
            .ok_or("Transfer no longer exists")?;
        if terminal(&entry.view.status) {
            return Err("Transfer stopped before publication".into());
        }
        let path = publish(temp, &self.directory, &view.name)?;
        entry.view.saved_path = Some(path.to_string_lossy().into_owned());
        entry.view.status = "complete".into();
        entry.view.finished_at = Some(crate::config::unix_millis());
        entry.token = None;
        // Completion notification follows the acknowledgement attempt so it cannot cancel that write.
        Ok(path)
    }
    fn trim(state: &mut State) {
        while state.entries.len() >= 50 {
            let oldest = state
                .entries
                .iter()
                .filter(|(_, e)| terminal(&e.view.status))
                .min_by_key(|(_, e)| e.view.started_at)
                .map(|(id, _)| id.clone());
            if let Some(id) = oldest {
                state.entries.remove(&id);
            } else {
                break;
            }
        }
    }
    pub fn list(&self) -> Vec<FileTransferView> {
        let mut values: Vec<_> = self
            .state
            .lock()
            .unwrap()
            .entries
            .values()
            .map(|e| e.view.clone())
            .collect();
        values.sort_by_key(|e| std::cmp::Reverse(e.started_at));
        values
    }
    pub async fn cancel(&self, id: &str) -> Result<(), String> {
        if !valid_id(id) {
            return Err("Invalid transfer ID".into());
        }
        let tx = {
            let mut state = self.state.lock().unwrap();
            let entry = state.entries.get_mut(id).ok_or("Transfer not found")?;
            if terminal(&entry.view.status) {
                return Ok(());
            }
            let device = entry.view.device_id.clone();
            Self::end(entry, "cancelled", None);
            state.sessions.get(&device).map(|s| s.tx.clone())
        };
        if let Some(tx) = tx {
            let _ = timeout(
                Duration::from_secs(5),
                tx.send(json!({"type":"file_cancel","id":id})),
            )
            .await;
        }
        Ok(())
    }
    pub fn send(
        self: &Arc<Self>,
        device: &str,
        paths: Vec<String>,
    ) -> Result<Vec<FileTransferView>, String> {
        if paths.is_empty() || paths.len() > 16 {
            return Err("Select between 1 and 16 files".into());
        }
        let mut validated = Vec::new();
        for path in paths {
            let path = PathBuf::from(path);
            let meta = std::fs::metadata(&path).map_err(io_error)?;
            let name = path
                .file_name()
                .and_then(|v| v.to_str())
                .filter(|v| valid_name(v))
                .ok_or("Invalid filename")?
                .to_string();
            if !meta.is_file() || meta.len() > MAX_SIZE {
                return Err("Only regular files up to 8 GiB are supported".into());
            }
            validated.push((path, name, meta.len()));
        }
        let mut state = self.state.lock().unwrap();
        let session = state
            .sessions
            .get(device)
            .cloned()
            .ok_or("Phone disconnected")?;
        if !session.supported {
            return Err("Update the phone companion to support file transfers".into());
        }
        if state
            .entries
            .values()
            .filter(|e| !terminal(&e.view.status))
            .count()
            + validated.len()
            > 16
        {
            return Err("Transfer queue is full (16 files)".into());
        }
        let mut views = Vec::new();
        for (path, name, size) in validated {
            Self::trim(&mut state);
            let id = uuid::Uuid::new_v4().to_string();
            let (signal, _) = watch::channel(false);
            let view = FileTransferView {
                id: id.clone(),
                device_id: device.into(),
                name,
                direction: "send".into(),
                size: Some(size),
                transferred: 0,
                status: "queued".into(),
                error: None,
                saved_path: None,
                started_at: crate::config::unix_millis(),
                finished_at: None,
            };
            state.entries.insert(
                id.clone(),
                Entry {
                    view: view.clone(),
                    generation: session.generation.clone(),
                    token: None,
                    expires: Instant::now(),
                    path: Some(path),
                    signal,
                },
            );
            views.push(view);
            let manager = self.clone();
            tokio::spawn(async move {
                manager.offer_outgoing(id).await;
            });
        }
        Ok(views)
    }
    async fn offer_outgoing(self: Arc<Self>, id: String) {
        let mut changed = {
            let state = self.state.lock().unwrap();
            let Some(entry) = state.entries.get(&id) else {
                return;
            };
            if terminal(&entry.view.status) {
                return;
            }
            entry.signal.subscribe()
        };
        let permit = tokio::select! {p=self.outgoing.clone().acquire_owned()=>p, _=changed.changed()=>return};
        let Ok(_permit) = permit else {
            return;
        };
        let offer = {
            let mut state = self.state.lock().unwrap();
            let Some(entry) = state.entries.get_mut(&id) else {
                return;
            };
            if terminal(&entry.view.status) {
                return;
            }
            let token = match random_token() {
                Ok(token) => token,
                Err(error) => {
                    Self::end(entry, "failed", Some(error));
                    return;
                }
            };
            entry.token = Some(token.clone());
            entry.expires = Instant::now() + OFFER;
            entry.view.status = "waiting".into();
            let message = json!({"type":"file_offer","id":id,"name":entry.view.name,"size":entry.view.size,"mime":"application/octet-stream","token":token});
            let device_id = entry.view.device_id.clone();
            state
                .sessions
                .get(&device_id)
                .map(|s| (s.tx.clone(), message))
        };
        if let Some((tx, message)) = offer {
            if !matches!(
                timeout(Duration::from_secs(5), tx.send(message)).await,
                Ok(Ok(()))
            ) {
                self.finish(&id, Err("Phone command queue unavailable".into()));
                return;
            }
        }
        tokio::select! {_=changed.changed()=>{}, _=tokio::time::sleep(OFFER)=>{let waiting=self.state.lock().unwrap().entries.get(&id).is_some_and(|e|e.view.status=="waiting"); if waiting {self.finish(&id,Err("File offer expired".into()));} else {let _=changed.changed().await;} }}
    }
    pub async fn control(
        self: &Arc<Self>,
        device: &str,
        generation: &str,
        message: &Value,
    ) -> Result<(), String> {
        let id = message["id"]
            .as_str()
            .filter(|v| valid_id(v))
            .ok_or("Invalid transfer ID")?;
        let kind = message["type"].as_str().unwrap_or("");
        if kind != "file_offer" {
            let mut state = self.state.lock().unwrap();
            if let Some(entry) = state.entries.get_mut(id).filter(|e| {
                e.view.device_id == device
                    && e.generation == generation
                    && !terminal(&e.view.status)
            }) {
                if matches!(kind, "file_cancel" | "file_reject") {
                    Self::end(
                        entry,
                        if kind == "file_cancel" {
                            "cancelled"
                        } else {
                            "failed"
                        },
                        Some("Phone stopped the transfer".into()),
                    );
                }
            }
            return Ok(());
        }
        let name = message["name"]
            .as_str()
            .filter(|v| valid_name(v))
            .ok_or("Unsafe filename")?;
        let offered_size = message
            .get("size")
            .ok_or("File size is required (or null)")?;
        let size = if offered_size.is_null() {
            None
        } else {
            Some(
                message["size"]
                    .as_u64()
                    .filter(|v| *v <= MAX_SIZE)
                    .ok_or("Invalid file size")?,
            )
        };
        message["mime"]
            .as_str()
            .filter(|v| valid_mime(v))
            .ok_or("Invalid MIME type")?;
        let (tx, token, mut expiry_signal) = {
            let mut state = self.state.lock().unwrap();
            let session = state
                .sessions
                .get(device)
                .filter(|s| s.generation == generation && s.supported)
                .cloned()
                .ok_or("Unsupported session")?;
            if state.entries.contains_key(id)
                || state
                    .entries
                    .values()
                    .filter(|e| !terminal(&e.view.status))
                    .count()
                    >= 16
            {
                return Err("Transfer queue is full or duplicate ID".into());
            }
            Self::trim(&mut state);
            let token = random_token()?;
            let (signal, _) = watch::channel(false);
            state.entries.insert(
                id.into(),
                Entry {
                    view: FileTransferView {
                        id: id.into(),
                        device_id: device.into(),
                        name: name.into(),
                        direction: "receive".into(),
                        size,
                        transferred: 0,
                        status: "waiting".into(),
                        error: None,
                        saved_path: None,
                        started_at: crate::config::unix_millis(),
                        finished_at: None,
                    },
                    generation: generation.into(),
                    token: Some(token.clone()),
                    expires: Instant::now() + OFFER,
                    path: None,
                    signal,
                },
            );
            let expiry_signal = state.entries[id].signal.subscribe();
            (session.tx, token, expiry_signal)
        };
        let accepted = timeout(
            Duration::from_secs(5),
            tx.send(json!({"type":"file_accept","id":id,"token":token})),
        )
        .await;
        if !matches!(accepted, Ok(Ok(()))) {
            self.finish(id, Err("Phone command queue unavailable".into()));
            return Err("Phone command queue unavailable".into());
        }
        let manager = self.clone();
        let id = id.to_string();
        tokio::spawn(async move {
            tokio::select! { _ = tokio::time::sleep(OFFER) => {}, _ = expiry_signal.changed() => return }
            let expired = manager
                .state
                .lock()
                .unwrap()
                .entries
                .get(&id)
                .is_some_and(|e| e.view.status == "waiting");
            if expired {
                manager.finish(&id, Err("File offer expired".into()));
            }
        });
        Ok(())
    }
    pub async fn stream(
        self: &Arc<Self>,
        mut stream: TcpStream,
        hello: Value,
    ) -> Result<(), String> {
        let id = hello["id"]
            .as_str()
            .filter(|v| valid_id(v))
            .ok_or("Invalid transfer ID")?
            .to_string();
        let (view, path, mut signal) = {
            let mut state = self.state.lock().unwrap();
            let entry = state.entries.get(&id).ok_or("Unknown file ticket")?;
            let live = state
                .sessions
                .get(&entry.view.device_id)
                .is_some_and(|s| s.generation == entry.generation);
            if !live
                || entry.view.status != "waiting"
                || entry.expires < Instant::now()
                || hello["token"].as_str() != entry.token.as_deref()
            {
                return Err("Invalid file ticket".into());
            }
            let entry = state.entries.get_mut(&id).unwrap();
            entry.token = None;
            entry.view.status = "transferring".into();
            (
                entry.view.clone(),
                entry.path.clone(),
                entry.signal.subscribe(),
            )
        };
        let result = tokio::select! {
            _ = signal.changed() => return Ok(()),
            result = async {
                timeout(Duration::from_secs(5), write_frame(&mut stream, &json!({"type":"file_ready","id":id}))).await.map_err(|_| "File handshake timed out")??;
                if view.direction == "send" { self.send_data(&mut stream, &view, path.as_deref().ok_or("Source unavailable")?).await.map(|_| None) }
                else { self.receive_data(&mut stream, &view).await.map(Some) }
            } => result
        };
        if result.is_err() {
            let _ = timeout(
                Duration::from_secs(1),
                write_frame(
                    &mut stream,
                    &json!({"type":"file_error","error":"Transfer failed"}),
                ),
            )
            .await;
        }
        self.finish(&id, result);
        Ok(())
    }
    fn progress(&self, id: &str, bytes: u64) {
        if let Some(entry) = self.state.lock().unwrap().entries.get_mut(id) {
            entry.view.transferred = bytes;
        }
    }
    async fn send_data(
        &self,
        stream: &mut TcpStream,
        view: &FileTransferView,
        path: &Path,
    ) -> Result<(), String> {
        use std::os::unix::fs::OpenOptionsExt;
        // A replaced FIFO must not leave a blocking file-open worker behind on cancellation.
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .map_err(io_error)?;
        if !file.metadata().map_err(io_error)?.is_file() {
            return Err("Source is not a regular file".into());
        }
        let mut file = tokio::fs::File::from_std(file);
        let mut hash = Sha256::new();
        let mut buf = vec![0; CHUNK];
        let mut total = 0;
        loop {
            let n = timeout(IDLE, file.read(&mut buf))
                .await
                .map_err(|_| "File read timed out")?
                .map_err(io_error)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > MAX_SIZE || view.size.is_some_and(|s| total > s) {
                return Err("Source size changed".into());
            }
            timeout(IDLE, async {
                stream.write_u32(n as u32).await?;
                stream.write_all(&buf[..n]).await
            })
            .await
            .map_err(|_| "File write timed out")?
            .map_err(io_error)?;
            hash.update(&buf[..n]);
            self.progress(&view.id, total);
        }
        if view.size.is_some_and(|s| s != total) {
            return Err("Source size changed".into());
        }
        let digest = format!("{:x}", hash.finalize());
        timeout(IDLE, stream.write_u32(0))
            .await
            .map_err(|_| "File write timed out")?
            .map_err(io_error)?;
        write_frame(
            stream,
            &json!({"type":"file_end","size":total,"sha256":digest}),
        )
        .await?;
        let ack = read_frame(stream).await?;
        if ack["type"] != "file_complete"
            || ack["size"].as_u64() != Some(total)
            || ack["sha256"].as_str() != Some(&digest)
        {
            return Err("Phone did not confirm file integrity".into());
        }
        Ok(())
    }
    async fn receive_data(
        &self,
        stream: &mut TcpStream,
        view: &FileTransferView,
    ) -> Result<PathBuf, String> {
        tokio::fs::create_dir_all(&self.directory)
            .await
            .map_err(io_error)?;
        let temp = self
            .directory
            .join(format!(".{}.part", uuid::Uuid::new_v4()));
        use std::os::unix::fs::OpenOptionsExt;
        // Creation cannot be cancelled between the blocking open and cleanup ownership.
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
            .map_err(io_error)?;
        let cleanup = PendingFile(temp.clone());
        let mut file = tokio::fs::File::from_std(file);
        let mut hash = Sha256::new();
        let mut total = 0;
        let mut buf = vec![0; CHUNK];
        loop {
            let n = timeout(IDLE, stream.read_u32())
                .await
                .map_err(|_| "File read timed out")?
                .map_err(io_error)? as usize;
            if n == 0 {
                break;
            }
            if n > CHUNK {
                return Err("Invalid file chunk".into());
            }
            total += n as u64;
            if total > MAX_SIZE || view.size.is_some_and(|s| total > s) {
                return Err("File exceeds offered size".into());
            }
            timeout(IDLE, stream.read_exact(&mut buf[..n]))
                .await
                .map_err(|_| "File read timed out")?
                .map_err(io_error)?;
            file.write_all(&buf[..n]).await.map_err(io_error)?;
            hash.update(&buf[..n]);
            self.progress(&view.id, total);
        }
        let end = read_frame(stream).await?;
        let digest = format!("{:x}", hash.finalize());
        if end["type"] != "file_end"
            || end["size"].as_u64() != Some(total)
            || end["sha256"].as_str() != Some(&digest)
            || view.size.is_some_and(|s| s != total)
        {
            return Err("File checksum or size mismatch".into());
        }
        file.sync_all().await.map_err(io_error)?;
        drop(file);
        let final_path = self.publish_received(view, &temp)?;
        drop(cleanup);
        if write_frame(
            stream,
            &json!({"type":"file_complete","size":total,"sha256":digest}),
        )
        .await
        .is_err()
        {
            log::debug!("File receipt published; acknowledgement could not be delivered");
        }
        Ok(final_path)
    }
}
struct PendingFile(PathBuf);
impl Drop for PendingFile {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.0) {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!("Cannot remove unpublished file transfer data");
            }
        }
    }
}
fn publish(temp: &Path, directory: &Path, name: &str) -> Result<PathBuf, String> {
    let original = Path::new(name);
    let stem = original
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or("file");
    let extension = original.extension().and_then(|v| v.to_str());
    for index in 0..10000 {
        let candidate = if index == 0 {
            directory.join(name)
        } else {
            directory.join(match extension {
                Some(ext) => format!("{stem} ({index}).{ext}"),
                None => format!("{stem} ({index})"),
            })
        };
        match std::fs::hard_link(temp, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_error(error)),
        }
    }
    Err("Too many files with the same name".into())
}
async fn write_frame(stream: &mut TcpStream, value: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|_| "Invalid file frame")?;
    if bytes.len() > 4096 {
        return Err("File frame exceeds limit".into());
    }
    timeout(IDLE, async {
        stream.write_u32(bytes.len() as u32).await?;
        stream.write_all(&bytes).await
    })
    .await
    .map_err(|_| "File write timed out")?
    .map_err(io_error)
}
async fn read_frame(stream: &mut TcpStream) -> Result<Value, String> {
    let size = timeout(IDLE, stream.read_u32())
        .await
        .map_err(|_| "File read timed out")?
        .map_err(io_error)? as usize;
    if size == 0 || size > 4096 {
        return Err("Invalid file frame size".into());
    }
    let mut bytes = vec![0; size];
    timeout(IDLE, stream.read_exact(&mut bytes))
        .await
        .map_err(|_| "File read timed out")?
        .map_err(io_error)?;
    serde_json::from_slice(&bytes).map_err(|_| "Invalid file JSON".into())
}

#[tauri::command]
pub async fn files_choose(app: tauri::AppHandle) -> Result<Vec<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        use gtk::prelude::*;
        let chooser = gtk::FileChooserNative::new(
            Some("Send files"),
            None::<&gtk::Window>,
            gtk::FileChooserAction::Open,
            Some("Select"),
            Some("Cancel"),
        );
        chooser.set_select_multiple(true);
        let sender = std::rc::Rc::new(std::cell::RefCell::new(Some(tx)));
        let timeout_source = std::rc::Rc::new(std::cell::RefCell::new(None::<gtk::glib::SourceId>));
        let response_sender = sender.clone();
        let response_timeout_source = timeout_source.clone();
        chooser.connect_response(move |dialog, response| {
            let Some(tx) = response_sender.borrow_mut().take() else {
                return;
            };
            if let Some(source) = response_timeout_source.borrow_mut().take() {
                source.remove();
            }
            let paths = if response == gtk::ResponseType::Accept {
                dialog
                    .filenames()
                    .into_iter()
                    .filter_map(|p| p.to_str().map(str::to_owned))
                    .collect()
            } else {
                Vec::new()
            };
            let _ = tx.send(paths);
            dialog.destroy();
        });
        let timeout_chooser = chooser.clone();
        let expired_timeout_source = timeout_source.clone();
        let source = gtk::glib::timeout_add_local_once(Duration::from_secs(300), move || {
            // The running one-shot source removes itself after this callback.
            let _ = expired_timeout_source.borrow_mut().take();
            let tx = sender.borrow_mut().take();
            if let Some(tx) = tx {
                let _ = tx.send(Vec::new());
                timeout_chooser.destroy();
            }
        });
        *timeout_source.borrow_mut() = Some(source);
        chooser.show();
    })
    .map_err(|_| "Cannot open file chooser")?;
    timeout(Duration::from_secs(305), rx)
        .await
        .map_err(|_| "File chooser timed out")?
        .map_err(|_| "File chooser closed".into())
}
#[tauri::command]
pub async fn files_send(
    state: tauri::State<'_, crate::AppState>,
    device_id: String,
    paths: Vec<String>,
) -> Result<Vec<FileTransferView>, String> {
    state.files.send(&device_id, paths)
}
#[tauri::command]
pub fn files_list(state: tauri::State<'_, crate::AppState>) -> Vec<FileTransferView> {
    state.files.list()
}
#[tauri::command]
pub async fn files_cancel(
    state: tauri::State<'_, crate::AppState>,
    id: String,
) -> Result<(), String> {
    state.files.cancel(&id).await
}
#[tauri::command]
pub async fn files_open_folder(state: tauri::State<'_, crate::AppState>) -> Result<(), String> {
    tokio::fs::create_dir_all(&state.files.directory)
        .await
        .map_err(io_error)?;
    let mut child = tokio::process::Command::new("xdg-open")
        .arg(&state.files.directory)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "Cannot start folder opener")?;
    let status = timeout(Duration::from_secs(10), child.wait())
        .await
        .map_err(|_| "Folder opener timed out")?
        .map_err(io_error)?;
    if status.success() {
        Ok(())
    } else {
        Err("Cannot open downloads folder".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_untrusted_metadata() {
        for name in ["", "..", "../x", "a\\b", "a\n", " "] {
            assert!(!valid_name(name));
        }
        assert!(valid_name("photo.jpg"));
        assert!(!valid_id("ABC"));
        assert!(valid_mime("image/jpeg"));
        assert!(!valid_mime("text/plain\n"));
    }
    #[test]
    fn publication_preserves_existing_file() {
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("image.txt"), b"original").unwrap();
        let temp = dir.join(".part");
        std::fs::write(&temp, b"new").unwrap();
        let target = publish(&temp, &dir, "image.txt").unwrap();
        assert_eq!(std::fs::read(dir.join("image.txt")).unwrap(), b"original");
        assert_eq!(std::fs::read(target).unwrap(), b"new");
        std::fs::remove_dir_all(dir).unwrap();
    }
    async fn setup() -> (Arc<FileTransfers>, mpsc::Receiver<Value>, String, String) {
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let manager = FileTransfers::new(dir);
        let (tx, mut rx) = mpsc::channel(16);
        manager.connected("phone", "generation", true, tx);
        let id = uuid::Uuid::new_v4().to_string();
        manager.control("phone","generation",&json!({"type":"file_offer","id":id,"name":"test.bin","size":3,"mime":"application/octet-stream"})).await.unwrap();
        let token = rx.recv().await.unwrap()["token"]
            .as_str()
            .unwrap()
            .to_string();
        (manager, rx, id, token)
    }
    async fn lane(
        manager: Arc<FileTransfers>,
        id: String,
        token: String,
    ) -> (TcpStream, tokio::task::JoinHandle<Result<(), String>>) {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let job = tokio::spawn(async move {
            manager
                .stream(server, json!({"type":"file_stream","id":id,"token":token}))
                .await
        });
        (client, job)
    }
    #[tokio::test]
    async fn upload_integrity_collision_and_single_use_ticket() {
        let (manager, _rx, id, token) = setup().await;
        let (mut client, job) = lane(manager.clone(), id.clone(), token.clone()).await;
        assert_eq!(read_frame(&mut client).await.unwrap()["type"], "file_ready");
        client.write_u32(3).await.unwrap();
        client.write_all(b"abc").await.unwrap();
        client.write_u32(0).await.unwrap();
        let digest = format!("{:x}", Sha256::digest(b"abc"));
        write_frame(
            &mut client,
            &json!({"type":"file_end","size":3,"sha256":digest}),
        )
        .await
        .unwrap();
        assert_eq!(
            read_frame(&mut client).await.unwrap()["type"],
            "file_complete"
        );
        job.await.unwrap().unwrap();
        assert_eq!(manager.list()[0].status, "complete");
        let (_, replay) = lane(manager.clone(), id, token).await;
        assert!(replay.await.unwrap().is_err());
        assert_eq!(
            std::fs::read(manager.directory.join("test.bin")).unwrap(),
            b"abc"
        );
        std::fs::remove_dir_all(&manager.directory).unwrap();
    }
    #[tokio::test]
    async fn wrong_token_and_cancel_leave_no_partial_data() {
        let (manager, _rx, id, token) = setup().await;
        let (_, wrong) = lane(manager.clone(), id.clone(), "wrong".into()).await;
        assert!(wrong.await.unwrap().is_err());
        assert!(!manager.directory.exists());
        let (mut client, job) = lane(manager.clone(), id.clone(), token).await;
        read_frame(&mut client).await.unwrap();
        client.write_u32(3).await.unwrap();
        client.write_all(b"a").await.unwrap();
        manager.cancel(&id).await.unwrap();
        job.await.unwrap().unwrap();
        assert_eq!(manager.list()[0].status, "cancelled");
        if manager.directory.exists() {
            assert_eq!(std::fs::read_dir(&manager.directory).unwrap().count(), 0);
            std::fs::remove_dir_all(&manager.directory).unwrap();
        }
    }
    #[tokio::test]
    async fn checksum_failure_and_disconnect_remove_pending_files() {
        for disconnect in [false, true] {
            let (manager, _rx, id, token) = setup().await;
            let (mut client, job) = lane(manager.clone(), id, token).await;
            read_frame(&mut client).await.unwrap();
            if disconnect {
                drop(client);
            } else {
                client.write_u32(3).await.unwrap();
                client.write_all(b"abc").await.unwrap();
                client.write_u32(0).await.unwrap();
                write_frame(
                    &mut client,
                    &json!({"type":"file_end","size":3,"sha256":"wrong"}),
                )
                .await
                .unwrap();
            }
            job.await.unwrap().unwrap();
            assert_eq!(manager.list()[0].status, "failed");
            assert_eq!(std::fs::read_dir(&manager.directory).unwrap().count(), 0);
            std::fs::remove_dir_all(&manager.directory).unwrap();
        }
    }
    #[tokio::test]
    async fn expired_and_replaced_session_tickets_are_rejected() {
        let (manager, _rx, id, token) = setup().await;
        manager
            .state
            .lock()
            .unwrap()
            .entries
            .get_mut(&id)
            .unwrap()
            .expires = Instant::now() - Duration::from_secs(1);
        let (_, expired) = lane(manager.clone(), id.clone(), token).await;
        assert!(expired.await.unwrap().is_err());
        let (tx, _rx) = mpsc::channel(16);
        manager.connected("phone", "next-generation", true, tx);
        manager.disconnected("phone", "generation");
        assert!(manager.state.lock().unwrap().sessions.contains_key("phone"));
        assert_eq!(manager.list()[0].status, "failed");
        assert!(!manager.directory.exists());
    }
    #[tokio::test]
    async fn outgoing_waits_for_integrity_ack_and_offers_one_at_a_time() {
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&dir).unwrap();
        let source = dir.join("source.bin");
        std::fs::write(&source, b"abc").unwrap();
        let manager = FileTransfers::new(dir.join("received"));
        let (tx, mut rx) = mpsc::channel(16);
        manager.connected("phone", "g", true, tx);
        let views = manager
            .send(
                "phone",
                vec![
                    source.to_str().unwrap().into(),
                    source.to_str().unwrap().into(),
                ],
            )
            .unwrap();
        assert_eq!(views.len(), 2);
        let offer = rx.recv().await.unwrap();
        assert!(timeout(Duration::from_millis(50), rx.recv()).await.is_err());
        let id = offer["id"].as_str().unwrap().to_string();
        let token = offer["token"].as_str().unwrap().to_string();
        let (mut client, job) = lane(manager.clone(), id.clone(), token).await;
        read_frame(&mut client).await.unwrap();
        assert_eq!(client.read_u32().await.unwrap(), 3);
        let mut bytes = [0; 3];
        client.read_exact(&mut bytes).await.unwrap();
        assert_eq!(&bytes, b"abc");
        assert_eq!(client.read_u32().await.unwrap(), 0);
        let end = read_frame(&mut client).await.unwrap();
        assert_eq!(
            manager.list().iter().find(|v| v.id == id).unwrap().status,
            "transferring"
        );
        assert!(timeout(Duration::from_millis(50), rx.recv()).await.is_err());
        write_frame(
            &mut client,
            &json!({"type":"file_complete","size":3,"sha256":end["sha256"]}),
        )
        .await
        .unwrap();
        job.await.unwrap().unwrap();
        assert_eq!(
            manager.list().iter().find(|v| v.id == id).unwrap().status,
            "complete"
        );
        let next = timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(next["type"], "file_offer");
        assert_ne!(next["id"], id);
        manager.stop();
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn receiver_publication_survives_acknowledgement_write_failure() {
        let (manager, _rx, id, _token) = setup().await;
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (mut server, _) = listener.accept().await.unwrap();
        server.shutdown().await.unwrap();
        let receiver = manager.clone();
        let transfer_id = id.clone();
        let job = tokio::spawn(async move {
            let view = receiver
                .list()
                .into_iter()
                .find(|v| v.id == transfer_id)
                .unwrap();
            let result = receiver.receive_data(&mut server, &view).await.map(Some);
            receiver.finish(&transfer_id, result);
        });
        client.write_u32(3).await.unwrap();
        client.write_all(b"abc").await.unwrap();
        client.write_u32(0).await.unwrap();
        write_frame(
            &mut client,
            &json!({"type":"file_end","size":3,"sha256":format!("{:x}",Sha256::digest(b"abc"))}),
        )
        .await
        .unwrap();
        job.await.unwrap();
        let view = manager.list().into_iter().find(|v| v.id == id).unwrap();
        assert_eq!(
            std::fs::read(manager.directory.join("test.bin")).unwrap(),
            b"abc"
        );
        assert_eq!(view.status, "complete");
        assert_eq!(
            view.saved_path.as_deref(),
            manager.directory.join("test.bin").to_str()
        );
        std::fs::remove_dir_all(&manager.directory).unwrap();
    }
}
