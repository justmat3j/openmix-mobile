use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[cfg(not(target_os = "android"))]
use std::process::{Command, Stdio};

use directories::ProjectDirs;

use crate::{model::LibraryData, platform::app_data_dir};

const MAX_LIBRARY_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) fn config_path() -> PathBuf {
    if let Some(root) = app_data_dir() {
        return root.join("config").join("library.json");
    }
    ProjectDirs::from("studio", "openmix", "OpenMix")
        .map(|p| p.config_dir().join("library.json"))
        .unwrap_or_else(|| PathBuf::from("library.json"))
}

pub(crate) fn load_library(path: &Path) -> Result<LibraryData, String> {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LibraryData {
                version: 1,
                ..Default::default()
            });
        }
        Err(e) => return Err(e.to_string()),
    };
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if metadata.len() > MAX_LIBRARY_BYTES {
        return Err("Library file is larger than 16 MiB.".into());
    }
    let mut data = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut data).map_err(|e| e.to_string())?;
    serde_json::from_slice(&data).map_err(|e| format!("Library file could not be read: {e}"))
}

pub(crate) fn save_library(path: &Path, data: &LibraryData) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(data).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_LIBRARY_BYTES {
        return Err("Library file exceeds the 16 MiB limit.".into());
    }
    let temp = path.with_extension("json.tmp");
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp).map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    fs::rename(temp, path).map_err(|e| e.to_string())
}

pub(crate) fn read_secret() -> Option<String> {
    read_secret_file()
        .ok()
        .flatten()
        .or_else(|| read_secret_account_nvidia("nvidia"))
}

#[cfg(not(target_os = "android"))]
pub(crate) fn read_secret_account_nvidia(account_nvidia: &str) -> Option<String> {
    Command::new("secret-tool")
        .args([
            "lookup",
            "service",
            "openmix",
            "account_nvidia",
            account_nvidia,
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
}

#[cfg(target_os = "android")]
pub(crate) fn read_secret_account_nvidia(_account_nvidia: &str) -> Option<String> {
    None
}

pub(crate) fn write_secret(value: &str) -> Result<String, String> {
    if value.is_empty() {
        return Err("Paste an OpenRouter API key before saving.".into());
    }
    #[cfg(target_os = "android")]
    {
        write_secret_file(value)?;
        Ok("Saved in the app's private storage.".into())
    }
    #[cfg(not(target_os = "android"))]
    match write_secret_account_nvidia(value, "nvidia", "OpenRouter") {
        Ok(()) => {
            let _ = fs::remove_file(secret_fallback_path());
            Ok("Saved in the system credential store.".into())
        }
        Err(credential_error) => {
            write_secret_file(value).map_err(|file_error| {
                format!("Could not save the key: {credential_error}; local fallback failed: {file_error}")
            })?;
            Ok("Saved locally with owner-only file permissions because the system credential store is unavailable.".into())
        }
    }
}

pub(crate) fn secret_fallback_path() -> PathBuf {
    if let Some(root) = app_data_dir() {
        return root.join("config").join("credentials").join("nvidia.key");
    }
    ProjectDirs::from("studio", "openmix", "OpenMix")
        .map(|dirs| dirs.config_dir().join("credentials").join("nvidia.key"))
        .unwrap_or_else(|| PathBuf::from("nvidia.key"))
}

pub(crate) fn read_secret_file() -> Result<Option<String>, String> {
    match fs::read_to_string(secret_fallback_path()) {
        Ok(value) if !value.trim().is_empty() => Ok(Some(value.trim().to_owned())),
        Ok(_) => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn write_secret_file(value: &str) -> Result<(), String> {
    let path = secret_fallback_path();
    let parent = path
        .parent()
        .ok_or_else(|| "credential path has no parent directory".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }
    let temp = path.with_extension("key.tmp");
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp).map_err(|error| error.to_string())?;
    file.write_all(value.as_bytes())
        .map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    fs::rename(temp, path).map_err(|error| error.to_string())
}

#[cfg(not(target_os = "android"))]
pub(crate) fn write_secret_account_nvidia(
    value: &str,
    account_nvidia: &str,
    label: &str,
) -> Result<(), String> {
    let label = format!("--label=OpenMix {label}");
    let mut child = Command::new("secret-tool")
        .args([
            "store",
            &label,
            "service",
            "openmix",
            "account_nvidia",
            account_nvidia,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    child
        .stdin
        .take()
        .ok_or("could not open credential input")?
        .write_all(value.as_bytes())
        .map_err(|e| e.to_string())?;
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

pub(crate) fn clear_secret() -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        match fs::remove_file(secret_fallback_path()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        let credential_result = clear_secret_account_nvidia("nvidia");
        let file_result = match fs::remove_file(secret_fallback_path()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        };
        match (credential_result, file_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(_), Ok(())) => Err(
                "Local key file cleared, but the system credential store could not be reached."
                    .into(),
            ),
            (Ok(()), Err(error)) => Err(format!(
                "System key cleared, but local key file could not be removed: {error}"
            )),
            (Err(credential_error), Err(file_error)) => Err(format!(
                "Credential store: {credential_error}; local key file: {file_error}"
            )),
        }
    }
}

#[cfg(not(target_os = "android"))]
pub(crate) fn clear_secret_account_nvidia(account_nvidia: &str) -> Result<(), String> {
    let output = Command::new("secret-tool")
        .args([
            "clear",
            "service",
            "openmix",
            "account_nvidia",
            account_nvidia,
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}
