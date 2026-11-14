use std::{path::PathBuf, sync::OnceLock};

static APP_DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

#[cfg(target_os = "android")]
pub(crate) fn set_app_data_dir(path: PathBuf) {
    let _ = APP_DATA_DIR.set(path);
}

pub(crate) fn app_data_dir() -> Option<&'static PathBuf> {
    APP_DATA_DIR.get()
}
