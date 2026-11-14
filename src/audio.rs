use std::{
    fs::File,
    io::{Seek, SeekFrom, Write},
    path::Path,
    time::Duration,
};

use rodio::{Decoder, Source};
use symphonia::core::{
    formats::FormatOptions,
    io::MediaSourceStream,
    meta::{MetadataOptions, StandardTagKey, Tag},
    probe::Hint,
};

pub const OUTPUT_RATE: u32 = 44_100;
pub const MAX_TRACK_SECONDS: u64 = 12 * 60;

#[derive(Clone, Debug, Default)]
pub struct Analysis {
    pub duration_seconds: u32,
    pub bpm: u16,
    pub energy: f32,
    pub rise_seconds: u32,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
}

#[derive(Clone, Debug)]
pub struct StereoPcm {
    pub samples: Vec<f32>,
    pub duration: Duration,
}

#[derive(Clone, Debug)]
pub struct TransitionPlan {
    pub entry_lead_seconds: u32,
    pub fade_seconds: u32,
    pub incoming_start_seconds: u32,
    pub early_gain: f32,
}

pub fn analyze_file(path: &Path) -> Result<Analysis, String> {
    let tags = read_tags(path);
    let file = File::open(path).map_err(|e| format!("Could not open audio: {e}"))?;
    let decoder = Decoder::try_from(file).map_err(|e| format!("Could not decode audio: {e}"))?;
    let channels = decoder.channels().get() as usize;
    let rate = decoder.sample_rate().get();
    // Some MP3s (no Xing/VBRI header) report no duration, so fall back to
    // measuring the decoded frame count instead of rejecting the file.
    let reported = decoder.total_duration();
    // Keep a compact 100 Hz RMS envelope for the entire song. This lets drop/rise
    // analysis see late sections without retaining minutes of decoded PCM.
    let hop_frames = (rate / 100).max(1) as usize;
    let mut samples = decoder;
    let mut envelope = Vec::new();
    let mut decoded_frames = 0u64;
    loop {
        let mut square_sum = 0.0f32;
        let mut frames = 0usize;
        for _ in 0..hop_frames {
            let mut frame_sum = 0.0f32;
            let mut frame_channels = 0usize;
            for _ in 0..channels {
                if let Some(sample) = samples.next() {
                    frame_sum += sample * sample;
                    frame_channels += 1;
                }
            }
            if frame_channels == 0 {
                break;
            }
            square_sum += frame_sum / frame_channels as f32;
            frames += 1;
        }
        if frames == 0 {
            break;
        }
        decoded_frames += frames as u64;
        envelope.push((square_sum / frames as f32).sqrt());
        if frames < hop_frames {
            break;
        }
    }
    let duration = reported
        .unwrap_or_else(|| Duration::from_secs_f64(decoded_frames as f64 / f64::from(rate.max(1))));
    if duration < Duration::from_secs(1) || duration > Duration::from_secs(MAX_TRACK_SECONDS) {
        return Err(format!(
            "Tracks must be between 1 second and {} minutes.",
            MAX_TRACK_SECONDS / 60
        ));
    }
    let (bpm, energy, rise_seconds) = analyze_envelope(&envelope);
    Ok(Analysis {
        duration_seconds: duration.as_secs() as u32,
        bpm,
        energy,
        rise_seconds,
        title: tags.0,
        artist: tags.1,
        album: tags.2,
    })
}

fn read_tags(path: &Path) -> (Option<String>, Option<String>, Option<String>) {
    let Ok(file) = File::open(path) else {
        return (None, None, None);
    };
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(extension);
    }
    let Ok(mut probed) = symphonia::default::get_probe().format(
        &hint,
        stream,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    ) else {
        return (None, None, None);
    };

    let mut tags = (None, None, None);
    if let Some(metadata) = probed.metadata.get()
        && let Some(revision) = metadata.current()
    {
        collect_tags(revision.tags(), &mut tags);
    }
    if let Some(revision) = probed.format.metadata().current() {
        collect_tags(revision.tags(), &mut tags);
    }
    tags
}

