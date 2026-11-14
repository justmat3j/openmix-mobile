use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::{audio, theme::AppTheme};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Track {
    pub(crate) id: String,
    pub(crate) path: PathBuf,
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) album: String,
    #[serde(default)]
    pub(crate) release_year: Option<String>,
    #[serde(default)]
    pub(crate) musicbrainz_id: Option<String>,
    #[serde(default)]
    pub(crate) cover_art_path: Option<PathBuf>,
    pub(crate) duration: u32,
    pub(crate) bpm: u16,
    pub(crate) energy: f32,
    pub(crate) rise: u32,
    pub(crate) added: u64,
    pub(crate) last_played: u64,
    pub(crate) liked: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct RemoteMetadata {
    pub(crate) release_year: Option<String>,
    pub(crate) album: Option<String>,
    pub(crate) musicbrainz_id: String,
    pub(crate) cover_art_path: Option<PathBuf>,
}

#[derive(Clone, Debug)]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) struct DeviceAudio {
    pub(crate) id: i64,
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) album: String,
    pub(crate) duration_ms: i64,
    pub(crate) mime_type: String,
    pub(crate) display_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Playlist {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) tracks: Vec<String>,
    pub(crate) created: u64,
}

pub(crate) type DecodedTransition = (
    audio::StereoPcm,
    audio::StereoPcm,
    audio::TransitionPlan,
    String,
    bool,
    u16,
    u16,
);

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct LibraryData {
    pub(crate) version: u32,
    pub(crate) tracks: Vec<Track>,
    pub(crate) playlists: Vec<Playlist>,
    pub(crate) recent: Vec<String>,
    #[serde(default)]
    pub(crate) user_name: String,
    #[serde(default)]
    pub(crate) user_icon: String,
    #[serde(default)]
    pub(crate) theme: AppTheme,
    #[serde(default)]
    pub(crate) use_openrouter: bool,
    #[serde(default)]
    pub(crate) use_musicbrainz: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Page {
    Listen,
    Mixing,
    Library,
    Liked,
    Playlist,
    Queue,
    Settings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SortMode {
    RecentlyAdded,
    PlaylistOrder,
    Title,
    Artist,
    Duration,
    Bpm,
}

impl SortMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::RecentlyAdded => "Recently added",
            Self::PlaylistOrder => "Playlist order",
            Self::Title => "Title",
            Self::Artist => "Artist",
            Self::Duration => "Duration",
            Self::Bpm => "Tempo",
        }
    }
}

/// What the "Add to playlist" menu in the selection toolbar should do after
/// the user picks an entry. Kept as a small local enum so the menu closure
/// only has to stash one value while it still borrows `self`.
pub(crate) enum BulkPlaylistAction {
    New,
    Existing(String),
}

pub(crate) enum VolumeAction {
    Set(f32),
    ToggleMute,
}

pub(crate) enum WorkerMessage {
    Imported(Result<(Vec<Track>, Vec<String>), String>),
    #[cfg(target_os = "android")]
    DeviceMusicLoaded(Result<Vec<DeviceAudio>, String>),
    #[cfg(target_os = "android")]
    AndroidTrackImported(Result<Track, String>),
    MetadataFetched {
        track_id: String,
        result: Result<Option<RemoteMetadata>, String>,
    },
    MetadataFetchFinished {
        total: usize,
        matched: usize,
        misses: usize,
        unmatched: Vec<String>,
        errors: Vec<String>,
    },
    PlaylistMixReady {
        first_id: String,
        result: Result<(PathBuf, Duration), String>,
    },
    TransitionDecoded {
        outgoing_id: String,
        incoming_id: String,
        result: Result<DecodedTransition, String>,
    },
    TransitionReady {
        outgoing_id: String,
        incoming_id: String,
        result: Result<(audio::StereoPcm, String, bool, Duration, Option<PathBuf>), String>,
    },
    OpenRouterTest(Result<(), String>),
    MixExportReady(Result<String, String>),
}

pub(crate) struct Toast {
    pub(crate) title: String,
    pub(crate) message: String,
    pub(crate) error: bool,
    pub(crate) expires_at: Instant,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_library_files_default_to_obsidian() {
        let data: LibraryData =
            serde_json::from_str(r#"{"version":1,"tracks":[],"playlists":[],"recent":[]}"#)
                .expect("library data should deserialize");

        assert_eq!(data.theme, AppTheme::Obsidian);
        assert!(data.user_name.is_empty());
        assert!(data.user_icon.is_empty());
    }
}
