#[cfg(target_os = "android")]
mod android;
mod audio;
mod export;
mod metadata;
mod model;
mod planning;
mod platform;
mod storage;
mod theme;
mod ui;

#[cfg(not(target_os = "android"))]
use std::sync::Arc;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    fs::{self, File},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use directories::ProjectDirs;

#[cfg(target_os = "android")]
use android_activity::AndroidApp;
use eframe::{
    egui,
    egui::{Align, Color32, CornerRadius, Frame, Layout, Margin, RichText, Stroke, Vec2},
};
use export::*;
use metadata::*;
use model::*;
use planning::*;
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Source, buffer::SamplesBuffer};
use serde::{Deserialize, Serialize};
use storage::*;
use theme::*;
use ui::*;

// "Mix all" streams the render straight to a WAV file on disk (see
// audio::MixWriter) instead of holding the whole thing in memory, so these
// caps are just sanity limits rather than memory guards: the track count
// keeps a single render from taking forever, and the duration cap keeps it
// from filling the disk.
const MIX_ALL_MAX_TRACKS: usize = 60;
const MIX_ALL_MAX_DURATION: Duration = Duration::from_secs(24 * 60 * 60);

thread_local! {
    static COVER_TEXTURES: RefCell<HashMap<PathBuf, CoverArtCache>> = RefCell::new(HashMap::new());
}

/// A missing or undecodable cover file is remembered for a while so the UI
/// does not re-read the disk on every repaint frame; it is retried only
/// occasionally in case the file reappears after a metadata refresh.
const COVER_RETRY_INTERVAL: Duration = Duration::from_secs(60);

enum CoverArtCache {
    Texture(egui::TextureHandle),
    Failed(Instant),
}

struct AudioEngine {
    stream: Option<MixerDeviceSink>,
    player: Option<Player>,
    duration: Duration,
    error: Option<String>,
    volume: f32,
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self {
            stream: None,
            player: None,
            duration: Duration::ZERO,
            error: None,
            volume: 0.8,
        }
    }
}

impl AudioEngine {
    fn ensure_stream(&mut self) -> Result<(), String> {
        if self.stream.is_none() {
            self.stream = Some(
                DeviceSinkBuilder::open_default_sink()
                    .map_err(|e| format!("No audio output is available: {e}"))?,
            );
        }
        Ok(())
    }

    fn new_player(&mut self) -> Result<Player, String> {
        self.ensure_stream()?;
        let player = Player::connect_new(self.stream.as_ref().unwrap().mixer());
        player.set_volume(self.volume);
        Ok(player)
    }

    fn play_file(&mut self, path: &Path, known_duration: u32) -> Result<Duration, String> {
        let file = File::open(path).map_err(|e| format!("Could not open this track: {e}"))?;
        let decoder =
            Decoder::try_from(file).map_err(|e| format!("Could not decode this track: {e}"))?;
        let duration = decoder
            .total_duration()
            .unwrap_or(Duration::from_secs(known_duration as u64));
        if let Some(old) = self.player.take() {
            old.stop();
        }
        let player = self.new_player()?;
        player.append(decoder);
        player.play();
        self.duration = duration;
        self.player = Some(player);
        self.error = None;
        Ok(duration)
    }

    /// Plays a "mix all" render streamed from disk (see `mix_all`), rather
    /// than loading the whole rendered mix into memory the way
    /// `play_pcm_from` does for the short two-track preview.
    fn play_mix_file(&mut self, path: &Path, duration: Duration) -> Result<(), String> {
        let file = File::open(path).map_err(|e| format!("Could not open the mix: {e}"))?;
        let decoder =
            Decoder::try_from(file).map_err(|e| format!("Could not decode the mix: {e}"))?;
        if let Some(old) = self.player.take() {
            old.stop();
        }
        let player = self.new_player()?;
        player.append(decoder);
        player.play();
        self.duration = duration;
        self.player = Some(player);
        self.error = None;
        Ok(())
    }

    fn play_pcm_from(&mut self, pcm: audio::StereoPcm, position: Duration) -> Result<(), String> {
        if let Some(old) = self.player.take() {
            old.stop();
        }
        let player = self.new_player()?;
        let channels = std::num::NonZeroU16::new(2).unwrap();
        let rate = std::num::NonZeroU32::new(audio::OUTPUT_RATE).unwrap();
        player.append(SamplesBuffer::new(channels, rate, pcm.samples));
        player.pause();
        self.duration = pcm.duration;
        self.player = Some(player);
        if position > Duration::ZERO {
            self.player
                .as_ref()
                .unwrap()
                .try_seek(position.min(self.duration))
                .map_err(|e| format!("Could not align the prepared mix: {e}"))?;
        }
        self.player.as_ref().unwrap().play();
        self.error = None;
        Ok(())
    }

    fn toggle(&mut self) -> Result<(), String> {
        let player = self
            .player
            .as_ref()
            .ok_or_else(|| "Choose a track to start listening.".to_owned())?;
        if player.is_paused() {
            player.play();
        } else {
            player.pause();
        }
        Ok(())
    }

    fn position(&self) -> Duration {
        self.player
            .as_ref()
            .map(Player::get_pos)
            .unwrap_or_default()
    }
    fn is_playing(&self) -> bool {
        self.player
            .as_ref()
            .is_some_and(|p| !p.is_paused() && !p.empty())
    }
    fn ended(&self) -> bool {
        self.player.as_ref().is_some_and(Player::empty)
    }
    fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        if let Some(player) = &self.player {
            player.set_volume(self.volume);
        }
    }
    fn seek(&mut self, position: Duration) -> Result<(), String> {
        let player = self
            .player
            .as_ref()
            .ok_or_else(|| "Choose a track before seeking.".to_owned())?;
        player
            .try_seek(position.min(self.duration))
            .map_err(|e| format!("Could not seek: {e}"))
    }
}

struct OpenMix {
    data: LibraryData,
    page: Page,
    active_playlist: Option<String>,
    search: String,
    sort: SortMode,
    sort_ascending: bool,
    queue: Vec<String>,
    current: Option<String>,
    shuffle: bool,
    repeat: u8,
    now_open: bool,
    /// Full-screen player modal (Spotify-style): shows big cover art, timeline, controls
    fullscreen_player_open: bool,
    search_focus: bool,
    status: String,
    toast: Option<Toast>,
    key_input: String,
    key_status: String,
    key_visible: bool,
    animated_page: Page,
    page_animation_target: bool,
    notes: HashMap<String, (String, bool)>,
    playlist_dialog: bool,
    playlist_edit: Option<String>,
    playlist_name: String,
    playlist_delete: Option<String>,
    profile_open: bool,
    selected_tracks: HashSet<String>,
    audio: AudioEngine,
    muted: bool,
    volume_before_mute: f32,
    config_file: PathBuf,
    tx: Sender<WorkerMessage>,
    rx: Receiver<WorkerMessage>,
    /// Path to the most recently generated mix source on disk. When this is
    /// set, the player bar exposes an "Export mix" button.
    last_mix_path: Option<PathBuf>,
    #[cfg(target_os = "android")]
    android: Option<android::AndroidBridge>,
    #[cfg(target_os = "android")]
    device_music: Vec<DeviceAudio>,
    #[cfg(target_os = "android")]
    device_music_open: bool,
    #[cfg(target_os = "android")]
    device_music_loading: bool,
    #[cfg(target_os = "android")]
    android_permission_pending: bool,
    #[cfg(target_os = "android")]
    pending_android_export: Option<PathBuf>,
    /// Folder picker state: URI of the picked folder and whether we're waiting for result
    #[cfg(target_os = "android")]
    folder_picker_pending: bool,
    #[cfg(target_os = "android")]
    picked_folder_uri: Option<String>,
    /// System bar / display-cutout padding so the UI renders above the
    /// Android status bar and the 3-button/gesture navigation bar.
    #[cfg(target_os = "android")]
    android_insets: egui::epaint::MarginF32,
    #[cfg(target_os = "android")]
    android_insets_next_poll: Instant,
    /// Left drawer (Android only): hidden until the user swipes in from the
    /// left edge (or taps the menu button), with a sliding animation.
    #[cfg(target_os = "android")]
    drawer_open: bool,
    #[cfg(target_os = "android")]
    drawer_drag: Option<DrawerDrag>,
    #[cfg(target_os = "android")]
    drawer_offset: f32,
    /// On-screen keyboard focus is requested only for the first frame of a
    /// sheet dialog. Re-requesting every frame made the keyboard open/close
    /// in a loop on Android.
    #[cfg(target_os = "android")]
    sheet_focus_pending: bool,
}

/// An in-progress left-drawer drag gesture (far edge swipe in, or drag the
/// scrim to pull the drawer back out).
#[cfg(target_os = "android")]
struct DrawerDrag {
    pointer_start_x: f32,
    offset_at_start: f32,
    started_on_scrim: bool,
    moved: bool,
}

impl OpenMix {
    fn new(
        cc: &eframe::CreationContext<'_>,
        #[cfg(target_os = "android")] android_bridge: Option<android::AndroidBridge>,
    ) -> Self {
        #[cfg(target_os = "android")]
        if let Some(bridge) = android_bridge.as_ref()
            && let Ok(files_dir) = bridge.files_dir()
        {
            platform::set_app_data_dir(files_dir);
        }
        let config_file = config_path();
        let data = load_library(&config_file).unwrap_or_default();
        set_theme(&cc.egui_ctx, data.theme);
        let key = read_secret()
            .or_else(|| std::env::var("NVIDIA_API_KEY").ok())
            .unwrap_or_default();
        let (tx, rx) = mpsc::channel();
        Self {
            data,
            page: Page::Listen,
            active_playlist: None,
            search: String::new(),
            sort: SortMode::RecentlyAdded,
            sort_ascending: false,
            queue: Vec::new(),
            current: None,
            shuffle: false,
            repeat: 0,
            now_open: !cfg!(target_os = "android"),
            fullscreen_player_open: false,
            search_focus: false,
            status: String::new(),
            toast: None,
            key_input: key,
            key_status: {
                #[cfg(target_os = "android")]
                {
                    "The API key is kept in this app's private storage (NVIDIA NIM)."
                }
                #[cfg(not(target_os = "android"))]
                {
                    "The API key is kept in your system credential store when available (NVIDIA NIM)."
                }
            }
            .into(),
            key_visible: false,
            animated_page: Page::Listen,
            page_animation_target: true,
            notes: HashMap::new(),
            playlist_dialog: false,
            playlist_edit: None,
            playlist_name: String::new(),
            playlist_delete: None,
            profile_open: false,
            selected_tracks: HashSet::new(),
            audio: AudioEngine::default(),
            muted: false,
            volume_before_mute: 0.8,
            config_file,
            tx,
            rx,
            last_mix_path: None,
            #[cfg(target_os = "android")]
            android: android_bridge,
            #[cfg(target_os = "android")]
            device_music: Vec::new(),
            #[cfg(target_os = "android")]
            device_music_open: false,
            #[cfg(target_os = "android")]
            device_music_loading: false,
            #[cfg(target_os = "android")]
            android_permission_pending: false,
            #[cfg(target_os = "android")]
            pending_android_export: None,
            #[cfg(target_os = "android")]
            folder_picker_pending: false,
            #[cfg(target_os = "android")]
            picked_folder_uri: None,
            #[cfg(target_os = "android")]
            android_insets: egui::epaint::MarginF32::default(),
            #[cfg(target_os = "android")]
            android_insets_next_poll: Instant::now(),
            #[cfg(target_os = "android")]
            drawer_open: false,
            #[cfg(target_os = "android")]
            drawer_drag: None,
            #[cfg(target_os = "android")]
            drawer_offset: 0.0,
            #[cfg(target_os = "android")]
            sheet_focus_pending: true,
        }
    }

    fn persist(&mut self) {
        match save_library(&self.config_file, &self.data) {
            Ok(()) => {}
            Err(error) => self.status = format!("Could not save library: {error}"),
        }
    }

    fn change_theme(&mut self, ctx: &egui::Context, theme: AppTheme) {
        if self.data.theme == theme {
            return;
        }
        self.data.theme = theme;
        set_theme(ctx, theme);
        self.persist();
        self.notify(
            "Theme updated",
            format!("{} is now active.", theme.label()),
            false,
        );
    }

    fn notify(&mut self, title: impl Into<String>, message: impl Into<String>, error: bool) {
        self.toast = Some(Toast {
            title: title.into(),
            message: message.into(),
            error,
            expires_at: Instant::now() + Duration::from_secs(5),
        });
    }

    fn lookup_metadata(&mut self, tracks: Vec<Track>) {
        if tracks.is_empty() {
            self.notify("MusicBrainz", "There are no tracks to look up.", false);
            return;
        }
        self.notify(
            "MusicBrainz",
            format!("Looking up metadata for {} track(s)…", tracks.len()),
            false,
        );
        let tx = self.tx.clone();
        thread::spawn(move || {
            let total = tracks.len();
            let mut matched = 0;
            let mut misses = 0;
            let mut unmatched = Vec::new();
            let mut errors = Vec::new();
            for track in tracks {
                let result = musicbrainz_metadata(&track);
                match &result {
                    Ok(Some(_)) => matched += 1,
                    Ok(None) => {
                        misses += 1;
                        unmatched.push(format!("{} — {}", track.title, track.artist));
                    }
                    Err(error) => errors.push(format!("{}: {error}", track.title)),
                }
                let _ = tx.send(WorkerMessage::MetadataFetched {
                    track_id: track.id,
                    result,
                });
            }
            let _ = tx.send(WorkerMessage::MetadataFetchFinished {
                total,
                matched,
                misses,
                unmatched,
                errors,
            });
        });
    }

    #[cfg(not(target_os = "android"))]
    fn import_files(&mut self) {
        let Some(files) = rfd::FileDialog::new()
            .add_filter("Audio", &["mp3", "wav"])
            .pick_files()
        else {
            return;
        };
        if files.is_empty() {
            return;
        }
        self.status = format!("Analyzing {} file(s)…", files.len());
        let tx = self.tx.clone();
        thread::spawn(move || {
            let mut imported = Vec::new();
            let mut problems = Vec::new();
            for path in files {
                match audio::analyze_file(&path) {
                    Ok(analysis) => {
                        let filename = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("Local file");
                        let (filename_artist, filename_title) = filename
                            .split_once(" - ")
                            .map(|(artist, title)| {
                                (artist.trim().to_owned(), title.trim().to_owned())
                            })
                            .unwrap_or_else(|| ("Local file".into(), filename.to_owned()));
                        let artist = analysis.artist.unwrap_or(filename_artist);
                        let title = analysis.title.unwrap_or(filename_title);
                        let path_string = path.to_string_lossy().into_owned();
                        imported.push(Track {
                            id: path_string.clone(),
                            path,
                            title,
                            artist,
                            album: analysis.album.unwrap_or_else(|| "Local library".into()),
                            release_year: None,
                            musicbrainz_id: None,
                            cover_art_path: None,
                            duration: analysis.duration_seconds,
                            bpm: analysis.bpm,
                            energy: analysis.energy,
                            rise: analysis.rise_seconds,
                            added: now_seconds(),
                            last_played: 0,
                            liked: false,
                        });
                    }
                    Err(error) => problems.push(format!(
                        "{}: {error}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    )),
                }
            }
            let result = if imported.is_empty() {
                Err(problems.join("\n"))
            } else {
                Ok((imported, problems))
            };
            let _ = tx.send(WorkerMessage::Imported(result));
        });
    }

    #[cfg(target_os = "android")]
    fn import_files(&mut self) {
        let Some(bridge) = self.android.as_ref() else {
            self.status = "Android device access is unavailable.".into();
            return;
        };

        // Check if we have media permission first
        if !bridge.has_media_permission() {
            if let Err(error) = bridge.request_media_permission() {
                self.status = format!("Could not request music access: {error}");
                self.notify("Music access", error, true);
            } else {
                self.android_permission_pending = true;
                self.status =
                    "Allow music access, then OpenMix will let you pick a music folder.".into();
            }
            return;
        }

        // Launch folder picker
        if let Err(error) = bridge.pick_folder() {
            self.status = format!("Could not open folder picker: {error}");
            self.notify("Folder picker", error, true);
        } else {
            self.folder_picker_pending = true;
            self.status = "Select a folder containing your music files…".into();
        }
    }

    #[cfg(target_os = "android")]
    fn load_device_music(&mut self) {
        let Some(bridge) = self.android.clone() else {
            return;
        };
        self.device_music_loading = true;
        self.status = "Reading music from this device…".into();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = bridge.query_device_audio();
            let _ = tx.send(WorkerMessage::DeviceMusicLoaded(result));
        });
    }