fn collect_tags(tags: &[Tag], output: &mut (Option<String>, Option<String>, Option<String>)) {
    for tag in tags {
        let slot = match tag.std_key {
            Some(StandardTagKey::TrackTitle) => &mut output.0,
            Some(StandardTagKey::Artist | StandardTagKey::AlbumArtist) => &mut output.1,
            Some(StandardTagKey::Album) => &mut output.2,
            _ => continue,
        };
        if slot.is_none() {
            let value = tag.value.to_string().trim().to_owned();
            if !value.is_empty() {
                *slot = Some(value);
            }
        }
    }
}

pub fn decode_stereo(path: &Path) -> Result<StereoPcm, String> {
    let file = File::open(path).map_err(|e| format!("Could not open audio: {e}"))?;
    let decoder = Decoder::try_from(file).map_err(|e| format!("Could not decode audio: {e}"))?;
    let channels = decoder.channels().get() as usize;
    let rate = decoder.sample_rate().get();
    // Duration may be unknown for headerless MP3s; the decoded frame count
    // is the authoritative input length, so check the cap after decoding.
    let interleaved: Vec<f32> = decoder.collect();
    let frames = interleaved.len() / channels;
    if frames == 0 {
        return Err("The file contains no audio frames.".to_owned());
    }
    let input_duration = Duration::from_secs_f64(frames as f64 / f64::from(rate.max(1)));
    if input_duration > Duration::from_secs(MAX_TRACK_SECONDS) {
        return Err(format!(
            "Tracks are limited to {} minutes for transition processing.",
            MAX_TRACK_SECONDS / 60
        ));
    }
    let output_frames = ((frames as f64) * (f64::from(OUTPUT_RATE) / f64::from(rate))) as usize;
    let mut stereo = Vec::with_capacity(output_frames * 2);
    for out_frame in 0..output_frames {
        let position = out_frame as f64 * f64::from(rate) / f64::from(OUTPUT_RATE);
        let index = (position as usize).min(frames - 1);
        let next = (index + 1).min(frames - 1);
        let fraction = (position - index as f64) as f32;
        let channel_sample = |channel: usize| {
            let c = channel.min(channels - 1);
            let a = interleaved[index * channels + c];
            let b = interleaved[next * channels + c];
            a + (b - a) * fraction
        };
        stereo.push(channel_sample(0));
        stereo.push(channel_sample(if channels > 1 { 1 } else { 0 }));
    }
    Ok(StereoPcm {
        samples: stereo,
        duration: Duration::from_secs_f64(output_frames as f64 / f64::from(OUTPUT_RATE)),
    })
}

pub fn trim_start(pcm: &StereoPcm, position: Duration) -> Result<StereoPcm, String> {
    let frames = pcm.samples.len() / 2;
    let start = (position.as_secs_f64() * f64::from(OUTPUT_RATE)) as usize;
    if frames == 0 || start >= frames {
        return Err("The outgoing track has already ended.".into());
    }
    let samples = pcm.samples[start * 2..].to_vec();
    let remaining_frames = frames - start;
    Ok(StereoPcm {
        samples,
        duration: Duration::from_secs_f64(remaining_frames as f64 / f64::from(OUTPUT_RATE)),
    })
}

