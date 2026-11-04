use anyhow::{bail, Context, Result};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    fn output(code: i32, stdout: &[u8], stderr: &[u8]) -> std::process::Output {
        std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
        }
    }

    #[test]
    fn empty_or_non_text_clipboard_is_not_a_missing_dependency() {
        assert_eq!(clipboard_text(output(1, b"", b"No selection")).unwrap(), "");
        assert_eq!(
            clipboard_text(output(1, b"", b"Nothing is copied")).unwrap(),
            ""
        );
        assert_eq!(
            clipboard_text(output(
                1,
                b"",
                b"Clipboard content is not available as requested type text"
            ))
            .unwrap(),
            ""
        );
        assert_eq!(
            clipboard_text(output(1, b"", b"No suitable type of content copied")).unwrap(),
            ""
        );
        assert!(clipboard_text(output(1, b"", b"Failed to connect to a Wayland server")).is_err());
    }

    #[test]
    #[ignore = "requires a graphical desktop clipboard session"]
    fn desktop_clipboard_session_is_readable() {
        gtk::init().expect("GTK desktop session must be available");
        let (sender, mut receiver) = tokio::sync::oneshot::channel();
        request_native_text(sender);
        let context = gtk::glib::MainContext::default();
        let deadline = std::time::Instant::now() + IPC_TIMEOUT;
        loop {
            while context.pending() {
                context.iteration(false);
            }
            if receiver.try_recv().is_ok() {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Native desktop clipboard request timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn clipboard_keeps_text_and_rejects_invalid_utf8() {
        assert_eq!(
            clipboard_text(output(0, b"line one\nline two", b"")).unwrap(),
            "line one\nline two"
        );
        assert!(clipboard_text(output(0, &[255], b"")).is_err());
    }
}

const IPC_TIMEOUT: Duration = Duration::from_secs(3);

fn clipboard_text(output: std::process::Output) -> Result<String> {
    if output.status.success() {
        return String::from_utf8(output.stdout).context("Clipboard text is not valid UTF-8");
    }
    let reason = String::from_utf8_lossy(&output.stderr).to_lowercase();
    if reason.contains("nothing is copied")
        || reason.contains("clipboard content is not available as requested type")
        || reason.contains("no selection")
        || reason.contains("no suitable type")
        || reason.contains("target string not available")
        || reason.contains("target utf8_string not available")
    {
        return Ok(String::new());
    }
    bail!("Cannot access text in the desktop clipboard. Check that LinkToLinux is running in your graphical desktop session.")
}

fn request_native_text(sender: tokio::sync::oneshot::Sender<String>) {
    gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD).request_text(move |_, text| {
        let _ = sender.send(text.unwrap_or_default().to_string());
    });
}

pub async fn read_clipboard(app: &tauri::AppHandle) -> Result<String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    if app
        .run_on_main_thread(move || request_native_text(sender))
        .is_ok()
    {
        if let Ok(Ok(text)) = timeout(IPC_TIMEOUT, receiver).await {
            return Ok(text);
        }
    }
    read_system_clipboard().await
}

pub async fn write_clipboard(app: &tauri::AppHandle, text: &str) -> Result<()> {
    let value = text.to_string();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    if app
        .run_on_main_thread(move || {
            gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD).set_text(&value);
            let _ = sender.send(());
        })
        .is_ok()
        && matches!(timeout(IPC_TIMEOUT, receiver).await, Ok(Ok(())))
    {
        return Ok(());
    }
    write_system_clipboard(text).await
}

pub async fn read_system_clipboard() -> Result<String> {
    let mut found = false;
    for (program, args) in [
        ("wl-paste", vec!["--no-newline", "--type", "text"]),
        ("xclip", vec!["-selection", "clipboard", "-o"]),
    ] {
        let mut command = Command::new(program);
        command.args(args).kill_on_drop(true);
        match timeout(IPC_TIMEOUT, command.output()).await {
            Ok(Ok(output)) => {
                found = true;
                if let Ok(text) = clipboard_text(output) {
                    return Ok(text);
                }
            }
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Ok(Err(_)) | Err(_) => found = true,
        }
    }
    if found {
        bail!("Cannot access the desktop clipboard session. Reopen LinkToLinux from your applications menu and copy some text.");
    }
    bail!("Clipboard tools are missing. Install wl-clipboard on Wayland or xclip on X11.")
}

pub async fn write_system_clipboard(content: &str) -> Result<()> {
    for (program, args) in [
        ("wl-copy", vec!["--type", "text/plain"]),
        ("xclip", vec!["-selection", "clipboard"]),
    ] {
        let mut child = match Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(child) => child,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).context("Starting system clipboard tool"),
        };
        let operation = async {
            let mut stdin = child
                .stdin
                .take()
                .context("Clipboard tool stdin unavailable")?;
            stdin.write_all(content.as_bytes()).await?;
            stdin.shutdown().await?;
            drop(stdin);
            let status = child.wait().await?;
            Ok::<bool, anyhow::Error>(status.success())
        };
        match timeout(IPC_TIMEOUT, operation).await {
            Ok(Ok(true)) => return Ok(()),
            Ok(Ok(false)) | Ok(Err(_)) | Err(_) => {
                let _ = child.kill().await;
            }
        }
    }
    bail!("No working clipboard tool. Install wl-clipboard on Wayland or xclip on X11.")
}