    #[cfg(target_os = "android")]
    fn import_device_track(&mut self, item: DeviceAudio) {
        let Some(bridge) = self.android.clone() else {
            return;
        };
        self.status = format!("Importing {}…", item.title);
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = (|| -> Result<Track, String> {
                let path = bridge.import_audio(&item)?;
                let analysis = audio::analyze_file(&path)
                    .map_err(|error| format!("Could not analyze {}: {error}", item.title))?;
                Ok(Track {
                    id: format!("android-media:{}", item.id),
                    path,
                    title: analysis.title.unwrap_or(item.title),
                    artist: analysis.artist.unwrap_or(item.artist),
                    album: analysis.album.unwrap_or(item.album),
                    release_year: None,
                    musicbrainz_id: None,
                    cover_art_path: None,
                    duration: if analysis.duration_seconds > 0 {
                        analysis.duration_seconds
                    } else {
                        (item.duration_ms / 1000).max(1) as u32
                    },
                    bpm: analysis.bpm,
                    energy: analysis.energy,
                    rise: analysis.rise_seconds,
                    added: now_seconds(),
                    last_played: 0,
                    liked: false,
                })
            })();
            let _ = tx.send(WorkerMessage::AndroidTrackImported(result));
        });
    }

    #[cfg(target_os = "android")]
    fn poll_android_permissions(&mut self) {
        let Some(bridge) = self.android.clone() else {
            return;
        };
        if self.android_permission_pending && bridge.has_media_permission() {
            self.android_permission_pending = false;
            self.load_device_music();
        }
        if self.pending_android_export.is_some() && bridge.has_write_permission() {
            if let Some(source) = self.pending_android_export.take() {
                self.export_android_mix(source);
            }
        }
    }

    fn add_to_queue(&mut self, id: &str) {
        if self.data.tracks.iter().any(|t| t.id == id) {
            self.queue.push(id.to_owned());
            self.status = format!("Added to queue · {} up next", self.queue.len());
        }
    }

    fn play_id(&mut self, id: &str) {
        let Some(track) = self.data.tracks.iter().find(|t| t.id == id).cloned() else {
            return;
        };
        match self.audio.play_file(&track.path, track.duration) {
            Ok(_) => {
                self.current = Some(track.id.clone());
                self.data
                    .tracks
                    .iter_mut()
                    .find(|t| t.id == track.id)
                    .unwrap()
                    .last_played = now_seconds();
                self.data.recent.retain(|item| item != &track.id);
                self.data.recent.insert(0, track.id);
                self.data.recent.truncate(50);
                self.persist();
                self.status.clear();
            }
            Err(error) => {
                self.audio.error = Some(error.clone());
                self.status = error;
            }
        }
    }

    fn toggle(&mut self) {
        if self.current.is_none() {
            if let Some(next) = self.queue.first().cloned() {
                self.queue.remove(0);
                self.play_id(&next);
            } else if let Some(first) = self.data.tracks.first().map(|t| t.id.clone()) {
                self.play_id(&first);
            }
            return;
        }
        if let Err(error) = self.audio.toggle() {
            self.status = error;
        }
    }

    /// The list of track IDs the transport controls operate over, mirroring
    /// what [`Self::next`] uses: the active playlist when one is open,
    /// otherwise the currently visible tracks.
    fn transport_ids(&self) -> Vec<String> {
        if let Some(playlist_id) = self.active_playlist.as_deref()
            && let Some(playlist) = self.data.playlists.iter().find(|p| p.id == playlist_id)
        {
            return playlist.tracks.clone();
        }
        self.visible_tracks()
            .into_iter()
            .map(|t| t.id.clone())
            .collect()
    }

    fn next(&mut self) {
        if self.repeat == 1 {
            if let Some(current) = self.current.clone() {
                self.play_id(&current);
            }
            return;
        }
        if !self.queue.is_empty() {
            let next = self.queue.remove(0);
            self.play_id(&next);
            return;
        }
        if self.data.tracks.is_empty() {
            return;
        }
        let ids = self.transport_ids();
        if ids.is_empty() {
            return;
        }
        let index = self
            .current
            .as_ref()
            .and_then(|id| ids.iter().position(|item| item == id))
            .unwrap_or(0);
        let next = if self.shuffle && ids.len() > 1 {
            let mut n = (random_u64() as usize) % ids.len();
            if ids[n] == self.current.clone().unwrap_or_default() {
                n = (n + 1) % ids.len();
            }
            n
        } else if index + 1 < ids.len() {
            index + 1
        } else if self.repeat == 2 {
            0
        } else {
            return;
        };
        self.play_id(&ids[next]);
    }

    fn previous(&mut self) {
        if self.audio.position() > Duration::from_secs(3) {
            let _ = self.audio.seek(Duration::ZERO);
            return;
        }
        if self.data.tracks.is_empty() {
            return;
        }
        let ids = self.transport_ids();
        if ids.is_empty() {
            return;
        }
        let index = self
            .current
            .as_ref()
            .and_then(|id| ids.iter().position(|item| item == id))
            .unwrap_or(0);
        if index > 0 {
            self.play_id(&ids[index - 1]);
        }
    }

    fn current_track(&self) -> Option<Track> {
        self.current
            .as_ref()
            .and_then(|id| self.data.tracks.iter().find(|t| &t.id == id))
            .cloned()
    }

    fn visible_tracks(&self) -> Vec<Track> {
        let mut tracks: Vec<Track> = match self.page {
            Page::Liked => self
                .data
                .tracks
                .iter()
                .filter(|t| t.liked)
                .cloned()
                .collect(),
            Page::Playlist => {
                let ids = self
                    .data
                    .playlists
                    .iter()
                    .find(|p| Some(p.id.as_str()) == self.active_playlist.as_deref())
                    .map(|p| p.tracks.as_slice())
                    .unwrap_or(&[]);
                ids.iter()
                    .filter_map(|id| {
                        self.data
                            .tracks
                            .iter()
                            .find(|track| &track.id == id)
                            .cloned()
                    })
                    .collect()
            }
            _ => self.data.tracks.clone(),
        };
        let query = self.search.trim().to_lowercase();
        if !query.is_empty() {
            tracks.retain(|t| {
                format!("{} {} {} {}", t.title, t.artist, t.album, t.path.display())
                    .to_lowercase()
                    .contains(&query)
            });
        }
        match self.sort {
            SortMode::RecentlyAdded => tracks.sort_by_key(|t| t.added),
            SortMode::PlaylistOrder => {}
            SortMode::Title => tracks.sort_by_cached_key(|track| track.title.to_lowercase()),
            SortMode::Artist => tracks.sort_by(|a, b| {
                a.artist
                    .to_lowercase()
                    .cmp(&b.artist.to_lowercase())
                    .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))
            }),
            SortMode::Duration => tracks.sort_by_key(|t| t.duration),
            SortMode::Bpm => tracks.sort_by_key(|t| t.bpm),
        }
        if !self.sort_ascending {
            tracks.reverse();
        }
        tracks
    }

    fn queue_transition(&mut self, outgoing: Track, incoming: Track) {
        let outgoing_position = if self.current.as_deref() == Some(outgoing.id.as_str()) {
            self.audio.position().as_secs().min(u64::from(u32::MAX)) as u32
        } else {
            0
        };
        let key = self.key_input.trim().to_owned();
        let use_openrouter = self.data.use_openrouter && !key.is_empty();
        self.status = if key.is_empty() {
            "Preparing a local mix…".into()
        } else if !self.data.use_openrouter {
            "Preparing a local mix… (AI mixing is off)".into()
        } else {
            "Planning your mix…".into()
        };
        let tx = self.tx.clone();
        thread::spawn(move || {
            let fallback_plan = local_plan_at(&outgoing, &incoming, outgoing_position);
            let (plan, rationale, used_local_plan) = if !use_openrouter {
                (fallback_plan, local_note(&outgoing, &incoming), true)
            } else {
                match openrouter_plan(&key, &outgoing, &incoming, outgoing_position) {
                    Ok((plan, rationale)) => (plan, rationale, false),
                    Err(error) => (fallback_plan, format!("Local plan used. {error}"), true),
                }
            };
            let rationale = format!(
                "{} {}s lead, {}s fade, incoming at {}s.",
                rationale, plan.entry_lead_seconds, plan.fade_seconds, plan.incoming_start_seconds,
            );
            let result = audio::decode_stereo(&outgoing.path).and_then(|a| {
                audio::decode_stereo(&incoming.path).map(|b| {
                    (
                        a,
                        b,
                        plan,
                        rationale,
                        used_local_plan,
                        outgoing.bpm,
                        incoming.bpm,
                    )
                })
            });
            let _ = tx.send(WorkerMessage::TransitionDecoded {
                outgoing_id: outgoing.id,
                incoming_id: incoming.id,
                result,
            });
        });
    }

    fn mix_all(&mut self, tracks: Vec<Track>) {
        if tracks.len() < 2 {
            if let Some(track) = tracks.first() {
                self.play_id(&track.id);
            }
            return;
        }
        if tracks.len() > MIX_ALL_MAX_TRACKS {
            self.status = format!(
                "Mix all is limited to {MIX_ALL_MAX_TRACKS} tracks per render to keep memory use predictable. This selection has {}.",
                tracks.len()
            );
            return;
        }
        self.status = format!("Preparing a continuous mix of {} tracks…", tracks.len());
        let key = self.key_input.trim().to_owned();
        let use_openrouter = self.data.use_openrouter && !key.is_empty();
        let first_id = tracks[0].id.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            #[allow(clippy::redundant_closure_call)] // IIFE keeps the early returns readable
            let result = (|| {
                let track_ids: Vec<String> = tracks.iter().map(|t| t.id.clone()).collect();
                let output_path = mix_output_path();
                let mut mixinfo = MixInfo {
                    status: "in_progress".into(),
                    total_tracks: tracks.len(),
                    completed_tracks: 1,
                    track_ids,
                    output_path: output_path.clone(),
                    updated_at: now_seconds(),
                    error: None,
                };
                write_mixinfo(&mixinfo);
                // Render on its own so any early failure (first-track decode,
                // WAV creation) still lands in the mixinfo error state below.
                let rendered = (|| -> Result<Duration, String> {
                    let mut writer = audio::MixWriter::create(&output_path)?;
                    let mut tail = audio::decode_stereo(&tracks[0].path)?;
                    let mut elapsed = Duration::from_secs(u64::from(tracks[0].duration));
                    for pair in tracks.windows(2) {
                        let (from, to) = (&pair[0], &pair[1]);
                        elapsed =
                            elapsed.saturating_add(Duration::from_secs(u64::from(to.duration)));
                        if elapsed > MIX_ALL_MAX_DURATION {
                            return Err(format!(
                                "Mix all is limited to {} hours of combined audio per render.",
                                MIX_ALL_MAX_DURATION.as_secs() / 3600
                            ));
                        }
                        let incoming = audio::decode_stereo(&to.path)?;
                        let plan = if !use_openrouter {
                            local_plan(from, to)
                        } else {
                            openrouter_plan(&key, from, to, 0)
                                .map(|(plan, _)| plan)
                                .unwrap_or_else(|_| local_plan(from, to))
                        };
                        tail = audio::transition_stream(
                            &mut writer,
                            &tail,
                            &incoming,
                            from.bpm,
                            to.bpm,
                            &plan,
                        )?;
                        mixinfo.completed_tracks += 1;
                        mixinfo.updated_at = now_seconds();
                        write_mixinfo(&mixinfo);
                    }
                    writer.write_frames(&tail.samples)?;
                    writer.finish()
                })();
                let outcome: Result<Duration, String> = match rendered {
                    Ok(duration) => {
                        mixinfo.status = "done".into();
                        Ok(duration)
                    }
                    Err(error) => {
                        mixinfo.status = "error".into();
                        mixinfo.error = Some(error.clone());
                        Err(error)
                    }
                };
                mixinfo.updated_at = now_seconds();
                write_mixinfo(&mixinfo);
                outcome.map(|duration| (output_path, duration))
            })();
            let _ = tx.send(WorkerMessage::PlaylistMixReady { first_id, result });
        });
    }

    /// Asks the user where to save the rendered mix as MP4 and kicks off the
    /// ffmpeg transcode on a background thread. The button that calls this
    /// only appears when `last_mix_path` is `Some`, so the guard is mostly
    /// defensive.
    fn export_source(&mut self) -> Option<PathBuf> {
        let Some(source) = self.last_mix_path.clone() else {
            self.notify(
                "No mix to export",
                "Create a mix first with Mix all or a playlist.",
                true,
            );
            return None;
        };
        if !source.is_file() {
            self.notify(
                "Mix file missing",
                "The rendered mix is no longer on disk. Mix again to recreate it.",
                true,
            );
            return None;
        }
        Some(source)
    }

    #[cfg(not(target_os = "android"))]
    fn export_mix(&mut self) {
        let Some(source) = self.export_source() else {
            return;
        };
        let timestamp = now_seconds();
        let default_name = format!("OpenMix-{timestamp}.mp4");
        let Some(mut output_path) = rfd::FileDialog::new()
            .set_title("Save mix as MP4")
            .set_file_name(&default_name)
            .add_filter("MP4 audio", &["mp4"])
            .save_file()
        else {
            return;
        };
        set_mp4_extension(&mut output_path);
        self.status = "Exporting mix to MP4…".into();
        let tx = self.tx.clone();
        let dest = output_path.clone();
        thread::spawn(move || {
            let result =
                export_wav_to_mp4(&source, &dest).map(|_| dest.to_string_lossy().into_owned());
            let _ = tx.send(WorkerMessage::MixExportReady(result));
        });
    }

    #[cfg(target_os = "android")]
    fn export_mix(&mut self) {
        let Some(source) = self.export_source() else {
            return;
        };
        let Some(bridge) = self.android.as_ref() else {
            self.status = "Android device access is unavailable.".into();
            return;
        };
        if !bridge.has_write_permission() {
            self.pending_android_export = Some(source);
            if let Err(error) = bridge.request_write_permission() {
                self.status = format!("Could not request storage access: {error}");
            } else {
                self.status = "Allow storage access to save the finished mix.".into();
            }
            return;
        }
        self.export_android_mix(source);
    }

    #[cfg(target_os = "android")]
    fn export_android_mix(&mut self, source: PathBuf) {
        let Some(bridge) = self.android.clone() else {
            return;
        };
        let display_name = format!("OpenMix-{}.wav", now_seconds());
        self.status = "Saving the finished mix…".into();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = bridge.export_wav(&source, &display_name);
            let _ = tx.send(WorkerMessage::MixExportReady(result));
        });
    }

    fn openrouter_test(&mut self) {
        let key = self.key_input.trim().to_owned();
        if key.is_empty() {
            self.key_status = "Enter a key first.".into();
            return;
        }
        self.key_status = "Checking NVIDIA NIM".into();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|e| e.to_string())
                .and_then(|client| {
                    match send_completion(
                        &client,
                        &key,
                        vec![
                            json_message("system", "Reply with a short connection confirmation."),
                            json_message("user", "Confirm the OpenMix connection."),
                        ],
                        2048,
                        true,
                    ) {
                        Ok(_) => Ok(()),
                        Err(error) if error == "NVIDIA NIM returned an empty completion." => Ok(()),
                        Err(error) => Err(error),
                    }
                });
            let _ = tx.send(WorkerMessage::OpenRouterTest(result));
        });
    }

    fn receive_messages(&mut self) {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                WorkerMessage::Imported(Ok((tracks, problems))) => {
                    let mut added = 0;
                    let mut lookup = Vec::new();
                    for track in tracks {
                        if !self.data.tracks.iter().any(|old| old.id == track.id) {
                            lookup.push(track.clone());
                            self.data.tracks.push(track);
                            added += 1;
                        }
                    }
                    self.persist();
                    self.status = if problems.is_empty() {
                        format!("Added {added} track(s) to your library.")
                    } else {
                        format!(
                            "Added {added} track(s); skipped {} file(s). {}",
                            problems.len(),
                            problems.join(" · ")
                        )
                    };
                    if !lookup.is_empty() && self.data.use_musicbrainz {
                        self.lookup_metadata(lookup);
                    }
                }
                WorkerMessage::Imported(Err(error)) => self.status = error,
                #[cfg(target_os = "android")]
                WorkerMessage::DeviceMusicLoaded(Ok(tracks)) => {
                    self.device_music_loading = false;
                    self.device_music = tracks;
                    self.status = if self.device_music.is_empty() {
                        "No MP3 or WAV files were found on this device.".to_owned()
                    } else {
                        format!("{} device track(s) available.", self.device_music.len())
                    };
                }
                #[cfg(target_os = "android")]
                WorkerMessage::DeviceMusicLoaded(Err(error)) => {
                    self.device_music_loading = false;
                    self.status = format!("Could not read device music: {error}");
                    self.notify("Device music", error, true);
                }
                #[cfg(target_os = "android")]
                WorkerMessage::AndroidTrackImported(Ok(track)) => {
                    let imported_id = track
                        .id
                        .trim_start_matches("android-media:")
                        .parse()
                        .unwrap_or(-1);
                    let imported_title = track.title.clone();
                    self.device_music.retain(|item| item.id != imported_id);
                    if !self.data.tracks.iter().any(|old| old.id == track.id) {
                        let lookup = self.data.use_musicbrainz.then(|| track.clone());
                        self.data.tracks.push(track);
                        self.persist();
                        self.status = format!("Added {imported_title} to your library.");
                        if let Some(track) = lookup {
                            self.lookup_metadata(vec![track]);
                        }
                    } else {
                        self.status = "That track is already in your library.".to_owned();
                    }
                }
                #[cfg(target_os = "android")]
                WorkerMessage::AndroidTrackImported(Err(error)) => {
                    self.status = error.clone();
                    self.notify("Import failed", error, true);
                }
                WorkerMessage::MetadataFetched { track_id, result } => {
                    if let Ok(Some(metadata)) = result
                        && let Some(track) = self
                            .data
                            .tracks
                            .iter_mut()
                            .find(|track| track.id == track_id)
                    {
                        track.release_year = metadata.release_year;
                        if track.album == "Local library"
                            && let Some(album) = metadata.album
                        {
                            track.album = album;
                        }
                        track.musicbrainz_id = Some(metadata.musicbrainz_id);
                        if metadata.cover_art_path.is_some() {
                            track.cover_art_path = metadata.cover_art_path;
                        }
                        self.persist();
                    }
                }
                WorkerMessage::MetadataFetchFinished {
                    total,
                    matched,
                    misses,
                    unmatched,
                    errors,
                } => {
                    if let Some(first_error) = errors.first() {
                        let detail = if errors.len() > 1 {
                            format!("{first_error} (and {} more)", errors.len() - 1)
                        } else {
                            first_error.clone()
                        };
                        self.notify(
                            "MusicBrainz lookup failed",
                            format!("{detail} · {matched}/{total} matched"),
                            true,
                        );
                    } else if matched == 0 {
                        let first = unmatched
                            .first()
                            .map(|track| format!(" First: {track}."))
                            .unwrap_or_default();
                        self.notify(
                            "MusicBrainz lookup complete",
                            format!("No catalog match for {total} track(s).{first}"),
                            false,
                        );
                    } else {
                        self.notify(
                            "MusicBrainz lookup complete",
                            format!("Updated {matched}/{total} track(s); {misses} unmatched."),
                            false,
                        );
                    }
                }
                WorkerMessage::PlaylistMixReady { first_id, result } => match result {
                    Ok((path, duration)) => match self.audio.play_mix_file(&path, duration) {
                        Ok(()) => {
                            self.last_mix_path = Some(path.clone());
                            self.current = Some(first_id.clone());
                            self.queue.clear();
                            self.data.recent.retain(|id| id != &first_id);
                            self.data.recent.insert(0, first_id.clone());
                            self.data.recent.truncate(50);
                            if let Some(track) =
                                self.data.tracks.iter_mut().find(|t| t.id == first_id)
                            {
                                track.last_played = now_seconds();
                            }
                            self.persist();
                            self.status.clear();
                        }
                        Err(error) => self.status = error,
                    },
                    Err(error) => self.status = format!("Could not mix playlist: {error}"),
                },
                WorkerMessage::TransitionDecoded {
                    outgoing_id,
                    incoming_id,
                    result: Ok((outgoing, incoming, plan, rationale, local, from_bpm, to_bpm)),
                } => {
                    if self.current.as_deref().is_some_and(|id| id != outgoing_id) {
                        self.status = "Mix cancelled because the playing track changed.".into();
                        continue;
                    }
                    if !self.data.tracks.iter().any(|track| track.id == incoming_id) {
                        self.status =
                            "Mix cancelled because the incoming track was removed.".into();
                        continue;
                    }
                    let position = if self.current.as_deref() == Some(outgoing_id.as_str()) {
                        self.audio.position()
                    } else {
                        Duration::ZERO
                    };
                    let mut plan = plan;
                    if position > Duration::ZERO
                        && let (Some(from), Some(to)) = (
                            self.data
                                .tracks
                                .iter()
                                .find(|track| track.id == outgoing_id),
                            self.data
                                .tracks
                                .iter()
                                .find(|track| track.id == incoming_id),
                        )
                    {
                        let seconds = position.as_secs().min(u64::from(u32::MAX)) as u32;
                        if to.rise > 0 {
                            let cue = aligned_incoming_start(
                                from,
                                to,
                                seconds,
                                plan.entry_lead_seconds,
                                plan.fade_seconds,
                            );
                            plan.incoming_start_seconds = plan
                                .incoming_start_seconds
                                .clamp(cue.saturating_sub(8), cue.saturating_add(8));
                        }
                    }
                    let starting_position = position;
                    let tx = self.tx.clone();
                    thread::spawn(move || {
                        let tail =
                            audio::trim_start(&outgoing, position).unwrap_or(audio::StereoPcm {
                                samples: Vec::new(),
                                duration: Duration::ZERO,
                            });
                        let pcm = audio::transition(&tail, &incoming, from_bpm, to_bpm, &plan);
                        let export_path = mix_preview_output_path();
                        let export_path = write_pcm_to_wav(&pcm, &export_path)
                            .ok()
                            .map(|_| export_path);
                        let result = Ok((pcm, rationale, local, starting_position, export_path));
                        let _ = tx.send(WorkerMessage::TransitionReady {
                            outgoing_id,
                            incoming_id,
                            result,
                        });
                    });
                }
                WorkerMessage::TransitionDecoded {
                    result: Err(error), ..
                } => self.status = error,
                WorkerMessage::TransitionReady {
                    outgoing_id,
                    incoming_id,
                    result: Ok((pcm, rationale, local, starting_position, export_path)),
                } => {
                    if self.current.as_deref().is_some_and(|id| id != outgoing_id) {
                        self.status = "Mix cancelled because the playing track changed.".into();
                        continue;
                    }
                    if !self.data.tracks.iter().any(|track| track.id == incoming_id) {
                        self.status =
                            "Mix cancelled because the incoming track was removed.".into();
                        continue;
                    }
                    let position = self.audio.position().saturating_sub(starting_position);
                    match self.audio.play_pcm_from(pcm, position) {
                        Ok(()) => {
                            if let Some(export_path) = export_path {
                                self.last_mix_path = Some(export_path);
                            }
                            self.notes.insert(incoming_id.clone(), (rationale, local));
                            self.current = Some(incoming_id.clone());
                            self.data.recent.retain(|id| id != &incoming_id);
                            self.data.recent.insert(0, incoming_id.clone());
                            self.data.recent.truncate(50);
                            if let Some(track) = self
                                .data
                                .tracks
                                .iter_mut()
                                .find(|track| track.id == incoming_id)
                            {
                                track.last_played = now_seconds();
                            }
                            self.persist();
                            self.status.clear();
                        }
                        Err(error) => self.status = error,
                    }
                }
                WorkerMessage::TransitionReady {
                    result: Err(error), ..
                } => self.status = error,
                WorkerMessage::OpenRouterTest(Ok(())) => {
                    self.key_status = "Connected. The check sent no track information.".into()
                }
                WorkerMessage::OpenRouterTest(Err(error)) => {
                    self.key_status = format!("Connection failed: {error}")
                }
                WorkerMessage::MixExportReady(Ok(location)) => {
                    self.notify("Export complete", format!("Saved {location}"), false);
                    self.status = format!("Exported {location}");
                }
                WorkerMessage::MixExportReady(Err(error)) => {
                    self.notify("Export failed", error.clone(), true);
                    self.status = error;
                }
            }
        }
    }

    fn update_shortcuts(&mut self, ctx: &egui::Context) {
        // Don't hijack keys while the user is typing in a text field
        // (search, playlist name, API key): Space would otherwise toggle
        // playback mid-typing.
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (space, ctrl_k, ctrl_f, right, left, ctrl_s, ctrl_r) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Space) && !i.modifiers.ctrl && !i.modifiers.command,
                i.key_pressed(egui::Key::K) && i.modifiers.command,
                i.key_pressed(egui::Key::F) && i.modifiers.command,
                i.key_pressed(egui::Key::ArrowRight) && i.modifiers.alt,
                i.key_pressed(egui::Key::ArrowLeft) && i.modifiers.alt,
                i.key_pressed(egui::Key::S) && i.modifiers.command,
                i.key_pressed(egui::Key::R) && i.modifiers.command,
            )
        });
        if space {
            self.toggle();
        }
        if ctrl_k || ctrl_f {
            self.search_focus = true;
            self.page = Page::Library;
        }
        if right {
            self.next();
        }
        if left {
            self.previous();
        }
        if ctrl_s {
            self.shuffle = !self.shuffle;
        }
        if ctrl_r {
            self.repeat = (self.repeat + 1) % 3;
        }
    }

    #[cfg(not(target_os = "android"))]
    fn left_sidebar(&mut self, root: &mut egui::Ui) {
        egui::Panel::left("library_sidebar")
            .resizable(false)
            .default_size(244.0)
            .frame(
                Frame::new()
                    .fill(theme_sidebar())
                    .inner_margin(Margin::symmetric(14, 20)),
            )
            .show(root, |ui| {
                self.sidebar_contents(ui);
            });
    }

    /// The navigation structure shared by the desktop left panel and the
    /// Android slide-in drawer: brand, playlists, and bottom navigation.
    fn sidebar_contents(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            brand_mark(ui);
            ui.label(
                RichText::new("openmix")
                    .size(21.0)
                    .strong()
                    .color(theme_text()),
            );
        });
        #[cfg(target_os = "android")]
        {
            ui.add_space(14.0);
            Frame::new()
                .fill(theme_surface())
                .stroke(Stroke::new(1.0, theme_edge()))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if profile_avatar(ui, self.profile_icon_key(), 40.0, false).clicked() {
                            self.profile_open = true;
                            self.drawer_open = false;
                        }
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(self.profile_name())
                                    .size(14.0)
                                    .strong()
                                    .color(theme_text()),
                            );
                            ui.label(
                                RichText::new("Tap the avatar to edit your profile")
                                    .size(10.5)
                                    .color(theme_quiet()),
                            );
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if icon_button(ui, Icon::Close, "Close menu").clicked() {
                                self.drawer_open = false;
                            }
                        });
                    });
                });
            ui.add_space(10.0);
        }
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("YOUR LIBRARY")
                    .size(10.0)
                    .strong()
                    .color(theme_quiet()),
            );
            #[cfg(target_os = "android")]
            if icon_button(ui, Icon::Add, "Import device music").clicked() {
                self.import_files();
            }
        });
        ui.add_space(7.0);

        let playlists = self.data.playlists.clone();
        let playlist_area_height = (ui.available_height() - 360.0).max(72.0);
        egui::ScrollArea::vertical()
            .id_salt("playlist_nav")
            .max_height(playlist_area_height)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                if playlists.is_empty() {
                    ui.label(
                        RichText::new("Create your first playlist in Your library.")
                            .size(11.5)
                            .color(theme_quiet()),
                    );
                } else {
                    for playlist in playlists {
                        let selected = self.active_playlist.as_deref()
                            == Some(playlist.id.as_str())
                            && self.page == Page::Playlist;
                        if nav_row(ui, Icon::Playlist, &playlist.name, selected).clicked() {
                            self.active_playlist = Some(playlist.id);
                            self.page = Page::Playlist;
                            self.sort = SortMode::PlaylistOrder;
                            self.sort_ascending = true;
                        }
                    }
                }
            });

        ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
            ui.label(
                RichText::new(format!("{} local tracks", self.data.tracks.len()))
                    .size(11.0)
                    .color(theme_quiet()),
            );
            if !self.status.is_empty() {
                ui.label(RichText::new(&self.status).size(11.0).color(theme_soft()));
            }
            ui.add_space(4.0);
            if nav_row(ui, Icon::Settings, "Settings", self.page == Page::Settings).clicked() {
                self.page = Page::Settings;
            }
            #[cfg(not(target_os = "android"))]
            {
                let profile_name = self.profile_name();
                if nav_row(ui, Icon::User, &profile_name, false).clicked() {
                    self.profile_open = true;
                }
            }
            ui.separator();
            self.nav_button(ui, Icon::Queue, "Queue", Page::Queue);
            self.nav_button(ui, Icon::Liked, "Liked tracks", Page::Liked);
            self.nav_button(ui, Icon::Library, "Your library", Page::Library);
            self.nav_button(ui, Icon::Mix, "Mixing", Page::Mixing);
            self.nav_button(ui, Icon::Home, "Listen", Page::Listen);
        });
    }

    /// Per-frame Android housekeeping: permission polling, system-bar
    /// insets, and the left-drawer gesture/animation.
    #[cfg(target_os = "android")]
    fn update_android_session(&mut self, ctx: &egui::Context) {
        let _ = ctx;
        // Only does work while a permission request is actually pending.
        self.poll_android_permissions();
        if Instant::now() >= self.android_insets_next_poll {
            let insets = self.android.as_ref().map(|bridge| bridge.window_insets());
            if let Some((left, right, top, bottom)) = insets {
                self.android_insets = egui::epaint::MarginF32 {
                    left,
                    right,
                    top,
                    bottom,
                };
                self.android_insets_next_poll = Instant::now() + Duration::from_secs(1);
            }
        }
        self.update_drawer(ctx);
    }

    #[cfg(target_os = "android")]
    fn drawer_width(&self, ctx: &egui::Context) -> f32 {
        320.0_f32
            .min(ctx.input(|i| i.viewport_rect().width()) * 0.82)
            .max(240.0)
    }

    /// Reads pointer input each frame: swiping in from the left edge drifts
    /// the drawer in finger-following, releasing past the midpoint keeps it
    /// open (and the reverse when dragging the scrim back out). When no
    /// gesture is in progress the offset eases toward its target with a
    /// fast, snappy lerp for responsive feel.
    #[cfg(target_os = "android")]
    fn update_drawer(&mut self, ctx: &egui::Context) {
        let width = self.drawer_width(ctx);
        if self.drawer_drag.is_none() {
            let target = if self.drawer_open { width } else { 0.0 };
            // Fast lerp for snappy response (0.35 = reaches 95% in ~8 frames at 30fps)
            let diff = target - self.drawer_offset;
            if diff.abs() > 0.5 {
                self.drawer_offset += diff * 0.35;
            } else {
                self.drawer_offset = target;
            }
        }
        let (pressed, down, pointer) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.latest_pos(),
            )
        });

        // Settle an in-flight gesture.
        let mut release_open: Option<bool> = None;
        if let Some(drag) = self.drawer_drag.as_mut() {
            if let Some(p) = pointer.filter(|_| down) {
                let new_offset =
                    (drag.offset_at_start + p.x - drag.pointer_start_x).clamp(0.0, width);
                if (new_offset - drag.offset_at_start).abs() > 8.0 {
                    drag.moved = true;
                }
                self.drawer_offset = new_offset;
            }
            if !down {
                release_open = Some(if drag.started_on_scrim && !drag.moved {
                    false
                } else {
                    self.drawer_offset > width * 0.55
                });
            }
        }
        if let Some(open) = release_open {
            self.drawer_open = open;
            self.drawer_drag = None;
        }

        // Start a new gesture: from the far-left edge when closed, or over
        // the scrim region when open.
        if self.drawer_drag.is_none() && release_open.is_none() && pressed {
            if let Some(p) = pointer {
                if self.drawer_offset <= 1.0
                    && p.x <= ctx.input(|i| i.viewport_rect().left()) + 36.0
                {
                    self.drawer_drag = Some(DrawerDrag {
                        pointer_start_x: p.x,
                        offset_at_start: 0.0,
                        started_on_scrim: false,
                        moved: false,
                    });
                } else if self.drawer_offset >= width - 1.0 && p.x > self.drawer_offset + 12.0 {
                    self.drawer_drag = Some(DrawerDrag {
                        pointer_start_x: p.x,
                        offset_at_start: width,
                        started_on_scrim: true,
                        moved: false,
                    });
                }
            }
        }
    }

    /// Paints the drawer overlay: a dimmed scrim that closes on tap, and
    /// the navigation drawer itself sliding in behind the user's finger.
    #[cfg(target_os = "android")]
    fn render_android_drawer(&mut self, ctx: &egui::Context) {
        if self.drawer_offset <= 0.5 {
            return;
        }
        let screen = ctx.input(|i| i.viewport_rect());
        let width = self.drawer_width(ctx);
        let drawer_rect = egui::Rect::from_min_max(
            egui::pos2(screen.left() + self.drawer_offset - width, screen.top()),
            egui::pos2(screen.left() + self.drawer_offset, screen.bottom()),
        );

        let page_before = self.page;

        // Draw drawer FIRST (higher order = on top for input)
        egui::Area::new(egui::Id::new("navigation_drawer"))
            .order(egui::Order::Foreground)
            .current_pos(egui::pos2(
                screen.left() + self.drawer_offset - width,
                screen.top(),
            ))
            .movable(false)
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.set_min_height(screen.height());
                let margin_top = (self.android_insets.top.round() as i8 + 12).max(12);
                let margin_bottom = (self.android_insets.bottom.round() as i8 + 8).max(8);
                let margin = Margin {
                    left: 12,
                    right: 12,
                    top: margin_top,
                    bottom: margin_bottom,
                };
                Frame::new()
                    .fill(theme_sidebar())
                    .stroke(Stroke::new(1.0, theme_edge()))
                    .inner_margin(margin)
                    .corner_radius(CornerRadius::ZERO)
                    .show(ui, |ui| self.sidebar_contents(ui));
            });

        // Scrim - rendered BEHIND drawer so it doesn't intercept clicks on drawer content
        egui::Area::new(egui::Id::new("drawer_scrim"))
            .order(egui::Order::Background) // Behind drawer
            .current_pos(screen.min)
            .movable(false)
            .show(ctx, |ui| {
                let (rect, response) = ui.allocate_exact_size(screen.size(), egui::Sense::click());
                let alpha = ((self.drawer_offset / width).min(1.0) * 0.55) as u8;
                ui.painter().rect_filled(
                    rect,
                    CornerRadius::ZERO,
                    Color32::from_black_alpha(alpha),
                );
                if response.clicked() {
                    // Only close if click is outside drawer rect
                    if let Some(pointer_pos) = ui.ctx().input(|i| i.pointer.interact_pos()) {
                        if !drawer_rect.contains(pointer_pos) {
                            self.drawer_open = false;
                        }
                    }
                }
            });

        // Navigating from the drawer closes it with the slide animation.
        if self.page != page_before {
            self.drawer_open = false;
        }
    }

    fn nav_button(&mut self, ui: &mut egui::Ui, icon: Icon, label: &str, page: Page) {
        if nav_row(ui, icon, label, self.page == page).clicked() {
            self.page = page;
        }
    }

    fn profile_name(&self) -> String {
        let name = self.data.user_name.trim();
        if name.is_empty() {
            "OpenMix Listener".to_owned()
        } else {
            name.to_owned()
        }
    }

    fn profile_icon_key(&self) -> &str {
        if self.data.user_icon.is_empty() {
            "user"
        } else {
            self.data.user_icon.as_str()
        }
    }

    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    fn bottom_nav_item(
        &mut self,
        ui: &mut egui::Ui,
        icon: Icon,
        label: &str,
        page: Page,
        height: f32,
    ) {
        let selected = self.page == page
            || (page == Page::Library && matches!(self.page, Page::Liked | Page::Playlist));
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), height),
            egui::Sense::click(),
        );
        let ctx = ui.ctx();
        let hovered = ctx.animate_bool_responsive(response.id.with("hover"), response.hovered());
        let selected_t = ctx.animate_bool_responsive(response.id.with("selected"), selected);
        let pressed = ctx.animate_bool_responsive(
            response.id.with("pressed"),
            response.is_pointer_button_down_on(),
        );

        if hovered > 0.01 {
            ui.painter().rect_filled(
                rect.shrink2(Vec2::new(3.0, 4.0)),
                CornerRadius::same(10),
                Color32::from_rgba_unmultiplied(
                    theme_hover().r(),
                    theme_hover().g(),
                    theme_hover().b(),
                    (hovered * 150.0) as u8,
                ),
            );
        }

        let icon_center = egui::pos2(rect.center().x, rect.top() + height * 0.40);
        if selected_t > 0.01 {
            let pill = egui::Rect::from_center_size(icon_center, Vec2::new(48.0, 28.0));
            ui.painter().rect_filled(
                pill,
                CornerRadius::same(14),
                Color32::from_rgba_unmultiplied(
                    theme_red().r(),
                    theme_red().g(),
                    theme_red().b(),
                    (selected_t * 42.0) as u8,
                ),
            );
        }

        let color = color_lerp(
            color_lerp(theme_quiet(), theme_text(), hovered),
            theme_red(),
            selected_t,
        );
        paint_icon(
            ui.painter(),
            egui::Rect::from_center_size(
                icon_center + Vec2::new(0.0, -pressed * 1.0),
                Vec2::splat(22.0),
            ),
            icon,
            color,
        );
        ui.painter().text(
            egui::pos2(rect.center().x, rect.bottom() - 7.0),
            egui::Align2::CENTER_BOTTOM,
            label,
            eframe::egui::FontId::proportional(10.5),
            color,
        );
        if response.clicked() {
            self.page = page;
        }
    }

    fn right_now_panel(&mut self, root: &mut egui::Ui) {
        if !self.now_open {
            return;
        }
        egui::Panel::right("now_playing_panel")
            .default_size(292.0)
            .size_range(256.0..=380.0)
            .resizable(true)
            .frame(
                Frame::new()
                    .fill(theme_sidebar())
                    .inner_margin(Margin::symmetric(17, 20)),
            )
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("NOW PLAYING")
                            .size(10.0)
                            .strong()
                            .color(theme_quiet()),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icon_button(ui, Icon::Close, "Hide this panel").clicked() {
                            self.now_open = false;
                        }
                    });
                });
                ui.add_space(18.0);
                if let Some(track) = self.current_track() {
                    record_art(ui, &track, ui.available_width().min(254.0));
                    ui.add_space(18.0);
                    ui.label(
                        RichText::new(&track.title)
                            .size(20.0)
                            .strong()
                            .color(theme_text()),
                    );
                    ui.label(RichText::new(&track.artist).size(14.0).color(theme_soft()));
                    ui.add_space(18.0);
                    ui.separator();
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new("TRACK DETAILS")
                            .size(10.0)
                            .strong()
                            .color(theme_quiet()),
                    );
                    detail_row(ui, "Album", &track.album);
                    detail_row(
                        ui,
                        "Tempo",
                        &if track.bpm > 0 {
                            format!("{} BPM", track.bpm)
                        } else {
                            "Not analyzed".into()
                        },
                    );
                    detail_row(ui, "Length", &format_time(track.duration));
                    ui.add_space(16.0);
                    let like_label = if track.liked {
                        "In your liked tracks"
                    } else {
                        "Add to liked tracks"
                    };
                    if secondary_button(ui, like_label).clicked() {
                        self.toggle_like(&track.id);
                    }
                    if secondary_button(ui, "Add to queue").clicked() {
                        self.add_to_queue(&track.id);
                    }
                } else {
                    ui.add_space(80.0);
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            RichText::new("Choose a local track to see its details here.")
                                .size(13.0)
                                .color(theme_soft()),
                        );
                    });
                }
            });
    }

    /// Page-area margins; on Android the system bars are subtracted from
    /// the base padding so content never renders under the status or
    /// navigation bar.
    fn content_margin(&self) -> Margin {
        #[cfg(not(target_os = "android"))]
        {
            Margin::symmetric(28, 22)
        }
        #[cfg(target_os = "android")]
        {
            Margin {
                left: (self.android_insets.left.round() as i8 + 14).min(80),
                right: (self.android_insets.right.round() as i8 + 14).min(80),
                top: (self.android_insets.top.round() as i8 + 14).min(80),
                bottom: (self.android_insets.bottom.round() as i8 + 14).min(80),
            }
        }
    }

    fn player_bar(&mut self, root: &mut egui::Ui) {
        // Only show player bar when a track is playing
        let track = self.current_track();

        #[cfg(target_os = "android")]
        {
            self.android_player_and_navigation(root, track.as_ref());
            return;
        }

        #[cfg(not(target_os = "android"))]
        if track.is_none() {
            return;
        }

        // Get screen size for responsive scaling
        let screen_rect = root.ctx().input(|i| i.viewport_rect());
        let screen_width = screen_rect.width();
        let is_mobile = screen_width < 600.0 || cfg!(target_os = "android");

        // Responsive scaling factor based on screen width (baseline 1080px)
        let scale = (screen_width / 1080.0_f32).clamp(0.75, 1.5);

        // On Android the navigation bar overlaps the bottom of the window:
        // push the bar's padding up but keep the surface painted behind it.
        let bottom_pad = {
            #[cfg(target_os = "android")]
            {
                (self.android_insets.bottom.round() as i8 + 6).max(6)
            }
            #[cfg(not(target_os = "android"))]
            {
                8_i8
            }
        };

        // Taller player bar - shows only album art + track name on mobile
        // Click to open full-screen player (Spotify-style)
        let base_height = if is_mobile { 100.0_f32 } else { 90.0_f32 };
        let bar_height = (base_height * scale).clamp(88.0_f32, 140.0_f32) + f32::from(bottom_pad);

        egui::Panel::bottom("player_bar")
            .default_size(bar_height)
            .size_range(88.0..=160.0)
            .frame(
                Frame::new()
                    .fill(theme_surface())
                    .stroke(Stroke::new(1.0, theme_edge()))
                    .inner_margin(Margin {
                        left: (16.0_f32 * scale).round() as i8,
                        right: (16.0_f32 * scale).round() as i8,
                        top: (8.0_f32 * scale).round() as i8,
                        bottom: bottom_pad,
                    }),
            )
            .show(root, |ui| {
                let bar_rect = ui.available_rect_before_wrap();
                let full_h = bar_rect.height();
                let avail_w = bar_rect.width();
                let art_size = (64.0_f32 * scale).round().max(56.0);
                let title_size = (15.0_f32 * scale).round().max(13.0);
                let artist_size = (12.0_f32 * scale).round().max(11.0);

                ui.horizontal(|ui| {
                    ui.set_min_height(full_h);
                    ui.spacing_mut().item_spacing.x = 0.0;

                    // Extract track reference once to avoid moving
                    let t = track.as_ref().unwrap();

                    // ── LEFT: track info (clickable to open full-screen player) ──────────────────
                    let left_rect = ui
                        .allocate_ui_with_layout(
                            egui::vec2(avail_w * 0.75, full_h),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                ui.spacing_mut().item_spacing.x = (14.0_f32 * scale).round();
                                record_art(ui, t, art_size);
                                ui.vertical(|ui| {
                                    ui.add_space((-4.0_f32 * scale).round()); // vertical centering
                                    ui.label(
                                        RichText::new(&t.title)
                                            .size(title_size)
                                            .strong()
                                            .color(theme_text()),
                                    );
                                    ui.label(
                                        RichText::new(&t.artist)
                                            .size(artist_size)
                                            .color(theme_soft()),
                                    );
                                });
                            },
                        )
                        .response
                        .rect;

                    // Make the left area clickable to open full-screen player
                    let left_response = ui.interact(
                        left_rect,
                        ui.make_persistent_id("track_area"),
                        egui::Sense::click(),
                    );
                    if left_response.clicked() {
                        self.fullscreen_player_open = true;
                    }
                    if left_response.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }

                    // ── CENTER: Mini transport controls ────────────────────
                    ui.allocate_ui_with_layout(
                        egui::vec2(avail_w * 0.15, full_h),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.spacing_mut().item_spacing.x = (16.0_f32 * scale).round();
                            let icon_size = (24.0_f32 * scale).round().max(20.0);

                            let playing = self.audio.is_playing();
                            if self
                                .icon_button_sized(
                                    ui,
                                    if playing { Icon::Pause } else { Icon::Play },
                                    icon_size,
                                    "Play/Pause",
                                )
                                .clicked()
                            {
                                self.toggle();
                            }
                            if self
                                .icon_button_sized(ui, Icon::Next, icon_size, "Next")
                                .clicked()
                            {
                                self.next();
                            }
                            if self
                                .icon_button_sized(ui, Icon::Previous, icon_size, "Previous")
                                .clicked()
                            {
                                self.previous();
                            }
                        },
                    );

                    // ── RIGHT: Queue only ──────────────────────────────────
                    ui.allocate_ui_with_layout(
                        egui::vec2(avail_w * 0.10, full_h),
                        Layout::right_to_left(Align::Center),
                        |ui| {
                            ui.spacing_mut().item_spacing.x = (10.0_f32 * scale).round();
                            if self
                                .icon_button_sized(
                                    ui,
                                    Icon::Queue,
                                    (22.0_f32 * scale).round(),
                                    "Queue",
                                )
                                .clicked()
                            {
                                self.page = Page::Queue;
                            }
                        },
                    );
                });

                if let Some(error) = self.audio.error.as_deref() {
                    ui.colored_label(theme_red(), error);
                }
            });
    }

    #[cfg(target_os = "android")]
    fn android_player_and_navigation(&mut self, root: &mut egui::Ui, track: Option<&Track>) {
        let screen_rect = root.ctx().input(|i| i.viewport_rect());
        let scale = (screen_rect.width() / 1080.0_f32).clamp(0.75, 1.35);
        let system_bottom = self.android_insets.bottom.round().max(0.0);
        // Keep the whole strip above the Android gesture/3-button navigation
        // bar, and give it a little breathing room so the labels never touch
        // the system buttons.
        let frame_top = 6.0_f32;
        let frame_bottom = system_bottom + 10.0;
        let player_height = if track.is_some() {
            (72.0_f32 * scale).round().clamp(64.0, 82.0)
        } else {
            0.0
        };
        let nav_height = (62.0_f32 * scale).round().clamp(58.0, 70.0);
        let separator_height = if track.is_some() { 34.0 } else { 0.0 };
        let total_height = frame_top + player_height + separator_height + nav_height + frame_bottom;

        egui::Panel::bottom("android_player_navigation")
            .default_size(total_height)
            .exact_size(total_height)
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(theme_surface())
                    .stroke(Stroke::new(1.0, theme_edge()))
                    .inner_margin(Margin {
                        left: 12,
                        right: 12,
                        top: frame_top as i8,
                        bottom: frame_bottom as i8,
                    }),
            )
            .show(root, |ui| {
                ui.set_min_height(player_height + separator_height + nav_height);
                if let Some(track) = track {
                    ui.horizontal(|ui| {
                        ui.set_min_height(player_height);
                        let art_size = (48.0_f32 * scale).round().clamp(42.0, 56.0);
                        let title_size = (14.0_f32 * scale).round().max(12.0);
                        let artist_size = (11.0_f32 * scale).round().max(10.0);
                        record_art(ui, track, art_size);
                        let controls_width = if screen_rect.width() < 420.0 {
                            126.0
                        } else {
                            142.0
                        };
                        let info_width = (ui.available_width() - controls_width).max(40.0);
                        ui.allocate_ui_with_layout(
                            Vec2::new(info_width, player_height),
                            Layout::top_down(Align::LEFT),
                            |ui| {
                                ui.add_space(
                                    ((player_height - (title_size + artist_size + 4.0)) * 0.5)
                                        .max(0.0),
                                );
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&track.title)
                                            .size(title_size)
                                            .strong()
                                            .color(theme_text()),
                                    )
                                    .truncate(),
                                );
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&track.artist)
                                            .size(artist_size)
                                            .color(theme_soft()),
                                    )
                                    .truncate(),
                                );
                            },
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if icon_button(ui, Icon::Queue, "Open queue").clicked() {
                                self.page = Page::Queue;
                            }
                            if icon_button(ui, Icon::Next, "Next track").clicked() {
                                self.next();
                            }
                            let playing = self.audio.is_playing();
                            if icon_button(
                                ui,
                                if playing { Icon::Pause } else { Icon::Play },
                                "Play/Pause",
                            )
                            .clicked()
                            {
                                self.toggle();
                            }
                        });
                    });
                    ui.add_space(2.0);
                    ui.separator();
                    ui.add_space(2.0);
                }

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let item_height = ui.available_height().max(nav_height);
                    ui.columns(4, |columns| {
                        for (index, (icon, label, page)) in [
                            (Icon::Home, "Listen", Page::Listen),
                            (Icon::Mix, "Mixing", Page::Mixing),
                            (Icon::Library, "Library", Page::Library),
                            (Icon::Queue, "Queue", Page::Queue),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            columns[index].vertical(|ui| {
                                ui.set_min_height(item_height);
                                self.bottom_nav_item(ui, icon, label, page, item_height);
                            });
                        }
                    });
                });
            });
    }

    /// Helper for creating icon buttons with custom size in player bar
    fn icon_button_sized(
        &mut self,
        ui: &mut egui::Ui,
        icon: Icon,
        size: f32,
        tooltip: &str,
    ) -> egui::Response {
        let (rect, response) =
            ui.allocate_exact_size(Vec2::splat(size + 8.0), egui::Sense::click());
        if response.hovered() || response.highlighted() {
            let painter = ui.painter();
            let radius = (size * 0.5).min(16.0);
            painter.circle_filled(rect.center(), radius, theme_surface_2());
        }
        let painter = ui.painter();
        let icon_rect = egui::Rect::from_center_size(rect.center(), Vec2::splat(size));
        paint_icon(
            painter,
            icon_rect,
            icon,
            if response.hovered() {
                theme_red()
            } else {
                theme_text()
            },
        );
        response.on_hover_text(tooltip)
    }

    /// Full-screen player modal (Spotify-style): big cover art, timeline, controls
    fn fullscreen_player(&mut self, ctx: &egui::Context) {
        if !self.fullscreen_player_open {
            return;
        }
        let Some(track) = self.current_track() else {
            self.fullscreen_player_open = false;
            return;
        };
        let mut open = true;
        let screen_rect = ctx.input(|i| i.viewport_rect());
        let is_mobile = screen_rect.width() < 600.0 || cfg!(target_os = "android");

        egui::Window::new("Now Playing")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .fixed_size(screen_rect.size())
            .frame(Frame::new().fill(theme_bg()))
            .show(ctx, |ui| {
                // Handle backdrop click to close
                let response = ui.allocate_rect(screen_rect, egui::Sense::click());
                if response.clicked() && !response.hovered() {
                    // Only close if clicking empty area - but we'll handle this differently
                }

                ui.vertical_centered(|ui| {
                    ui.add_space(if is_mobile { 40.0 } else { 80.0 });

                    // ── BIG COVER ART ──────────────────────────────────────
                    let art_size = if is_mobile {
                        (screen_rect.width() * 0.75).min(320.0)
                    } else {
                        320.0
                    };
                    record_art(ui, &track, art_size);

                    ui.add_space(32.0);

                    // ── TRACK INFO ─────────────────────────────────────────
                    ui.label(
                        RichText::new(&track.title)
                            .size(if is_mobile { 28.0 } else { 32.0 })
                            .strong()
                            .color(theme_text()),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(&track.artist)
                            .size(if is_mobile { 18.0 } else { 20.0 })
                            .color(theme_soft()),
                    );
                    if track.album != "Local library" && track.album != "Unknown album" {
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(&track.album)
                                .size(if is_mobile { 15.0 } else { 16.0 })
                                .color(theme_quiet()),
                        );
                    }

                    ui.add_space(40.0);

                    // ── TIMELINE ───────────────────────────────────────────
                    let timeline_width = if is_mobile {
                        screen_rect.width() * 0.85
                    } else {
                        500.0
                    };
                    let pos = self.audio.position().as_secs_f32();
                    let len = self.audio.duration.as_secs_f32().max(1.0);
                    let progress = pos / len;

                    ui.allocate_ui_with_layout(
                        egui::vec2(timeline_width, 60.0),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            let time_font = if is_mobile { 14.0 } else { 13.0 };
                            ui.label(
                                RichText::new(format_time(pos as u32))
                                    .size(time_font)
                                    .color(theme_quiet()),
                            );
                            ui.add_space(12.0);

                            let (slider_rect, _slider_response) = ui.allocate_exact_size(
                                egui::vec2(timeline_width - 120.0, 44.0),
                                egui::Sense::drag(),
                            );

                            // Draw custom slider
                            let painter = ui.painter_at(slider_rect);
                            let track_height = 4.0;
                            let track_rect = egui::Rect::from_min_max(
                                egui::pos2(
                                    slider_rect.left(),
                                    slider_rect.center().y - track_height / 2.0,
                                ),
                                egui::pos2(
                                    slider_rect.right(),
                                    slider_rect.center().y + track_height / 2.0,
                                ),
                            );
                            painter.rect_filled(track_rect, CornerRadius::from(2.0), theme_edge());

                            let fill_width = track_rect.width() * progress;
                            let fill_rect = egui::Rect::from_min_max(
                                track_rect.min,
                                egui::pos2(track_rect.left() + fill_width, track_rect.bottom()),
                            );
                            painter.rect_filled(fill_rect, CornerRadius::from(2.0), theme_red());

                            // Handle (circle)
                            let handle_x = track_rect.left() + fill_width;
                            let handle_center = egui::pos2(handle_x, track_rect.center().y);
                            let handle_radius = if is_mobile { 12.0 } else { 10.0 };
                            painter.circle_filled(handle_center, handle_radius, theme_red());
                            painter.circle_stroke(
                                handle_center,
                                handle_radius + 2.0,
                                Stroke::new(
                                    3.0,
                                    Color32::from_rgba_unmultiplied(255, 255, 255, 180),
                                ),
                            );

                            // Interaction
                            let slider_response = ui.interact(
                                slider_rect,
                                ui.make_persistent_id("fs_slider"),
                                egui::Sense::click_and_drag(),
                            );
                            if (slider_response.dragged() || slider_response.clicked())
                                && let Some(pointer_pos) =
                                    ui.ctx().input(|i| i.pointer.interact_pos())
                            {
                                let new_progress = ((pointer_pos.x - track_rect.left())
                                    / track_rect.width())
                                .clamp(0.0, 1.0);
                                let new_pos = new_progress * len;
                                let _ = self.audio.seek(Duration::from_secs_f32(new_pos));
                            }

                            ui.add_space(12.0);
                            ui.label(
                                RichText::new(format_time(len as u32))
                                    .size(time_font)
                                    .color(theme_quiet()),
                            );
                        },
                    );

                    ui.add_space(40.0);

                    // ── CONTROLS (Spotify-style: shuffle, prev, play, next, repeat) ─────────────
                    let controls_spacing = if is_mobile { 28.0 } else { 32.0 };
                    let play_button_size = if is_mobile { 72.0 } else { 64.0 };
                    let other_button_size = if is_mobile { 48.0 } else { 40.0 };

                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = controls_spacing;

                        // Shuffle
                        let shuffle_active = self.shuffle;
                        let shuffle_resp = ui
                            .add(
                                egui::Button::new(
                                    RichText::new(icon_char(Icon::Shuffle))
                                        .size(other_button_size)
                                        .color(if shuffle_active {
                                            theme_red()
                                        } else {
                                            theme_soft()
                                        }),
                                )
                                .frame(false)
                                .fill(if shuffle_active {
                                    theme_surface_2()
                                } else {
                                    Color32::TRANSPARENT
                                })
                                .corner_radius(other_button_size / 2.0)
                                .min_size(Vec2::splat(other_button_size)),
                            )
                            .on_hover_text("Shuffle");
                        if shuffle_resp.clicked() {
                            self.shuffle = !self.shuffle;
                        }

                        // Previous
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(icon_char(Icon::Previous))
                                        .size(other_button_size)
                                        .color(theme_text()),
                                )
                                .frame(false)
                                .fill(Color32::TRANSPARENT)
                                .corner_radius(other_button_size / 2.0)
                                .min_size(Vec2::splat(other_button_size)),
                            )
                            .on_hover_text("Previous")
                            .clicked()
                        {
                            self.previous();
                        }

                        // Play/Pause (large center button)
                        let playing = self.audio.is_playing();
                        let play_icon = if playing { Icon::Pause } else { Icon::Play };
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(icon_char(play_icon))
                                        .size(play_button_size)
                                        .color(theme_bg()),
                                )
                                .frame(true)
                                .fill(theme_red())
                                .corner_radius(play_button_size / 2.0)
                                .stroke(Stroke::NONE)
                                .min_size(Vec2::splat(play_button_size)),
                            )
                            .on_hover_text(if playing { "Pause" } else { "Play" })
                            .clicked()
                        {
                            self.toggle();
                        }

                        // Next
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(icon_char(Icon::Next))
                                        .size(other_button_size)
                                        .color(theme_text()),
                                )
                                .frame(false)
                                .fill(Color32::TRANSPARENT)
                                .corner_radius(other_button_size / 2.0)
                                .min_size(Vec2::splat(other_button_size)),
                            )
                            .on_hover_text("Next")
                            .clicked()
                        {
                            self.next();
                        }

                        // Repeat
                        let repeat_active = self.repeat > 0;
                        let repeat_icon = if self.repeat == 1 {
                            Icon::RepeatOne
                        } else {
                            Icon::Repeat
                        };
                        let repeat_resp = ui
                            .add(
                                egui::Button::new(
                                    RichText::new(icon_char(repeat_icon))
                                        .size(other_button_size)
                                        .color(if repeat_active {
                                            theme_red()
                                        } else {
                                            theme_soft()
                                        }),
                                )
                                .frame(false)
                                .fill(if repeat_active {
                                    theme_surface_2()
                                } else {
                                    Color32::TRANSPARENT
                                })
                                .corner_radius(other_button_size / 2.0)
                                .min_size(Vec2::splat(other_button_size)),
                            )
                            .on_hover_text("Repeat");
                        if repeat_resp.clicked() {
                            self.repeat = (self.repeat + 1) % 3;
                        }
                    });

                    ui.add_space(if is_mobile { 60.0 } else { 40.0 });

                    // ── CLOSE BUTTON ───────────────────────────────────────
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("✕")
                                    .size(if is_mobile { 22.0 } else { 20.0 })
                                    .color(theme_soft()),
                            )
                            .frame(false)
                            .fill(Color32::TRANSPARENT),
                        )
                        .on_hover_text("Close")
                        .clicked()
                    {
                        self.fullscreen_player_open = false;
                    }
                });
            });

        if !open {
            self.fullscreen_player_open = false;
        }
    }

    fn content(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_rect_before_wrap();
        ui.painter()
            .rect_filled(available, CornerRadius::ZERO, theme_bg());

        let margin = self.content_margin();

        // Header metrics. The search box drops to a second row on narrow
        // screens; the header height must account for that or its content
        // spills over the page below it.
        let scale = (available.width() / 1080.0_f32).clamp(0.75, 1.5);
        let title_size = (27.0_f32 * scale).round().max(20.0);
        let row1_height = (title_size + 22.0).max(50.0);
        let search_row_height = 42.0_f32;
        let topbar_padding = (10.0_f32 * scale).round().clamp(8.0, 12.0);
        let topbar_side = (18.0_f32 * scale).round().clamp(14.0, 30.0);
        #[cfg(target_os = "android")]
        let (topbar_left, topbar_right) = (
            topbar_side + self.android_insets.left.round(),
            topbar_side + self.android_insets.right.round(),
        );
        #[cfg(not(target_os = "android"))]
        let (topbar_left, topbar_right) = (topbar_side, topbar_side);

        let is_collection = matches!(self.page, Page::Library | Page::Liked | Page::Playlist);
        let can_search = is_collection || matches!(self.page, Page::Listen | Page::Mixing);
        // The title, import button and search box cannot share one line on a
        // phone, so the search box gets a full-width row of its own.
        let narrow = (available.width() - topbar_left - topbar_right) < 720.0;
        let show_search_row = can_search && narrow;

        let topbar_content_height = row1_height
            + if show_search_row {
                8.0 + search_row_height
            } else {
                0.0
            };
        let topbar_height = topbar_padding * 2.0 + topbar_content_height;

        #[cfg(target_os = "android")]
        let topbar_extra = (self.android_insets.top.round() + 6.0).max(20.0);
        #[cfg(not(target_os = "android"))]
        let topbar_extra = 0.0;

        let total_topbar_height = topbar_height + topbar_extra;

        // Reserve space for the system bar and the lower app header.
        let content_top = available.top() + total_topbar_height + 6.0;
        let content_bottom = available.bottom() - f32::from(margin.bottom);
        let content_width = (available.width() - f32::from(margin.left + margin.right)).max(0.0);

        // On Android the header starts below the notification/status bar so
        // navigation labels never sit inside the system UI.
        let topbar_rect = egui::Rect::from_min_max(
            egui::pos2(available.left(), available.top() + topbar_extra),
            egui::pos2(available.right(), available.top() + total_topbar_height),
        );
        if topbar_extra > 0.0 {
            let system_bar_rect = egui::Rect::from_min_max(
                available.left_top(),
                egui::pos2(available.right(), available.top() + topbar_extra),
            );
            ui.painter_at(system_bar_rect).rect_filled(
                system_bar_rect,
                CornerRadius::ZERO,
                theme_surface(),
            );
        }
        ui.painter_at(topbar_rect)
            .rect_filled(topbar_rect, CornerRadius::ZERO, theme_surface());
        ui.painter_at(topbar_rect).rect_stroke(
            egui::Rect::from_min_max(
                topbar_rect.min,
                egui::pos2(topbar_rect.max.x, topbar_rect.max.y - 1.0),
            ),
            CornerRadius::ZERO,
            Stroke::new(1.0, theme_edge()),
            egui::StrokeKind::Inside,
        );

        // Topbar content with side margins.
        let topbar_content_rect = egui::Rect::from_min_max(
            egui::pos2(
                topbar_rect.left() + topbar_left,
                topbar_rect.top() + topbar_padding,
            ),
            egui::pos2(
                topbar_rect.right() - topbar_right,
                topbar_rect.bottom() - topbar_padding,
            ),
        );

        let mut topbar_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(topbar_content_rect)
                .layout(Layout::top_down(Align::LEFT)),
        );
        self.render_topbar_content(
            &mut topbar_ui,
            scale,
            narrow,
            row1_height,
            search_row_height,
            show_search_row,
        );

        // Content area below topbar
        let content_rect = egui::Rect::from_min_max(
            egui::pos2(available.left() + f32::from(margin.left), content_top),
            egui::pos2(available.right() - f32::from(margin.right), content_bottom),
        );

        let mut content_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(content_rect)
                .layout(Layout::top_down(Align::Center)),
        );
        content_ui.set_min_width(content_width);

        egui::ScrollArea::vertical()
            .id_salt("main_page_scroll")
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(&mut content_ui, |ui| {
                match self.page {
                    Page::Listen => self.listen_page(ui),
                    Page::Mixing => self.mixing_page(ui),
                    Page::Library | Page::Liked | Page::Playlist => self.library_page(ui),
                    Page::Queue => self.queue_page(ui),
                    Page::Settings => self.settings_page(ui),
                };
                // Add extra bottom padding on Android to ensure content
                // is not hidden behind the navigation bar
                #[cfg(target_os = "android")]
                {
                    let extra_bottom = (self.android_insets.bottom.round() + 12.0).max(20.0);
                    ui.add_space(extra_bottom);
                }
            });
    }

    fn render_topbar_content(
        &mut self,
        ui: &mut egui::Ui,
        scale: f32,
        narrow: bool,
        row1_height: f32,
        search_row_height: f32,
        show_search_row: bool,
    ) {
        let title = match self.page {
            Page::Listen => "Listen".to_owned(),
            Page::Mixing => "Mixing".to_owned(),
            Page::Library => "Your library".to_owned(),
            Page::Liked => "Liked tracks".to_owned(),
            Page::Playlist => self
                .active_playlist
                .as_ref()
                .and_then(|id| self.data.playlists.iter().find(|p| &p.id == id))
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "Playlist".to_owned()),
            Page::Queue => "Play queue".to_owned(),
            Page::Settings => "Settings".to_owned(),
        };
        let is_collection = matches!(self.page, Page::Library | Page::Liked | Page::Playlist);
        let can_search = is_collection || matches!(self.page, Page::Listen | Page::Mixing);
        let away_from_collection = !is_collection
            && !self.search.trim().is_empty()
            && self.page != Page::Queue
            && self.page != Page::Settings;
        let hint = if is_collection {
            "Filter this collection"
        } else {
            "Search your library"
        };
        let title_size = (27.0_f32 * scale).round().max(20.0);
        let title_gap = (10.0_f32 * scale).round().clamp(6.0, 12.0);
        let inline_search = !narrow && can_search;
        let inline_search_width = (240.0_f32 * scale).round().clamp(200.0, 280.0);
        let show_results_reserve = if inline_search && !is_collection {
            104.0
        } else {
            0.0
        };
        // Space kept free on the right for the search/import controls so the
        // title can truncate instead of pushing them off-screen.
        let right_reserve = if narrow {
            96.0
        } else {
            148.0
                + if inline_search {
                    title_gap + inline_search_width + show_results_reserve
                } else {
                    0.0
                }
        };

        ui.horizontal(|ui| {
            ui.set_min_height(row1_height);
            ui.spacing_mut().item_spacing.x = title_gap;
            #[cfg(target_os = "android")]
            if profile_avatar_button(ui, self.profile_icon_key(), "Open the full menu").clicked() {
                self.drawer_open = true;
            }
            let title_width = (ui.available_width() - right_reserve).max(48.0);
            ui.allocate_ui_with_layout(
                Vec2::new(title_width, row1_height),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(title)
                                .size(title_size)
                                .strong()
                                .color(theme_text()),
                        )
                        .truncate(),
                    );
                },
            );
            if inline_search {
                self.search_box(ui, hint, inline_search_width);
                if away_from_collection && small_secondary_button(ui, "Show results").clicked() {
                    self.page = Page::Library;
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let import = if narrow {
                    secondary_button(ui, "Import")
                } else {
                    primary_button(ui, "Import files")
                };
                if import.clicked() {
                    self.import_files();
                }
            });
        });
        if show_search_row {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.set_min_height(search_row_height);
                let width = if away_from_collection {
                    (ui.available_width() - 112.0).max(120.0)
                } else {
                    ui.available_width()
                };
                self.search_box(ui, hint, width);
                if away_from_collection && small_secondary_button(ui, "Show results").clicked() {
                    self.page = Page::Library;
                }
            });
        }
    }

    fn search_box(&mut self, ui: &mut egui::Ui, hint: &str, width: f32) {
        let has_query = !self.search.is_empty();
        let mut clear = false;
        let response = Frame::new()
            .fill(theme_surface_2())
            .stroke(Stroke::new(1.0, theme_edge()))
            .corner_radius(CornerRadius::same(9))
            .inner_margin(Margin::symmetric(10, 4))
            .show(ui, |ui| {
                ui.set_width((width - 20.0).max(80.0));
                ui.horizontal(|ui| {
                    inline_icon(ui, Icon::Search);
                    let reserve = if has_query { 38.0 } else { 6.0 };
                    let text_width = (ui.available_width() - reserve).max(60.0);
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .hint_text(hint)
                            .frame(Frame::NONE)
                            .desired_width(text_width),
                    );
                    if has_query
                        && ui
                            .add(
                                egui::Button::new(
                                    RichText::new("×").size(16.0).color(theme_quiet()),
                                )
                                .frame(false)
                                .fill(Color32::TRANSPARENT)
                                .min_size(Vec2::new(24.0, 24.0)),
                            )
                            .on_hover_text("Clear search")
                            .clicked()
                    {
                        clear = true;
                    }
                    response
                })
                .inner
            })
            .inner;
        if clear {
            self.search.clear();
        }
        if self.search_focus {
            response.request_focus();
            self.search_focus = false;
        }
    }

    fn listen_page(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        ui.label(
            RichText::new("Your music, in one place.")
                .size(15.0)
                .color(theme_soft()),
        );
        ui.add_space(24.0);
        let mut added_tracks = self.data.tracks.clone();
        added_tracks.sort_by_key(|track| std::cmp::Reverse(track.added));
        if !added_tracks.is_empty() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Recently added")
                        .size(18.0)
                        .strong()
                        .color(theme_text()),
                );
                if added_tracks.len() >= 2 && primary_button(ui, "Mix all").clicked() {
                    self.mix_all(added_tracks.clone());
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.link("View library").clicked() {
                        self.page = Page::Library;
                    }
                });
            });
            ui.add_space(8.0);
            for track in added_tracks.into_iter().take(8) {
                self.track_row(ui, &track, false);
            }
            ui.add_space(24.0);
        }
        let recent_ids = self.data.recent.clone();
        let recent: Vec<Track> = recent_ids
            .iter()
            .filter_map(|id| self.data.tracks.iter().find(|t| &t.id == id).cloned())
            .collect();
        if recent.is_empty() && self.data.tracks.is_empty() {
            self.empty_state(
                ui,
                "No music here yet",
                "Import local audio files to start building your own library.",
            );
        } else if !recent.is_empty() {
            self.section_header(ui, "Recently played", Some(("View library", Page::Library)));
            ui.add_space(8.0);
            for track in recent.into_iter().take(8) {
                self.track_row(ui, &track, false);
            }
        }
        ui.add_space(30.0);
        if !self.data.tracks.is_empty() {
            self.section_header(ui, "Ready to mix", Some(("Open Mixing", Page::Mixing)));
            ui.label(
                RichText::new(
                    "Choose a starting track and OpenMix will find compatible local files.",
                )
                .size(12.0)
                .color(theme_soft()),
            );
        }
    }

    fn mixing_page(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("Create a hand-off from the music you own.")
                .size(14.0)
                .color(theme_soft()),
        );
        ui.add_space(22.0);
        if self.data.tracks.len() < 2 {
            self.empty_state(
                ui,
                "Mixing needs two tracks",
                "Import at least two local MP3 or WAV files. Analysis stays on this device.",
            );
            return;
        }
        let selected = self
            .current_track()
            .or_else(|| self.data.tracks.first().cloned());
        let Some(outgoing) = selected else {
            return;
        };
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("FROM")
                    .size(10.0)
                    .strong()
                    .color(theme_quiet()),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new(&outgoing.title)
                    .size(16.0)
                    .strong()
                    .color(theme_text()),
            );
            ui.label(
                RichText::new(format!(
                    "{}  ·  {} BPM",
                    outgoing.artist,
                    bpm_label(outgoing.bpm)
                ))
                .size(12.0)
                .color(theme_soft()),
            );
            if self.current.is_none() && secondary_button(ui, "Choose from library").clicked() {
                self.page = Page::Library;
            }
        });
        ui.add_space(22.0);
        self.section_header(ui, "Compatible next tracks", None);
        ui.add_space(8.0);
        let mut candidates = self.data.tracks.clone();
        candidates.retain(|t| t.id != outgoing.id);
        candidates.sort_by(|a, b| mix_score(&outgoing, a).total_cmp(&mix_score(&outgoing, b)));
        for track in candidates.into_iter().take(12) {
            self.suggestion_row(&outgoing, &track, ui);
        }
        ui.add_space(12.0);
        ui.label(
            RichText::new(
                "OpenMix compares tempo, full-track energy changes, and release details to find compatible hand-offs.",
            )
            .size(11.0)
            .color(theme_quiet()),
        );
    }

    fn suggestion_row(&mut self, from: &Track, to: &Track, ui: &mut egui::Ui) {
        let reason = if from.bpm > 0 && to.bpm > 0 {
            format!(
                "{} BPM  ·  {}",
                bpm_label(to.bpm),
                if (from.bpm as i32 - to.bpm as i32).abs() <= 5 {
                    "close tempo"
                } else {
                    "half-time compatible"
                }
            )
        } else {
            format!("{} BPM  ·  tempo not detected", bpm_label(to.bpm))
        };
        Frame::new()
            .fill(theme_surface())
            .inner_margin(Margin::symmetric(12, 10))
            .stroke(Stroke::new(1.0, theme_edge()))
            .corner_radius(CornerRadius::same(7))
            .show(ui, |ui| {
                let narrow = ui.available_width() < 560.0;
                ui.horizontal(|ui| {
                    record_art(ui, to, 42.0);
                    let reserve = if narrow { 0.0 } else { 196.0 };
                    let info_width = (ui.available_width() - reserve).max(80.0);
                    ui.allocate_ui_with_layout(
                        Vec2::new(info_width, 42.0),
                        Layout::top_down(Align::LEFT),
                        |ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&to.title)
                                        .size(14.0)
                                        .strong()
                                        .color(theme_text()),
                                )
                                .truncate(),
                            );
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!("{}  ·  {reason}", to.artist))
                                        .size(11.0)
                                        .color(theme_soft()),
                                )
                                .truncate(),
                            );
                        },
                    );
                    if !narrow {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if primary_button(ui, "Mix now").clicked() {
                                self.queue_transition(from.clone(), to.clone());
                            }
                            if secondary_button(ui, "Play next").clicked() {
                                self.add_to_queue(&to.id);
                            }
                        });
                    }
                });
                if narrow {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if primary_button(ui, "Mix now").clicked() {
                            self.queue_transition(from.clone(), to.clone());
                        }
                        if secondary_button(ui, "Play next").clicked() {
                            self.add_to_queue(&to.id);
                        }
                    });
                }
                if let Some((note, fallback)) = self.notes.get(&to.id) {
                    ui.add_space(7.0);
                    ui.label(
                        RichText::new(if *fallback {
                            format!("LOCAL PLAN  ·  {note}")
                        } else {
                            format!("AI PLAN  ·  {note}")
                        })
                        .size(12.0)
                        .color(if *fallback {
                            theme_soft()
                        } else {
                            theme_text()
                        }),
                    );
                }
            });
        ui.add_space(7.0);
    }

    fn library_page(&mut self, ui: &mut egui::Ui) {
        let tracks = self.visible_tracks();
        let heading = match self.page {
            Page::Liked => format!("{} liked tracks", tracks.len()),
            Page::Playlist => format!("{} tracks", tracks.len()),
            _ => format!("{} tracks", tracks.len()),
        };
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
            ui.label(RichText::new(heading).size(12.0).color(theme_soft()));
            ui.add_space(8.0);
            if self.page == Page::Library && secondary_button(ui, "New playlist").clicked() {
                self.playlist_name.clear();
                self.playlist_edit = None;
                self.playlist_dialog = true;
            }
            egui::ComboBox::from_id_salt("sort_mode")
                .selected_text(format!("Sort: {}", self.sort.label()))
                .show_ui(ui, |ui| {
                    for mode in [
                        SortMode::RecentlyAdded,
                        SortMode::PlaylistOrder,
                        SortMode::Title,
                        SortMode::Artist,
                        SortMode::Duration,
                        SortMode::Bpm,
                    ] {
                        ui.selectable_value(&mut self.sort, mode, mode.label());
                    }
                });
            if icon_button(
                ui,
                if self.sort_ascending {
                    Icon::SortUp
                } else {
                    Icon::SortDown
                },
                "Reverse sort",
            )
            .clicked()
            {
                self.sort_ascending = !self.sort_ascending;
            }
            if !tracks.is_empty()
                && self.selected_tracks.len() < tracks.len()
                && small_secondary_button(ui, "Select all").clicked()
            {
                let ids: HashSet<String> = tracks.iter().map(|t| t.id.clone()).collect();
                self.selected_tracks.extend(ids);
            }
            if self.page == Page::Playlist {
                if !tracks.is_empty() && primary_button(ui, "Play playlist").clicked() {
                    self.queue = tracks.iter().skip(1).map(|t| t.id.clone()).collect();
                    self.play_id(&tracks[0].id);
                }
                if tracks.len() >= 2 && primary_button(ui, "Mix all").clicked() {
                    self.mix_all(tracks.clone());
                }
                let mut rename = false;
                let mut delete = false;
                menu_button(ui, "More", |ui| {
                    rename = secondary_button(ui, "Rename playlist").clicked();
                    delete = danger_button(ui, "Delete playlist").clicked();
                });
                if rename {
                    self.playlist_edit = self.active_playlist.clone();
                    self.playlist_name = self
                        .active_playlist
                        .as_ref()
                        .and_then(|id| self.data.playlists.iter().find(|p| &p.id == id))
                        .map(|p| p.name.clone())
                        .unwrap_or_default();
                    self.playlist_dialog = true;
                }
                if delete {
                    self.playlist_delete = self.active_playlist.clone();
                }
            }
        });
        if !self.selected_tracks.is_empty() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{} selected", self.selected_tracks.len()))
                        .color(theme_red()),
                );
                ui.add_space(12.0);
                let mut bulk_action: Option<BulkPlaylistAction> = None;
                menu_button(ui, "Add to playlist", |ui| {
                    if secondary_button(ui, "New playlist…").clicked() {
                        bulk_action = Some(BulkPlaylistAction::New);
                        ui.close();
                    }
                    if !self.data.playlists.is_empty() {
                        ui.separator();
                        let playlists = self.data.playlists.clone();
                        for p in playlists {
                            if secondary_button(ui, p.name.as_str()).clicked() {
                                bulk_action = Some(BulkPlaylistAction::Existing(p.id));
                                ui.close();
                            }
                        }
                    }
                });
                match bulk_action {
                    Some(BulkPlaylistAction::New) => {
                        self.playlist_dialog = true;
                        self.playlist_edit = None;
                        self.playlist_name = String::new();
                    }
                    Some(BulkPlaylistAction::Existing(id)) => {
                        if let Some(p) = self.data.playlists.iter_mut().find(|p| p.id == id) {
                            for track_id in &self.selected_tracks {
                                if !p.tracks.contains(track_id) {
                                    p.tracks.push(track_id.clone());
                                }
                            }
                            self.persist();
                            self.selected_tracks.clear();
                        }
                    }
                    None => {}
                }
                if ghost_button(ui, "Clear selection").clicked() {
                    self.selected_tracks.clear();
                }
            });
            ui.separator();
        }
        ui.add_space(12.0);
        if tracks.is_empty() {
            let empty_playlist = self.page == Page::Playlist;
            let heading = if !self.search.is_empty() {
                "No results"
            } else if empty_playlist {
                "This playlist is empty"
            } else {
                "Your library is empty"
            };
            let detail = if !self.search.is_empty() {
                "Try another search or clear the filter."
            } else if empty_playlist {
                "Add tracks from the library with each track’s menu."
            } else {
                "Choose the files you want to listen to. OpenMix does not bundle music."
            };
            self.empty_state(ui, heading, detail);
            return;
        }
        if ui.available_width() >= 560.0 {
            ui.horizontal(|ui| {
                ui.add_space(105.0);
                ui.label(
                    RichText::new("TITLE")
                        .size(10.0)
                        .strong()
                        .color(theme_quiet()),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new("LENGTH")
                            .size(10.0)
                            .strong()
                            .color(theme_quiet()),
                    );
                    ui.add_space(62.0);
                    ui.label(
                        RichText::new("BPM")
                            .size(10.0)
                            .strong()
                            .color(theme_quiet()),
                    );
                });
            });
            ui.separator();
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show_rows(ui, 58.0, tracks.len(), |ui, range| {
                for index in range {
                    self.track_row(ui, &tracks[index], true);
                }
            });
    }

    fn track_row(&mut self, ui: &mut egui::Ui, track: &Track, show_actions: bool) {
        let selected = self.current.as_deref() == Some(track.id.as_str());
        let narrow = ui.available_width() < 560.0;
        Frame::new()
            .fill(if selected {
                theme_surface_2()
            } else {
                Color32::TRANSPARENT
            })
            .inner_margin(Margin::symmetric(7, 5))
            .corner_radius(CornerRadius::same(5))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = if narrow { 6.0 } else { 9.0 };
                    let play = icon_button(
                        ui,
                        if selected && self.audio.is_playing() {
                            Icon::Pause
                        } else {
                            Icon::Play
                        },
                        if selected && self.audio.is_playing() {
                            "Pause"
                        } else {
                            "Play"
                        },
                    );
                    if play.clicked() {
                        if selected && !self.audio.ended() {
                            self.toggle();
                        } else {
                            self.play_id(&track.id);
                        }
                    }
                    ui.add_space(if narrow { 0.0 } else { 2.0 });
                    let mut selected_val = self.selected_tracks.contains(&track.id);
                    if ui.checkbox(&mut selected_val, "").changed() {
                        if selected_val {
                            self.selected_tracks.insert(track.id.clone());
                        } else {
                            self.selected_tracks.remove(&track.id);
                        }
                    }
                    ui.add_space(if narrow { 0.0 } else { 2.0 });
                    record_art(ui, track, if narrow { 32.0 } else { 36.0 });
                    let reserve = if narrow {
                        if show_actions { 80.0 } else { 46.0 }
                    } else if show_actions {
                        156.0
                    } else {
                        100.0
                    };
                    let info_width = (ui.available_width() - reserve).max(64.0);
                    ui.allocate_ui_with_layout(
                        Vec2::new(info_width, 38.0),
                        Layout::top_down(Align::LEFT),
                        |ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            let title = ui.add(
                                egui::Label::new(
                                    RichText::new(&track.title).size(13.0).color(if selected {
                                        theme_red()
                                    } else {
                                        theme_text()
                                    }),
                                )
                                .truncate()
                                .sense(egui::Sense::click()),
                            );
                            title.context_menu(|ui| self.track_context_menu(ui, track));
                            let mut details = vec![track.artist.clone()];
                            if let Some(year) = &track.release_year {
                                details.push(year.clone());
                            }
                            if track.album != "Local library" {
                                details.push(track.album.clone());
                            }
                            ui.add(
                                egui::Label::new(
                                    RichText::new(details.join("  ·  "))
                                        .size(10.5)
                                        .color(theme_soft()),
                                )
                                .truncate(),
                            );
                        },
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format_time(track.duration))
                                .size(11.0)
                                .color(theme_soft()),
                        );
                        if !narrow {
                            ui.add_space(18.0);
                            ui.label(
                                RichText::new(bpm_label(track.bpm))
                                    .size(11.0)
                                    .color(theme_soft()),
                            );
                        }
                        if show_actions {
                            ui.add_space(if narrow { 4.0 } else { 12.0 });
                            menu_button(
                                ui,
                                RichText::new("⋯").size(18.0).color(theme_soft()),
                                |ui| self.track_context_menu(ui, track),
                            )
                            .response
                            .on_hover_text("Track actions");
                        }
                    });
                });
            });
    }

    fn track_context_menu(&mut self, ui: &mut egui::Ui, track: &Track) {
        if primary_button(ui, "Play now").clicked() {
            self.play_id(&track.id);
            ui.close();
        }
        if secondary_button(ui, "Add to queue").clicked() {
            self.add_to_queue(&track.id);
            ui.close();
        }
        let like_label = if track.liked {
            "Remove from liked tracks"
        } else {
            "Add to liked tracks"
        };
        if secondary_button(ui, like_label).clicked() {
            self.toggle_like(&track.id);
            ui.close();
        }
        ui.separator();
        if danger_button(ui, "Remove from OpenMix")
            .on_hover_text(
                "Removes it from your library and playlists. The audio file stays on disk.",
            )
            .clicked()
        {
            self.remove_track(&track.id);
            ui.close();
        }
        if self.page == Page::Playlist {
            if danger_button(ui, "Remove from this playlist").clicked() {
                if let Some(playlist) = self
                    .active_playlist
                    .as_ref()
                    .and_then(|id| self.data.playlists.iter_mut().find(|p| &p.id == id))
                {
                    playlist.tracks.retain(|id| id != &track.id);
                    self.persist();
                }
                ui.close();
            }
            let playlist_id = self.active_playlist.clone();
            let position = playlist_id.as_ref().and_then(|id| {
                self.data
                    .playlists
                    .iter()
                    .find(|playlist| &playlist.id == id)
                    .and_then(|playlist| {
                        playlist
                            .tracks
                            .iter()
                            .position(|item| item == &track.id)
                            .map(|index| (index, playlist.tracks.len()))
                    })
            });
            let move_earlier = position.is_some_and(|(index, _)| index > 0)
                && secondary_button(ui, "Move earlier in playlist").clicked();
            let move_later = position.is_some_and(|(index, len)| index + 1 < len)
                && secondary_button(ui, "Move later in playlist").clicked();
            if let (Some(id), Some((index, _))) = (playlist_id, position)
                && (move_earlier || move_later)
            {
                if let Some(playlist) = self.data.playlists.iter_mut().find(|p| p.id == id) {
                    let target = if move_earlier { index - 1 } else { index + 1 };
                    playlist.tracks.swap(index, target);
                    self.persist();
                }
                ui.close();
            }
        }
        if !self.data.playlists.is_empty() {
            ui.separator();
            menu_button(ui, "Add to playlist", |ui| {
                for playlist in self.data.playlists.clone() {
                    if secondary_button(ui, playlist.name.as_str()).clicked() {
                        if let Some(p) =
                            self.data.playlists.iter_mut().find(|p| p.id == playlist.id)
                            && !p.tracks.contains(&track.id)
                        {
                            p.tracks.push(track.id.clone());
                            self.persist();
                        }
                        ui.close();
                    }
                }
            });
        }
    }

    fn remove_track(&mut self, id: &str) {
        let Some(track) = self.data.tracks.iter().find(|track| track.id == id) else {
            return;
        };
        let title = track.title.clone();
        if self.current.as_deref() == Some(id) {
            if let Some(player) = self.audio.player.take() {
                player.stop();
            }
            self.audio.duration = Duration::ZERO;
            self.current = None;
        }
        self.data.tracks.retain(|track| track.id != id);
        self.data.recent.retain(|recent| recent != id);
        self.queue.retain(|queued| queued != id);
        for playlist in &mut self.data.playlists {
            playlist.tracks.retain(|track_id| track_id != id);
        }
        self.notes.remove(id);
        self.persist();
        self.status = format!("Removed {title} from OpenMix. The local file was kept.");
    }

    fn queue_page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} tracks queued", self.queue.len()))
                    .size(12.0)
                    .color(theme_soft()),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if !self.queue.is_empty() && danger_button(ui, "Clear queue").clicked() {
                    self.queue.clear();
                }
                if !self.queue.is_empty() && secondary_button(ui, "Shuffle queue").clicked() {
                    shuffle_slice(&mut self.queue);
                }
                if !self.queue.is_empty() && primary_button(ui, "Play queue").clicked() {
                    self.next();
                }
            });
        });
        ui.add_space(14.0);
        if let Some(current) = self.current_track() {
            self.section_header(ui, "NOW PLAYING", None);
            self.track_row(ui, &current, false);
        }
        self.section_header(ui, "UP NEXT", None);
        ui.add_space(6.0);
        if self.queue.is_empty() {
            ui.label(
                RichText::new("Nothing queued. Add tracks from your library or Mixing.")
                    .size(13.0)
                    .color(theme_soft()),
            );
            return;
        }
        let mut remove_index = None;
        let mut move_action = None;
        let queue = self.queue.clone();
        for (index, id) in queue.iter().enumerate() {
            let Some(track) = self.data.tracks.iter().find(|t| &t.id == id).cloned() else {
                continue;
            };
            Frame::new()
                .fill(theme_surface())
                .inner_margin(Margin::symmetric(10, 7))
                .corner_radius(CornerRadius::same(5))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("{:02}", index + 1))
                                .size(11.0)
                                .color(theme_quiet()),
                        );
                        record_art(ui, &track, 36.0);
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&track.title).size(13.0).color(theme_text()));
                            ui.label(RichText::new(&track.artist).size(11.0).color(theme_soft()));
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if icon_button(ui, Icon::Remove, "Remove from queue").clicked() {
                                remove_index = Some(index);
                            }
                            if index + 1 < self.queue.len()
                                && icon_button(ui, Icon::MoveDown, "Move down").clicked()
                            {
                                move_action = Some((index, index + 1));
                            }
                            if index > 0 && icon_button(ui, Icon::MoveUp, "Move up").clicked() {
                                move_action = Some((index, index - 1));
                            }
                        });
                    });
                });
            ui.add_space(3.0);
        }
        if let Some(index) = remove_index {
            self.queue.remove(index);
        }
        if let Some((from, to)) = move_action {
            self.queue.swap(from, to);
        }
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("Profile")
                .size(17.0)
                .strong()
                .color(theme_text()),
        );
        ui.add_space(10.0);
        self.profile_editor(ui);
        ui.add_space(20.0);
        ui.separator();
        ui.add_space(18.0);
        ui.label(
            RichText::new("Appearance")
                .size(17.0)
                .strong()
                .color(theme_text()),
        );
        ui.add_space(12.0);
        let active_theme = self.data.theme;
        let mut chosen_theme = None;
        ui.columns(2, |columns| {
            for (column_index, column) in columns.iter_mut().enumerate() {
                column.vertical(|ui| {
                    for theme in AppTheme::ALL.iter().skip(column_index).step_by(2) {
                        if theme_card(ui, *theme, active_theme == *theme) {
                            chosen_theme = Some(*theme);
                        }
                        ui.add_space(10.0);
                    }
                });
            }
        });
        if let Some(theme) = chosen_theme {
            self.change_theme(ui.ctx(), theme);
        }
        ui.add_space(14.0);
        ui.separator();
        ui.add_space(18.0);
        ui.label(
            RichText::new("NVIDIA NIM")
                .size(17.0)
                .strong()
                .color(theme_text()),
        );
        let mut ai_enabled = self.data.use_openrouter;
        if ui
            .checkbox(&mut ai_enabled, "Use NVIDIA NIM for mix planning")
            .changed()
        {
            self.data.use_openrouter = ai_enabled;
            self.persist();
        }
        ui.add_space(8.0);
        ui.label(RichText::new(MODEL).size(11.0).color(theme_quiet()));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.key_input)
                    .password(!self.key_visible)
                    .hint_text("Paste your NVIDIA API key")
                    .desired_width(360.0),
            );
            let visibility_label = if self.key_visible { "Hide" } else { "Show" };
            if secondary_button(ui, visibility_label).clicked() {
                self.key_visible = !self.key_visible;
            }
        });
        ui.horizontal(|ui| {
            if primary_button(ui, "Save key").clicked() {
                self.key_input = self.key_input.trim().to_owned();
                match write_secret(&self.key_input) {
                    Ok(message) => self.key_status = message,
                    Err(error) => self.key_status = error,
                }
            }
            if ui
                .add_enabled_ui(self.data.use_openrouter, |ui| {
                    primary_button(ui, "Test connection")
                })
                .inner
                .clicked()
            {
                self.openrouter_test();
            }
            if danger_button(ui, "Clear key").clicked() {
                self.key_input.clear();
                self.key_status = match clear_secret() {
                    Ok(()) => "Key cleared from this session and the credential store.".into(),
                    Err(error) => {
                        format!("Key cleared for this session; credential store: {error}")
                    }
                };
            }
        });
        ui.label(
            RichText::new(&self.key_status)
                .size(11.0)
                .color(theme_soft()),
        );
        ui.add_space(22.0);
        ui.separator();
        ui.add_space(12.0);
        ui.label(
            RichText::new("MusicBrainz")
                .size(17.0)
                .strong()
                .color(theme_text()),
        );
        let mut metadata_enabled = self.data.use_musicbrainz;
        if ui
            .checkbox(&mut metadata_enabled, "Look up release metadata on import")
            .changed()
        {
            self.data.use_musicbrainz = metadata_enabled;
            self.persist();
        }
        ui.add_enabled_ui(
            self.data.use_musicbrainz && !self.data.tracks.is_empty(),
            |ui| {
                if secondary_button(ui, "Retry metadata lookup for library").clicked() {
                    let tracks = self.data.tracks.clone();
                    self.lookup_metadata(tracks);
                }
            },
        );
        ui.add_space(30.0);
        ui.separator();
        ui.add_space(18.0);
        ui.label(
            RichText::new("Playback")
                .size(17.0)
                .strong()
                .color(theme_text()),
        );
        ui.horizontal(|ui| {
            ui.label("Crossfade length");
            ui.label(RichText::new("6 to 22 seconds").color(theme_soft()));
        });
        ui.label(RichText::new("Drop timing uses whole-track energy analysis and phrase-aligned BPM. No lyrics are fetched or sent. If online lookups or NVIDIA NIM are unavailable, the local audio plan still works.").size(12.0).color(theme_soft()));
        ui.add_space(24.0);
        ui.label(
            RichText::new("Keyboard shortcuts")
                .size(15.0)
                .strong()
                .color(theme_text()),
        );
        for (key, action) in [
            ("Space", "Play or pause"),
            ("Ctrl K", "Search your library"),
            ("Ctrl F", "Filter the library"),
            ("Alt →", "Next track"),
            ("Alt ←", "Previous track"),
            ("Ctrl S", "Shuffle"),
            ("Ctrl R", "Cycle repeat"),
        ] {
            ui.horizontal(|ui| {
                ui.label(RichText::new(key).monospace().color(theme_text()));
                ui.add_space(16.0);
                ui.label(RichText::new(action).color(theme_soft()));
            });
        }
        ui.add_space(26.0);
        ui.separator();
        ui.add_space(12.0);
    }

    fn profile_editor(&mut self, ui: &mut egui::Ui) {
        Frame::new()
            .fill(theme_surface())
            .stroke(Stroke::new(1.0, theme_edge()))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::symmetric(16, 14))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    profile_avatar(ui, self.profile_icon_key(), 58.0, false);
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Your listener profile")
                                .size(15.0)
                                .strong()
                                .color(theme_text()),
                        );
                        ui.label(
                            RichText::new("Personalize how OpenMix greets you.")
                                .size(11.5)
                                .color(theme_soft()),
                        );
                    });
                });
                ui.add_space(14.0);
                ui.label(
                    RichText::new("DISPLAY NAME")
                        .size(10.0)
                        .strong()
                        .color(theme_quiet()),
                );
                ui.add_space(5.0);
                let mut name = self.data.user_name.clone();
                let name_response = ui.add(
                    egui::TextEdit::singleline(&mut name)
                        .hint_text("OpenMix Listener")
                        .desired_width((ui.available_width() - 2.0).max(180.0)),
                );
                if name_response.changed() {
                    self.data.user_name = name;
                    self.persist();
                }
                ui.add_space(14.0);
                ui.label(
                    RichText::new("PROFILE ICON")
                        .size(10.0)
                        .strong()
                        .color(theme_quiet()),
                );
                ui.add_space(7.0);
                egui::Grid::new("profile_icon_grid")
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        for (index, (key, label)) in [
                            ("user", "Person"),
                            ("headphones", "Headphones"),
                            ("waveform", "Waveform"),
                            ("star", "Star"),
                            ("moon", "Moon"),
                            ("flame", "Flame"),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            if index % 3 == 0 && index > 0 {
                                ui.end_row();
                            }
                            let selected = if self.data.user_icon.is_empty() {
                                key == "user"
                            } else {
                                self.data.user_icon == key
                            };
                            ui.vertical(|ui| {
                                if profile_avatar(ui, key, 46.0, selected).clicked() {
                                    self.data.user_icon = key.to_owned();
                                    self.persist();
                                }
                                ui.label(RichText::new(label).size(10.5).color(if selected {
                                    theme_red()
                                } else {
                                    theme_soft()
                                }));
                            });
                        }
                    });
            });
    }

    fn profile_window(&mut self, ctx: &egui::Context) {
        if !self.profile_open {
            return;
        }
        let mut open = true;
        let window = egui::Window::new("Your profile")
            .collapsible(false)
            .resizable(false)
            .open(&mut open);
        #[cfg(target_os = "android")]
        let window = {
            let inset_bottom = self.android_insets.bottom.round();
            window.anchor(egui::Align2::CENTER_BOTTOM, [0.0, -(inset_bottom + 12.0)])
        };
        window.show(ctx, |ui| {
            let screen = ctx.input(|i| i.viewport_rect());
            let sheet_width = if cfg!(target_os = "android") {
                (screen.width() - 36.0).clamp(260.0, 420.0)
            } else {
                360.0
            };
            ui.set_min_width(sheet_width);
            ui.set_max_width(sheet_width);
            egui::ScrollArea::vertical()
                .id_salt("profile_window_scroll")
                .max_height((screen.height() * 0.72).max(260.0))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        profile_avatar(ui, self.profile_icon_key(), 64.0, true);
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(self.profile_name())
                                    .size(18.0)
                                    .strong()
                                    .color(theme_text()),
                            );
                            ui.label(
                                RichText::new("Your profile stays on this device.")
                                    .size(11.5)
                                    .color(theme_soft()),
                            );
                        });
                    });
                    ui.add_space(12.0);
                    self.profile_editor(ui);
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if primary_button(ui, "All settings").clicked() {
                            self.page = Page::Settings;
                            self.profile_open = false;
                        }
                        if secondary_button(ui, "Done").clicked() {
                            self.profile_open = false;
                        }
                    });
                });
        });
        if !open {
            self.profile_open = false;
        }
    }

    fn section_header(&mut self, ui: &mut egui::Ui, title: &str, action: Option<(&str, Page)>) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).size(18.0).strong().color(theme_text()));
            if let Some((name, page)) = action {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.link(RichText::new(name).color(theme_soft())).clicked() {
                        self.page = page;
                    }
                });
            }
        });
    }

    fn empty_state(&mut self, ui: &mut egui::Ui, title: &str, detail: &str) {
        let height = ui.available_size_before_wrap().y;
        ui.set_min_height(height);
        ui.add_space(((height - 250.0) * 0.5).max(20.0));
        ui.vertical_centered(|ui| {
            empty_library_mark(ui);
            ui.add_space(12.0);
            ui.label(RichText::new(title).size(21.0).strong().color(theme_text()));
            ui.add_space(5.0);
            ui.label(RichText::new(detail).size(13.0).color(theme_soft()));
            ui.add_space(16.0);
            if self.page == Page::Playlist {
                if primary_button(ui, "Browse library").clicked() {
                    self.page = Page::Library;
                }
            } else if primary_button(ui, "Add local music").clicked() {
                self.import_files();
            }
        });
    }

    fn toggle_like(&mut self, id: &str) {
        if let Some(track) = self.data.tracks.iter_mut().find(|t| t.id == id) {
            track.liked = !track.liked;
        }
        self.persist();
    }

    /// One-shot keyboard-focus gate for sheet dialogs. On Android the
    /// focus request must be issued exactly once per dialog; requesting
    /// it every frame makes the IME open/close in a visible loop.
    fn sheet_request_focus(&mut self) -> bool {
        #[cfg(target_os = "android")]
        if self.sheet_focus_pending {
            self.sheet_focus_pending = false;
            true
        } else {
            false
        }
        #[cfg(not(target_os = "android"))]
        {
            true
        }
    }

    /// Android bottom sheet listing the tracks MediaStore finds on this
    /// device, with an import button per row.
    #[cfg(target_os = "android")]
    fn music_sheet_window(&mut self, ctx: &egui::Context) {
        if !self.device_music_open {
            return;
        }
        let screen = ctx.input(|i| i.viewport_rect());
        let width = (screen.width() - 32.0).max(240.0);
        let inset_bottom = self.android_insets.bottom.round();
        let mut open = true;
        let mut window = egui::Window::new("Device music")
            .collapsible(false)
            .resizable(false)
            .open(&mut open);
        window = window.anchor(egui::Align2::CENTER_BOTTOM, [0.0, -(inset_bottom + 12.0)]);
        window.show(ctx, |ui| {
            ui.set_min_width(width);
            if self.device_music_loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Reading music from this device…");
                });
            } else if self.device_music.is_empty() {
                ui.label(
                    RichText::new(
                        "No MP3 or WAV files were found on this device yet.\nCopy music over, then reopen this list.",
                    )
                    .color(theme_quiet()),
                );
            } else {
                ui.label(
                    RichText::new(format!(
                        "{} track(s) found — imported tracks are analyzed and added to your library.",
                        self.device_music.len()
                    ))
                    .size(12.0)
                    .color(theme_quiet()),
                );
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .id_salt("device_music_list")
                    .max_height(screen.height() * 0.55)
                    .show(ui, |ui| {
                        for item in self.device_music.clone() {
                            ui.add_space(2.0);
                            Frame::new()
                                .fill(theme_surface())
                                .corner_radius(CornerRadius::same(8))
                                .inner_margin(Margin::symmetric(10, 8))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.vertical(|ui| {
                                            ui.set_min_width(width - 140.0);
                                            ui.label(
                                                RichText::new(&item.title)
                                                    .strong()
                                                    .color(theme_text()),
                                            );
                                            ui.label(
                                                RichText::new(format!(
                                                    "{} · {}",
                                                    item.artist,
                                                    format_time((item.duration_ms / 1000).max(0) as u32)
                                                ))
                                                .size(11.0)
                                                .color(theme_quiet()),
                                            );
                                        });
                                        if secondary_button(ui, "Import").clicked() {
                                            self.import_device_track(item);
                                        }
                                    });
                                });
                        }
                    });
            }
        });
        if !open {
            self.device_music_open = false;
        }
    }

    /// Desktop uses a centered dialog window; Android anchors it to the
    /// bottom of the screen (a bottom sheet) so it rides above the on-screen
    /// keyboard as the viewport resizes, instead of a small corner popup.
    fn playlist_window(&mut self, ctx: &egui::Context) {
        // Reset the one-shot keyboard-focus request so a freshly opened
        // sheet grabs focus exactly once.
        #[cfg(target_os = "android")]
        if !self.playlist_dialog {
            self.sheet_focus_pending = true;
        }
        if self.playlist_dialog {
            let mut open = true;
            #[cfg_attr(not(target_os = "android"), allow(unused_mut))]
            let mut window = egui::Window::new(if self.playlist_edit.is_some() {
                "Rename playlist"
            } else {
                "New playlist"
            })
            .collapsible(false)
            .resizable(false)
            .open(&mut open);
            let screen_width = ctx.input(|i| i.viewport_rect().width());
            let sheet_width = if cfg!(target_os = "android") {
                (screen_width - 40.0).max(240.0)
            } else {
                300.0
            };
            #[cfg(target_os = "android")]
            {
                // Anchor to bottom so it stays above the keyboard when it appears
                window = window.anchor(
                    egui::Align2::CENTER_BOTTOM,
                    [0.0, -(self.android_insets.bottom.round() + 12.0)],
                );
                // Make window auto-size to content so it doesn't overlap keyboard
                window = window.auto_sized();
            }
            window.show(ctx, |ui| {
                if cfg!(target_os = "android") {
                    ui.set_min_width(sheet_width - 16.0);
                }
                ui.label("Give this playlist a name.");
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.playlist_name)
                        .desired_width(sheet_width - 20.0),
                );
                if self.sheet_request_focus() {
                    response.request_focus();
                }
                ui.horizontal(|ui| {
                    if secondary_button(ui, "Cancel").clicked() {
                        self.playlist_dialog = false;
                    }
                    if ui
                        .add_enabled_ui(!self.playlist_name.trim().is_empty(), |ui| {
                            primary_button(ui, "Save playlist")
                        })
                        .inner
                        .clicked()
                    {
                        let name = self.playlist_name.trim().to_owned();
                        if let Some(id) = self.playlist_edit.take() {
                            if let Some(p) = self.data.playlists.iter_mut().find(|p| p.id == id) {
                                p.name = name;
                                if !self.selected_tracks.is_empty() {
                                    for track_id in &self.selected_tracks {
                                        if !p.tracks.contains(track_id) {
                                            p.tracks.push(track_id.clone());
                                        }
                                    }
                                    self.selected_tracks.clear();
                                }
                            }
                        } else {
                            // Unique-per-render ID: random component avoids
                            // collisions when playlists are created,
                            // deleted, and recreated in the same second.
                            let id =
                                format!("playlist-{}-{}", random_u64(), self.data.playlists.len());
                            let mut tracks = Vec::new();
                            if !self.selected_tracks.is_empty() {
                                tracks = self.selected_tracks.iter().cloned().collect();
                            }
                            self.data.playlists.push(Playlist {
                                id: id.clone(),
                                name,
                                tracks,
                                created: now_seconds(),
                            });
                            // The selection was consumed into the playlist.
                            self.selected_tracks.clear();
                            self.active_playlist = Some(id);
                            self.page = Page::Playlist;
                            self.sort = SortMode::PlaylistOrder;
                            self.sort_ascending = true;
                        }
                        self.persist();
                        self.playlist_dialog = false;
                    }
                });
            });
            if !open {
                self.playlist_dialog = false;
            }
        }
        if let Some(id) = self.playlist_delete.clone() {
            #[cfg_attr(not(target_os = "android"), allow(unused_mut))]
            let mut delete_window = egui::Window::new("Delete playlist?")
                .collapsible(false)
                .resizable(false);
            #[cfg(target_os = "android")]
            {
                delete_window = delete_window.anchor(
                    egui::Align2::CENTER_BOTTOM,
                    [0.0, -(self.android_insets.bottom.round() + 12.0)],
                );
            }
            delete_window.show(ctx, |ui| {
                ui.label("This removes the playlist and keeps your local files in the library.");
                ui.horizontal(|ui| {
                    if secondary_button(ui, "Cancel").clicked() {
                        self.playlist_delete = None;
                    }
                    if danger_button(ui, "Delete playlist").clicked() {
                        self.data.playlists.retain(|p| p.id != id);
                        if self.active_playlist.as_deref() == Some(&id) {
                            self.active_playlist = None;
                            self.page = Page::Library;
                        }
                        self.playlist_delete = None;
                        self.persist();
                    }
                });
            });
        }
    }

    fn notification_toast(&mut self, ctx: &egui::Context) {
        let Some(toast) = &self.toast else {
            return;
        };
        if Instant::now() >= toast.expires_at {
            self.toast = None;
            return;
        }
        let toast = Toast {
            title: toast.title.clone(),
            message: toast.message.clone(),
            error: toast.error,
            expires_at: toast.expires_at,
        };
        let accent = if toast.error {
            theme_red()
        } else {
            Color32::from_rgb(119, 190, 143)
        };
        egui::Area::new(egui::Id::unique("openmix_notification"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_TOP, Vec2::new(-18.0, 18.0))
            .show(ctx, |ui| {
                Frame::new()
                    .fill(theme_surface_2())
                    .stroke(Stroke::new(1.0, theme_edge()))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::symmetric(14, 11))
                    .show(ui, |ui| {
                        ui.set_max_width(380.0);
                        ui.horizontal(|ui| {
                            let (dot, _) =
                                ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
                            ui.painter().circle_filled(dot.center(), 4.0, accent);
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new(&toast.title)
                                        .size(13.0)
                                        .strong()
                                        .color(theme_text()),
                                );
                                ui.label(
                                    RichText::new(&toast.message).size(11.0).color(theme_soft()),
                                );
                            });
                        });
                    });
            });
    }
}
impl eframe::App for OpenMix {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.receive_messages();
        self.update_shortcuts(&ctx);
        #[cfg(target_os = "android")]
        self.update_android_session(&ctx);
        if self.current.is_some() && self.audio.ended() {
            if self.repeat == 1 {
                if let Some(id) = self.current.clone() {
                    self.play_id(&id);
                }
            } else if !self.queue.is_empty() || self.repeat == 2 {
                self.next();
            }
        }
        self.player_bar(ui);
        #[cfg(not(target_os = "android"))]
        self.left_sidebar(ui);
        self.right_now_panel(ui);
        self.content(ui);
        #[cfg(target_os = "android")]
        self.render_android_drawer(&ctx);
        #[cfg(target_os = "android")]
        self.music_sheet_window(&ctx);
        self.playlist_window(&ctx);
        self.profile_window(&ctx);
        self.fullscreen_player(&ctx);
        self.notification_toast(&ctx);
        ctx.request_repaint_after(Duration::from_millis(33));
    }
}