/// Computes the full crossfaded buffer, and also reports `fade_start` in
/// frames: the point after which no sample in the result can ever be
/// changed by a *later* transition. Everything before `fade_start` is
/// playing the outgoing track at full gain (the tail blend only ever adds a
/// small amount of the incoming track in at `early_gain`, which is already
/// fully resolved here since both tracks are in hand), so a streaming
/// caller can flush it to disk immediately instead of holding it in memory.
fn blend(
    a: &[f32],
    incoming: &StereoPcm,
    from_bpm: u16,
    to_bpm: u16,
    plan: &TransitionPlan,
) -> (Vec<f32>, usize) {
    let incoming_frames = incoming.samples.len() / 2;
    let requested_start = plan.incoming_start_seconds as usize * OUTPUT_RATE as usize;
    let incoming_start = requested_start.min(incoming_frames.saturating_sub(1));
    let incoming_samples = &incoming.samples[incoming_start * 2..];
    let b = if (from_bpm == 0) || (to_bpm == 0) {
        incoming_samples.to_vec()
    } else {
        let ratio = f64::from(from_bpm) / f64::from(to_bpm);
        let ratio = if (1.8..=2.2).contains(&ratio) || (0.45..=0.55).contains(&ratio) {
            1.0
        } else {
            ratio.clamp(0.92, 1.08)
        };
        resample_stereo(incoming_samples, ratio)
    };
    let a_frames = a.len() / 2;
    let b_frames = b.len() / 2;
    let fade_seconds = plan.fade_seconds.clamp(4, 30) as usize;
    let lead_seconds = (plan.entry_lead_seconds as usize).clamp(fade_seconds, 60);
    let start = a_frames.saturating_sub(lead_seconds * OUTPUT_RATE as usize);
    let fade = (fade_seconds * OUTPUT_RATE as usize).min(a_frames);
    let fade_start = a_frames.saturating_sub(fade);
    let output_frames = a_frames.max(start.saturating_add(b_frames));
    let ramp_frames = (OUTPUT_RATE as usize * 2).min(fade_start.saturating_sub(start));
    let early_gain = plan.early_gain.clamp(0.03, 0.4);
    let mut result = vec![0.0; output_frames * 2];
    for frame in 0..output_frames {
        let mut left = 0.0f32;
        let mut right = 0.0f32;
        if frame < a_frames {
            let gain = if frame >= fade_start {
                let x = (frame - fade_start) as f32 / (a_frames - fade_start).max(1) as f32;
                (x * std::f32::consts::FRAC_PI_2).cos()
            } else {
                1.0
            };
            left += a[frame * 2] * gain;
            right += a[frame * 2 + 1] * gain;
        }
        let b_frame = frame as isize - start as isize;
        if b_frame >= 0 && (b_frame as usize) < b_frames {
            let gain = if frame < start + ramp_frames {
                let x = (frame - start) as f32 / ramp_frames.max(1) as f32;
                early_gain * x.sin().powi(2)
            } else if frame < fade_start {
                early_gain
            } else if frame < a_frames {
                let x = (frame - fade_start) as f32 / (a_frames - fade_start).max(1) as f32;
                early_gain + (1.0 - early_gain) * (x * std::f32::consts::FRAC_PI_2).sin()
            } else {
                1.0
            };
            let i = b_frame as usize * 2;
            left += b[i] * gain;
            right += b[i + 1] * gain;
        }
        result[frame * 2] = soft_limit(left);
        result[frame * 2 + 1] = soft_limit(right);
    }
    (result, fade_start)
}

pub fn transition(
    outgoing: &StereoPcm,
    incoming: &StereoPcm,
    from_bpm: u16,
    to_bpm: u16,
    plan: &TransitionPlan,
) -> StereoPcm {
    let (samples, _) = blend(&outgoing.samples, incoming, from_bpm, to_bpm, plan);
    let frames = samples.len() / 2;
    StereoPcm {
        duration: Duration::from_secs_f64(frames as f64 / f64::from(OUTPUT_RATE)),
        samples,
    }
}

/// Like [`transition`], but for a continuous multi-track mix: instead of
/// returning the whole combined buffer (which would otherwise grow with
/// every track added), everything that is already finalized is written
/// straight to `writer` and only the still-open tail is returned. Memory
/// use this way stays bounded by roughly one track's length, however many
/// tracks are being mixed.
pub fn transition_stream(
    writer: &mut MixWriter,
    outgoing_tail: &StereoPcm,
    incoming: &StereoPcm,
    from_bpm: u16,
    to_bpm: u16,
    plan: &TransitionPlan,
) -> Result<StereoPcm, String> {
    let (samples, fade_start) = blend(&outgoing_tail.samples, incoming, from_bpm, to_bpm, plan);
    writer.write_frames(&samples[..fade_start * 2])?;
    let tail_samples = samples[fade_start * 2..].to_vec();
    let frames = tail_samples.len() / 2;
    Ok(StereoPcm {
        duration: Duration::from_secs_f64(frames as f64 / f64::from(OUTPUT_RATE)),
        samples: tail_samples,
    })
}

