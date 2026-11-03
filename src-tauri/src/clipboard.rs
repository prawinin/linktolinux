use anyhow::{bail, Context, Result};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

const IPC_TIMEOUT: Duration = Duration::from_secs(3);

pub async fn read_system_clipboard() -> Option<String> {
    for (program, args) in [
        ("wl-paste", vec!["--no-newline"]),
        ("xclip", vec!["-selection", "clipboard", "-o"]),
    ] {
        let mut command = Command::new(program);
        command.args(args).kill_on_drop(true);
        match timeout(IPC_TIMEOUT, command.output()).await {
            Ok(Ok(output)) if output.status.success() => {
                return String::from_utf8(output.stdout).ok()
            }
            Ok(Ok(_)) | Ok(Err(_)) | Err(_) => continue,
        }
    }
    None
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
