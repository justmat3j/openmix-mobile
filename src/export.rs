use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use directories::ProjectDirs;

use crate::{audio, platform::app_data_dir};

/// Where the rendered "mix all" WAV goes. Kept outside the binary's own
/// directory (which may not be writable) and named for the render's start
/// time so a new mix never clobbers one still being listened to.
pub(crate) fn mix_output_path() -> PathBuf {
    mix_cache_dir().join(format!("mix-{}.wav", unique_mix_timestamp()))
}

/// Where a generated two-track transition is stored so it can be exported
/// later without regenerating the preview.
pub(crate) fn mix_preview_output_path() -> PathBuf {
    mix_cache_dir().join(format!("mix-preview-{}.wav", unique_mix_timestamp()))
}

pub(crate) fn mix_cache_dir() -> PathBuf {
    if let Some(root) = app_data_dir() {
        return root.join("cache").join("mixes");
    }
    ProjectDirs::from("studio", "openmix", "OpenMix")
        .map(|dirs| dirs.cache_dir().join("mixes"))
        .unwrap_or_else(|| PathBuf::from("mixes"))
}

pub(crate) fn unique_mix_timestamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub(crate) fn write_pcm_to_wav(pcm: &audio::StereoPcm, path: &Path) -> Result<Duration, String> {
    let mut writer = audio::MixWriter::create(path)?;
    writer.write_frames(&pcm.samples)?;
    writer.finish()
}

pub(crate) fn set_mp4_extension(path: &mut PathBuf) {
    path.set_extension("mp4");
}

/// Converts a rendered mix WAV into an audio-only MP4 (AAC) using ffmpeg.
/// ffmpeg is used rather than a bundled encoder so the app stays small; if
/// it's missing we surface a clear error instead of silently failing.
pub(crate) fn export_wav_to_mp4(source: &Path, dest: &Path) -> Result<(), String> {
    if !source.is_file() {
        return Err("The rendered mix is no longer on disk.".into());
    }
    let output = Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            &source.to_string_lossy(),
            "-c:a",
            "aac",
            "-b:a",
            "256k",
            "-movflags",
            "+faststart",
            &dest.to_string_lossy(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Could not run ffmpeg (install it to export MP4): {e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "ffmpeg failed: {}",
            err.chars().take(400).collect::<String>()
        ));
    }
    if !dest.is_file() {
        return Err("ffmpeg finished but no output file was written.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_destination_is_always_mp4() {
        let mut destination = PathBuf::from("OpenMix-mix.wav");
        set_mp4_extension(&mut destination);
        assert_eq!(destination, PathBuf::from("OpenMix-mix.mp4"));

        let mut destination = PathBuf::from("OpenMix-mix");
        set_mp4_extension(&mut destination);
        assert_eq!(destination, PathBuf::from("OpenMix-mix.mp4"));
    }
}