const WAV_HEADER_BYTES: u64 = 44;

/// Streams a mix out to a 16-bit PCM WAV file on disk instead of
/// accumulating it in memory, so a long mix (many tracks) stays bounded by
/// disk space rather than RAM.
pub struct MixWriter {
    file: File,
    frames_written: u64,
}

impl MixWriter {
    pub fn create(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut file = File::create(path).map_err(|e| format!("Could not create mix file: {e}"))?;
        write_wav_header(&mut file, 0)?;
        Ok(Self {
            file,
            frames_written: 0,
        })
    }

    /// Appends interleaved stereo `f32` samples (one `left, right` pair per
    /// frame) to the file.
    pub fn write_frames(&mut self, samples: &[f32]) -> Result<(), String> {
        if samples.is_empty() {
            return Ok(());
        }
        let mut bytes = Vec::with_capacity(samples.len() * 2);
        for &sample in samples {
            let clamped = sample.clamp(-1.0, 1.0);
            let value = (clamped * f32::from(i16::MAX)) as i16;
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        self.file
            .write_all(&bytes)
            .map_err(|e| format!("Could not write mix audio: {e}"))?;
        self.frames_written += (samples.len() / 2) as u64;
        Ok(())
    }

    /// Finalizes the WAV header with the real sizes now that the total
    /// length is known, and returns the mix's total duration.
    pub fn finish(mut self) -> Result<Duration, String> {
        write_wav_header(&mut self.file, self.frames_written)?;
        self.file
            .sync_all()
            .map_err(|e| format!("Could not finish writing the mix file: {e}"))?;
        Ok(Duration::from_secs_f64(
            self.frames_written as f64 / f64::from(OUTPUT_RATE),
        ))
    }
}

fn write_wav_header(file: &mut File, frames: u64) -> Result<(), String> {
    const CHANNELS: u16 = 2;
    const BITS_PER_SAMPLE: u16 = 16;
    let byte_rate = OUTPUT_RATE * u32::from(CHANNELS) * u32::from(BITS_PER_SAMPLE / 8);
    let block_align = CHANNELS * (BITS_PER_SAMPLE / 8);
    let data_size = frames * u64::from(CHANNELS) * u64::from(BITS_PER_SAMPLE / 8);
    let riff_size = (WAV_HEADER_BYTES - 8) + data_size;
    let mut header = Vec::with_capacity(WAV_HEADER_BYTES as usize);
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&(riff_size as u32).to_le_bytes());
    header.extend_from_slice(b"WAVE");
    header.extend_from_slice(b"fmt ");
    header.extend_from_slice(&16u32.to_le_bytes());
    header.extend_from_slice(&1u16.to_le_bytes());
    header.extend_from_slice(&CHANNELS.to_le_bytes());
    header.extend_from_slice(&OUTPUT_RATE.to_le_bytes());
    header.extend_from_slice(&byte_rate.to_le_bytes());
    header.extend_from_slice(&block_align.to_le_bytes());
    header.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
    header.extend_from_slice(b"data");
    header.extend_from_slice(&(data_size as u32).to_le_bytes());
    file.seek(SeekFrom::Start(0))
        .map_err(|e| format!("Could not write mix header: {e}"))?;
    file.write_all(&header)
        .map_err(|e| format!("Could not write mix header: {e}"))?;
    Ok(())
}