fn record_art(ui: &mut egui::Ui, track: &Track, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::from(4.0), Color32::from_rgb(36, 38, 41));
    let texture = track.cover_art_path.as_ref().and_then(|path| {
        COVER_TEXTURES.with(|textures| {
            let mut textures = textures.borrow_mut();
            match textures.get(path) {
                Some(CoverArtCache::Texture(texture)) => return Some(texture.clone()),
                Some(CoverArtCache::Failed(at)) if at.elapsed() < COVER_RETRY_INTERVAL => {
                    return None;
                }
                _ => {}
            }
            let loaded = image::open(path).ok().map(|image| {
                let rgba = image.to_rgba8();
                let dimensions = [rgba.width() as usize, rgba.height() as usize];
                let color_image =
                    egui::ColorImage::from_rgba_unmultiplied(dimensions, rgba.as_raw());
                ui.ctx().load_texture(
                    format!("cover:{}", path.display()),
                    color_image,
                    egui::TextureOptions::LINEAR,
                )
            });
            textures.insert(
                path.clone(),
                match loaded.as_ref() {
                    Some(texture) => CoverArtCache::Texture(texture.clone()),
                    None => CoverArtCache::Failed(Instant::now()),
                },
            );
            loaded
        })
    });
    if let Some(texture) = texture {
        painter.image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        return;
    }
    let center = rect.center();
    let r = size * 0.36;
    painter.circle_stroke(center, r, Stroke::new(1.0, Color32::from_rgb(83, 85, 87)));
    painter.circle_stroke(
        center,
        r * 0.61,
        Stroke::new(1.0, Color32::from_rgb(65, 67, 69)),
    );
    painter.circle_filled(center, size * 0.065, theme_red());
}

