use std::{fs, path::PathBuf, sync::Mutex, thread, time::Duration};

use directories::ProjectDirs;

use crate::model::{RemoteMetadata, Track};

const MUSICBRAINZ_PROJECT_URL: &str = "https://github.com/matejpcs/openmix";
static MUSICBRAINZ_RATE_LIMITER: Mutex<()> = Mutex::new(());

fn musicbrainz_user_agent() -> String {
    format!(
        "OpenMix/{} ({MUSICBRAINZ_PROJECT_URL})",
        env!("CARGO_PKG_VERSION")
    )
}

pub(crate) fn musicbrainz_metadata(track: &Track) -> Result<Option<RemoteMetadata>, String> {
    let _rate_lock = MUSICBRAINZ_RATE_LIMITER
        .lock()
        .map_err(|_| "Metadata lookup is temporarily unavailable.".to_owned())?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|error| format!("Could not create network client: {error}"))?;
    let escape = |text: &str| text.replace('\\', "\\\\").replace('"', "\\\"");
    // Search by title first. A producer may not be credited as the recording's
    // artist in MusicBrainz, and constraining both fields hid otherwise valid hits.
    let query = format!("recording:\"{}\"", escape(&track.title));
    let search_url = format!(
        "https://musicbrainz.org/ws/2/recording/?query={}&fmt=json&limit=100",
        encode_query_component(&query)
    );
    let search = musicbrainz_json(&client, &search_url)?;
    let local_title = normalize_music_text(&track.title);
    let local_artist = normalize_music_text(&track.artist);
    let recordings = search
        .get("recordings")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "MusicBrainz response did not contain recordings.".to_owned())?;
    let exact_title_count = recordings
        .iter()
        .filter(|item| {
            item.get("title")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|title| normalize_music_text(title) == local_title)
        })
        .count();
    let candidate = recordings
        .iter()
        .filter_map(|item| {
            let title = item.get("title")?.as_str()?;
            if normalize_music_text(title) != local_title {
                return None;
            }
            let credited = item
                .get("artist-credit")
                .and_then(serde_json::Value::as_array)
                .map(|credits| {
                    credits
                        .iter()
                        .filter_map(|credit| credit.get("name").and_then(|n| n.as_str()))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            let artist_match = local_artist == "localfile"
                || normalize_music_text(&credited).contains(&local_artist)
                || local_artist.contains(&normalize_music_text(&credited));
            let duration_delta = item
                .get("length")
                .and_then(serde_json::Value::as_u64)
                .map(|ms| ms.abs_diff(u64::from(track.duration) * 1000));
            // Prefer a credited artist, otherwise use a close duration to handle
            // producer-versus-performer credits. A unique exact title can stand
            // on its own when MusicBrainz has no duration for the recording.
            (artist_match
                || duration_delta.is_some_and(|delta| delta <= 90_000)
                || (duration_delta.is_none() && exact_title_count == 1))
                .then_some((item, artist_match, duration_delta.unwrap_or(u64::MAX)))
        })
        .min_by_key(|(_, artist_match, duration_delta)| (!*artist_match, *duration_delta));
    let Some((candidate, _, _)) = candidate else {
        return Ok(None);
    };
    let Some(id) = candidate.get("id").and_then(serde_json::Value::as_str) else {
        return Err("MusicBrainz result did not include a recording ID.".into());
    };
    let id = id.to_owned();
    let first_date = candidate
        .get("first-release-date")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    thread::sleep(Duration::from_secs(1));
    let detail = musicbrainz_json(
        &client,
        &format!("https://musicbrainz.org/ws/2/recording/{id}?fmt=json&inc=releases"),
    )
    .ok();
    thread::sleep(Duration::from_secs(1));
    let album = detail
        .as_ref()
        .and_then(|data| data.get("releases"))
        .and_then(serde_json::Value::as_array)
        .and_then(|releases| releases.first())
        .and_then(|release| release.get("title"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            candidate
                .get("releases")?
                .as_array()?
                .first()?
                .get("title")?
                .as_str()
                .map(str::to_owned)
        });
    let release_id = detail
        .as_ref()
        .and_then(|data| data.get("releases"))
        .and_then(serde_json::Value::as_array)
        .and_then(|releases| releases.first())
        .and_then(|release| release.get("id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            candidate
                .get("releases")?
                .as_array()?
                .first()?
                .get("id")?
                .as_str()
                .map(str::to_owned)
        });
    let cover_art_path = release_id.and_then(|release_id| {
        thread::sleep(Duration::from_secs(1));
        download_cover_art(&client, &release_id).ok().flatten()
    });
    Ok(Some(RemoteMetadata {
        release_year: first_date.and_then(|date| date.get(..4).map(str::to_owned)),
        album,
        musicbrainz_id: id,
        cover_art_path,
    }))
}

fn cover_art_dir() -> PathBuf {
    ProjectDirs::from("studio", "openmix", "OpenMix")
        .map(|dirs| dirs.cache_dir().join("cover-art"))
        .unwrap_or_else(|| PathBuf::from("cover-art"))
}

/// Downloads a release's front cover art from the Cover Art Archive
/// (https://coverartarchive.org), which is keyed by MusicBrainz release ID.
/// Returns `Ok(None)` when the release has no cover art on file, rather than
/// treating a 404 as an error.
pub(crate) fn download_cover_art(
    client: &reqwest::blocking::Client,
    release_id: &str,
) -> Result<Option<PathBuf>, String> {
    let dir = cover_art_dir();
    for extension in ["jpg", "png"] {
        let cached = dir.join(format!("{release_id}.{extension}"));
        if cached.is_file() {
            return Ok(Some(cached));
        }
    }
    let url = format!("https://coverartarchive.org/release/{release_id}/front");
    for attempt in 0..4 {
        let response = match client
            .get(&url)
            .header("User-Agent", musicbrainz_user_agent())
            .send()
        {
            Ok(response) => response,
            Err(_) if attempt < 3 => {
                thread::sleep(Duration::from_secs(2_u64 << attempt));
                continue;
            }
            Err(error) => {
                return Err(format!("Cover art request failed after retries: {error}"));
            }
        };
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if status.is_success() {
            let extension = match response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
            {
                Some(content_type) if content_type.contains("png") => "png",
                _ => "jpg",
            };
            let bytes = response
                .bytes()
                .map_err(|error| format!("Could not read cover art response: {error}"))?;
            fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
            let path = dir.join(format!("{release_id}.{extension}"));
            fs::write(&path, &bytes).map_err(|error| error.to_string())?;
            return Ok(Some(path));
        }
        if (status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error())
            && attempt < 3
        {
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .map(|seconds| seconds.clamp(2, 30));
            thread::sleep(Duration::from_secs(retry_after.unwrap_or(2_u64 << attempt)));
            continue;
        }
        return Err(format!("Cover art request returned HTTP {status}."));
    }
    Err("Cover art request failed after retries. Try again shortly.".into())
}

fn musicbrainz_json(
    client: &reqwest::blocking::Client,
    url: &str,
) -> Result<serde_json::Value, String> {
    for attempt in 0..4 {
        let response = match client
            .get(url)
            .header("User-Agent", musicbrainz_user_agent())
            .send()
        {
            Ok(response) => response,
            Err(_) if attempt < 3 => {
                thread::sleep(Duration::from_secs(2_u64 << attempt));
                continue;
            }
            Err(error) => return Err(format!("Network request failed after retries: {error}")),
        };
        let status = response.status();
        if status.is_success() {
            return response
                .json()
                .map_err(|error| format!("Could not read MusicBrainz response: {error}"));
        }
        if (status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error())
            && attempt < 3
        {
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .map(|seconds| seconds.clamp(2, 30));
            thread::sleep(Duration::from_secs(retry_after.unwrap_or(2_u64 << attempt)));
            continue;
        }
        return Err(format!(
            "MusicBrainz returned HTTP {status} after retries. It may be rate-limiting requests; try again shortly."
        ));
    }
    Err("MusicBrainz request failed after retries. Try again shortly.".into())
}

fn normalize_music_text(text: &str) -> String {
    text.chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn encode_query_component(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}