fn resample_stereo(input: &[f32], speed: f64) -> Vec<f32> {
    if input.len() < 2 || speed == 1.0 {
        return input.to_vec();
    }
    let frames = input.len() / 2;
    let output_frames = (frames as f64 / speed) as usize;
    let mut out = Vec::with_capacity(output_frames * 2);
    for frame in 0..output_frames {
        let position = frame as f64 * speed;
        let index = (position as usize).min(frames - 1);
        let next = (index + 1).min(frames - 1);
        let fraction = (position - index as f64) as f32;
        for channel in 0..2 {
            let a = input[index * 2 + channel];
            let b = input[next * 2 + channel];
            out.push(a + (b - a) * fraction);
        }
    }
    out
}

fn soft_limit(value: f32) -> f32 {
    const THRESHOLD: f32 = 0.86;
    if value > THRESHOLD {
        THRESHOLD + (1.0 - THRESHOLD) * ((value - THRESHOLD) / (1.0 - THRESHOLD)).tanh()
    } else if value < -THRESHOLD {
        -THRESHOLD - (1.0 - THRESHOLD) * ((-value - THRESHOLD) / (1.0 - THRESHOLD)).tanh()
    } else {
        value
    }
}

fn analyze_envelope(envelope: &[f32]) -> (u16, f32, u32) {
    if envelope.len() < 300 {
        return (0, 0.0, 0);
    }
    let mean = envelope.iter().sum::<f32>() / envelope.len() as f32;
    let bpm_envelope = &envelope[..envelope.len().min(12_000)];
    let flux: Vec<f32> = bpm_envelope
        .windows(2)
        .map(|x| (x[1] - x[0] - mean * 0.015).max(0.0))
        .collect();
    let mut best = (0usize, 0.0f64);
    for lag in 33..=100.min(flux.len() / 2) {
        let score: f64 = (lag..flux.len())
            .map(|i| (flux[i] * flux[i - lag]) as f64)
            .sum();
        if score > best.1 {
            best = (lag, score);
        }
    }
    let bpm = if best.0 == 0 || best.1 <= 0.0 {
        0
    } else {
        let mut value = (6000.0 / best.0 as f64).round() as i32;
        while value < 60 {
            value *= 2;
        }
        while value > 180 {
            value /= 2;
        }
        if (60..=180).contains(&value) {
            value as u16
        } else {
            0
        }
    };
    let energy = (mean * 4.0).clamp(0.0, 1.0);
    let rise = find_rise(envelope);
    (bpm, energy, rise)
}

fn find_rise(envelope: &[f32]) -> u32 {
    let bin_frames = 50;
    let bins: Vec<f32> = envelope
        .chunks(bin_frames)
        .map(|v| v.iter().sum::<f32>() / v.len().max(1) as f32)
        .collect();
    let look = 8;
    if bins.len() < look * 2 + 1 {
        return 0;
    }
    let mut best = 0.0f32;
    let mut best_at = 0usize;
    for at in look..bins.len().saturating_sub(look) {
        let before = bins[at - look..at].iter().sum::<f32>() / look as f32;
        let after = bins[at..at + look].iter().sum::<f32>() / look as f32;
        let delta = after - before;
        if delta > best {
            best = delta;
            best_at = at;
        }
    }
    if best < 0.025 {
        0
    } else {
        (best_at / 2) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_plan() -> TransitionPlan {
        TransitionPlan {
            entry_lead_seconds: 8,
            fade_seconds: 4,
            incoming_start_seconds: 0,
            early_gain: 0.1,
        }
    }

    #[test]
    fn half_time_bpm_is_not_stretched() {
        let pcm = StereoPcm {
            samples: vec![0.2; 44_100 * 2],
            duration: Duration::from_secs(1),
        };
        let out = transition(&pcm, &pcm, 128, 64, &test_plan());
        assert_eq!(out.samples.len(), pcm.samples.len());
    }

    #[test]
    fn overlapping_audio_is_limited() {
        let pcm = StereoPcm {
            samples: vec![0.95; 44_100 * 8],
            duration: Duration::from_secs(4),
        };
        let out = transition(&pcm, &pcm, 120, 120, &test_plan());
        assert!(out.samples.iter().all(|sample| sample.abs() < 1.0));
    }
}