fn mix_score(from: &Track, to: &Track) -> f32 {
    let bpm_gap = if from.bpm == 0 || to.bpm == 0 {
        10.0
    } else {
        let a = (from.bpm as f32 - to.bpm as f32).abs();
        a.min((from.bpm as f32 - 2.0 * to.bpm as f32).abs())
            .min((2.0 * from.bpm as f32 - to.bpm as f32).abs())
    };
    bpm_gap * 1.7 + (from.energy - to.energy).abs() * 20.0
}

fn bpm_label(bpm: u16) -> String {
    if bpm == 0 {
        "—".into()
    } else {
        bpm.to_string()
    }
}
fn format_time(seconds: u32) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}
fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Minimal xorshift64* PRNG. Seeded from the clock once, then advanced on
/// every call so repeated calls within the same second (fast next presses,
/// quick shuffles) still give independent results.
static RNG_STATE: AtomicU64 = AtomicU64::new(0);

fn random_u64() -> u64 {
    let mut state = RNG_STATE.load(Ordering::Relaxed);
    if state == 0 {
        state = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        if state == 0 {
            state = 0x9E37_79B9_7F4A_7C15;
        }
    }
    state ^= state >> 12;
    state ^= state << 25;
    state ^= state >> 27;
    RNG_STATE.store(state, Ordering::Relaxed);
    state.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

fn shuffle_slice(ids: &mut [String]) {
    for i in (1..ids.len()).rev() {
        let j = (random_u64() as usize) % (i + 1);
        ids.swap(i, j);
    }
}

#[derive(Serialize, Deserialize)]
struct MixInfo {
    status: String,
    total_tracks: usize,
    completed_tracks: usize,
    track_ids: Vec<String>,
    output_path: PathBuf,
    updated_at: u64,
    #[serde(default)]
    error: Option<String>,
}

/// Progress for the in-flight "mix all" render, written to the app's cache
/// area so the render's state lives on disk rather than only in memory: a
/// crash or restart mid-render leaves both this file and the partial WAV
/// behind instead of losing everything.
fn mixinfo_path() -> PathBuf {
    if let Some(root) = platform::app_data_dir() {
        return root.join("cache").join("mixinfo.json");
    }
    ProjectDirs::from("studio", "openmix", "OpenMix")
        .map(|dirs| dirs.cache_dir().join("mixinfo.json"))
        .unwrap_or_else(|| PathBuf::from("mixinfo.json"))
}

fn write_mixinfo(info: &MixInfo) {
    let Ok(json) = serde_json::to_vec_pretty(info) else {
        return;
    };
    if let Some(parent) = mixinfo_path().parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(mixinfo_path(), json);
}

#[cfg(not(target_os = "android"))]
pub fn run_desktop() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1480.0, 940.0])
            .with_min_inner_size([980.0, 680.0])
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "OpenMix",
        options,
        Box::new(|cc| Ok(Box::new(OpenMix::new(cc)))),
    )
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "C" fn android_main(app: AndroidApp) {
    let bridge = android::AndroidBridge::new(app.clone());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default(),
        android_app: Some(app),
        ..Default::default()
    };
    let result = eframe::run_native(
        "OpenMix",
        options,
        Box::new(move |cc| Ok(Box::new(OpenMix::new(cc, Some(bridge))))),
    );
    if let Err(error) = result {
        eprintln!("OpenMix could not start: {error}");
    }
}

#[cfg(not(target_os = "android"))]
fn app_icon() -> Arc<egui::IconData> {
    let size = 64u32;
    let radius = 14.0f32;
    let bars = [(17, 17), (25, 29), (33, 39), (41, 27), (49, 17)];
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let dx = if px < radius {
                radius - px
            } else if px > 64.0 - radius {
                px - (64.0 - radius)
            } else {
                0.0
            };
            let dy = if py < radius {
                radius - py
            } else if py > 64.0 - radius {
                py - (64.0 - radius)
            } else {
                0.0
            };
            let inside = dx * dx + dy * dy <= radius * radius;
            let wave = bars.iter().any(|(bar_x, height)| {
                x >= *bar_x && x < *bar_x + 4 && (py - 32.0).abs() <= *height as f32 * 0.5
            });
            let color = if wave { theme_red() } else { theme_bg() };
            rgba.extend_from_slice(&[
                color.r(),
                color.g(),
                color.b(),
                if inside { 255 } else { 0 },
            ]);
        }
    }
    Arc::new(egui::IconData {
        rgba,
        width: size,
        height: size,
    })
}
