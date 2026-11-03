use std::time::{SystemTime, UNIX_EPOCH};

pub fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}
pub fn unix_seconds() -> i64 {
    unix_millis() / 1000
}

pub fn pairing_code(directory: &std::path::Path, reset: bool) -> Result<String, String> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let path = directory.join("pairing-code");
    if !reset {
        match std::fs::read_to_string(&path) {
            Ok(code) => {
                let code = code.trim();
                if code.len() != 12 || !code.bytes().all(|value| value.is_ascii_hexdigit()) {
                    return Err(
                        "Saved pairing code is invalid. Reset desktop pairing to replace it."
                            .into(),
                    );
                }
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                    .map_err(|error| format!("Cannot protect pairing code: {error}"))?;
                return Ok(code.to_uppercase());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Cannot read pairing code: {error}")),
        }
    }
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("Cannot create application data directory: {error}"))?;
    let code = uuid::Uuid::new_v4().simple().to_string()[..12].to_uppercase();
    let temporary = directory.join(format!(".pairing-{}", uuid::Uuid::new_v4()));
    let write_result = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(code.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)
    })();
    if let Err(error) = write_result {
        if let Err(cleanup) = std::fs::remove_file(&temporary) {
            if cleanup.kind() != std::io::ErrorKind::NotFound {
                log::warn!("Cannot remove temporary pairing file: {cleanup}");
            }
        }
        return Err(format!("Cannot save pairing code: {error}"));
    }
    Ok(code)
}

pub fn load_clipboard_sync(directory: &std::path::Path) -> bool {
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;
    let path = directory.join("clipboard-sync");
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return true,
        Err(error) => {
            log::warn!(
                "Clipboard synchronization disabled: configuration read failed ({:?})",
                error.kind()
            );
            return false;
        }
    };
    if let Err(error) = file.set_permissions(std::fs::Permissions::from_mode(0o600)) {
        log::warn!(
            "Clipboard synchronization disabled: configuration permissions failed ({:?})",
            error.kind()
        );
        return false;
    }
    let mut bytes = Vec::with_capacity(6);
    if let Err(error) = file.take(6).read_to_end(&mut bytes) {
        log::warn!(
            "Clipboard synchronization disabled: configuration read failed ({:?})",
            error.kind()
        );
        return false;
    }
    match bytes.as_slice() {
        b"true" => true,
        b"false" => false,
        _ => {
            log::warn!("Clipboard synchronization disabled: invalid boolean configuration");
            false
        }
    }
}

pub fn save_clipboard_sync(directory: &std::path::Path, enabled: bool) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::create_dir_all(directory)
        .map_err(|_| "Cannot create clipboard configuration directory")?;
    let temporary = directory.join(format!(".clipboard-sync-{}", uuid::Uuid::new_v4()));
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(if enabled { b"true" } else { b"false" })?;
        file.sync_all()?;
        std::fs::rename(&temporary, directory.join("clipboard-sync"))
    })();
    if let Err(error) = result {
        if let Err(cleanup) = std::fs::remove_file(&temporary) {
            if cleanup.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    "Cannot remove temporary clipboard configuration ({:?})",
                    cleanup.kind()
                );
            }
        }
        log::warn!("Cannot save clipboard configuration ({:?})", error.kind());
        return Err("Cannot save clipboard synchronization preference".into());
    }
    Ok(())
}

pub fn claim_instance(
    directory: &std::path::Path,
) -> Result<std::os::unix::net::UnixListener, String> {
    use std::hash::{Hash, Hasher};
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixListener};
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("Cannot create application data directory: {error}"))?;
    let directory = directory
        .canonicalize()
        .map_err(|error| format!("Cannot resolve application data directory: {error}"))?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    directory.hash(&mut hash);
    let address = SocketAddr::from_abstract_name(format!("linktolinux-{:016x}", hash.finish()))
        .map_err(|error| error.to_string())?;
    UnixListener::bind_addr(&address).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AddrInUse {
            "LinkToLinux is already running. Use the existing desktop window.".into()
        } else {
            format!("Cannot protect desktop pairing from concurrent instances: {error}")
        }
    })
}

#[cfg(test)]
mod pairing_tests {
    #[test]
    fn pairing_survives_restart_and_reset_revokes_it() {
        let directory =
            std::env::temp_dir().join(format!("linktolinux-pairing-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let first = super::pairing_code(&directory, false).unwrap();
        assert_eq!(first.len(), 12);
        assert_eq!(first, super::pairing_code(&directory, false).unwrap());
        let reset = super::pairing_code(&directory, true).unwrap();
        assert_ne!(first, reset);
        assert_eq!(reset, super::pairing_code(&directory, false).unwrap());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(directory.join("pairing-code"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[cfg(test)]
mod instance_tests {
    #[test]
    fn only_one_process_can_own_a_desktop_data_directory() {
        let directory =
            std::env::temp_dir().join(format!("linktolinux-instance-{}", uuid::Uuid::new_v4()));
        let first = super::claim_instance(&directory).unwrap();
        assert!(super::claim_instance(&directory).is_err());
        drop(first);
        assert!(super::claim_instance(&directory).is_ok());
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[cfg(test)]
mod clipboard_tests {
    #[test]
    fn clipboard_choice_survives_restart_and_invalid_configuration_fails_closed() {
        use std::os::unix::fs::PermissionsExt;
        let directory =
            std::env::temp_dir().join(format!("linktolinux-clipboard-{}", uuid::Uuid::new_v4()));
        assert!(super::load_clipboard_sync(&directory));
        super::save_clipboard_sync(&directory, false).unwrap();
        assert!(!super::load_clipboard_sync(&directory));
        assert_eq!(
            std::fs::metadata(directory.join("clipboard-sync"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        super::save_clipboard_sync(&directory, true).unwrap();
        assert!(super::load_clipboard_sync(&directory));
        std::fs::write(directory.join("clipboard-sync"), "not-a-boolean").unwrap();
        assert!(!super::load_clipboard_sync(&directory));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
