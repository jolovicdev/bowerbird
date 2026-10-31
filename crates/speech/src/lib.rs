//! Speech on this machine: Kokoro-82M reads text aloud in two voices and
//! the recording is saved as Ogg Opus; Whisper transcribes audio files. Each
//! model is downloaded once, on first use.

pub mod listen;

use std::path::{Path, PathBuf};
use std::time::Duration;

use loqui_g2p::OovFallback;
use loqui_kokoro::{Blend, Device, Kokoro, KokoroModel};
use opus_pure::{Application, MAX_PACKET_BYTES, OggOpusWriter, OpusEncoder, OpusHead};
use tokio::io::AsyncWriteExt;

pub const SAMPLE_RATE: u32 = loqui_kokoro::SAMPLE_RATE;
/// The two hosts' voices.
pub const HOSTS: [&str; 2] = ["af_heart", "am_michael"];
const REPOSITORY: &str = "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/resolve/main";
/// The half-precision export: on CPUs it speaks as fast as the full model
/// at half the download (163 MB), and about four times faster than the
/// smaller 8-bit one.
const MODEL: &str = "onnx/model_fp16.onnx";
const BITRATE: i32 = 32_000;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Download(String),
    #[error("The voice model couldn't speak: {0}")]
    Voice(#[from] loqui_kokoro::Error),
    #[error("The recording couldn't be saved: {0}")]
    Encode(#[from] opus_pure::Error),
    #[error("This audio file can't be read: {0}.")]
    Decode(String),
    #[error("Transcription failed: {0}")]
    Listen(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// What a recording needs, as paths inside the model repository.
fn voice_files() -> Vec<String> {
    std::iter::once(MODEL.to_string())
        .chain(HOSTS.iter().map(|voice| format!("voices/{voice}.bin")))
        .collect()
}

/// Whether the voice model and voices are already in `dir`.
pub fn voices_ready(dir: &Path) -> bool {
    voice_files().iter().all(|file| dir.join(file).is_file())
}

/// Downloads what reading aloud needs into `dir`. `progress` gets the bytes
/// done and the total of the file being downloaded, as they arrive.
pub async fn download_voices(dir: &Path, progress: impl Fn(u64, u64)) -> Result<()> {
    fetch(dir, REPOSITORY, &voice_files(), progress).await
}

/// Whether the transcription model is already in `dir`.
pub fn transcriber_ready(dir: &Path) -> bool {
    dir.join(listen::MODEL).is_file()
}

/// Downloads the transcription model into `dir`, reporting as `download_voices`.
pub async fn download_transcriber(dir: &Path, progress: impl Fn(u64, u64)) -> Result<()> {
    fetch(
        dir,
        listen::REPOSITORY,
        &[listen::MODEL.to_string()],
        progress,
    )
    .await
}

async fn fetch(
    dir: &Path,
    repository: &str,
    files: &[String],
    progress: impl Fn(u64, u64),
) -> Result<()> {
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| Error::Download(e.to_string()))?;
    for file in files {
        let path = dir.join(file);
        if path.is_file() {
            continue;
        }
        let failed = |e: reqwest::Error| Error::Download(format!("Couldn't download {file}: {e}"));
        let mut response = http
            .get(format!("{repository}/{file}"))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(failed)?;
        let total = response.content_length().unwrap_or(0);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        // Written beside the real name and renamed when complete, so an
        // interrupted download is never mistaken for a finished one.
        let part = PathBuf::from(format!("{}.part", path.display()));
        let mut out = tokio::fs::File::create(&part).await?;
        let mut done = 0;
        while let Some(chunk) = response.chunk().await.map_err(failed)? {
            out.write_all(&chunk).await?;
            done += chunk.len() as u64;
            progress(done, total);
        }
        out.flush().await?;
        if total > 0 && done != total {
            return Err(Error::Download(format!(
                "The download of {file} stopped at {done} of {total} bytes. Try again."
            )));
        }
        tokio::fs::rename(&part, &path).await?;
    }
    Ok(())
}

/// The voice model, loaded once and used for every line.
pub struct Speaker {
    kokoro: Kokoro,
    hosts: Vec<Blend>,
}

impl Speaker {
    pub fn load(dir: &Path) -> Result<Self> {
        let model = KokoroModel::load(&dir.join(MODEL), Device::Cpu, None)?;
        let hosts = HOSTS
            .iter()
            .map(|voice| Blend::parse(voice, |_| None))
            .collect::<std::result::Result<_, _>>()?;
        Ok(Speaker {
            kokoro: Kokoro::new(model, dir.join("voices"), OovFallback::Neural),
            hosts,
        })
    }

    /// One host's line as mono samples at `SAMPLE_RATE`.
    pub fn speak(&self, host: usize, text: &str) -> Result<Vec<f32>> {
        let voice = &self.hosts[host % self.hosts.len()];
        Ok(self.kokoro.speak(text, voice, 1.0)?)
    }
}

/// Mono samples at `SAMPLE_RATE` as an Ogg Opus file.
pub fn ogg_opus(samples: &[f32]) -> Result<Vec<u8>> {
    let rate = SAMPLE_RATE as i32;
    let frame = (rate / 50) as usize;
    let mut encoder = OpusEncoder::new(rate, 1, Application::Audio)?;
    encoder.bitrate_bps = BITRATE;
    let head = OpusHead::for_encoder(&encoder, SAMPLE_RATE);
    let mut writer = OggOpusWriter::new(Vec::new(), head)?;
    let mut packet = vec![0u8; MAX_PACKET_BYTES];
    let blocks = samples.chunks_exact(frame);
    let mut last = blocks.remainder().to_vec();
    for block in blocks {
        let n = encoder.encode(block, frame, &mut packet)?;
        writer.write_packet(&packet[..n])?;
    }
    if !last.is_empty() {
        last.resize(frame, 0.0);
        let n = encoder.encode(&last, frame, &mut packet)?;
        writer.write_packet(&packet[..n])?;
    }
    Ok(writer.finish()?)
}

#[cfg(test)]
mod tests {
    use opus_pure::OggOpusReader;

    use super::*;

    #[test]
    fn a_recording_keeps_its_length() {
        let seconds = 1.51;
        let samples: Vec<f32> = (0..(SAMPLE_RATE as f32 * seconds) as usize)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / SAMPLE_RATE as f32).sin() * 0.3)
            .collect();
        let ogg = ogg_opus(&samples).unwrap();
        assert!(ogg.starts_with(b"OggS"));
        let mut reader = OggOpusReader::new(std::io::Cursor::new(ogg)).unwrap();
        let mut packets = 0;
        while reader.read_packet().unwrap().is_some() {
            packets += 1;
        }
        // 75 whole 20 ms packets and a padded one for the last 10 ms.
        assert_eq!(packets, 76);
    }

    /// Everything `listen::decode` hands on, joined.
    fn decoded(bytes: &[u8]) -> (Vec<f32>, Option<f64>) {
        let mut all = Vec::new();
        let mut total = None;
        listen::decode(bytes, |chunk, length| {
            all.extend_from_slice(chunk);
            total = length;
            Ok(())
        })
        .unwrap();
        (all, total)
    }

    #[test]
    fn stereo_wav_becomes_16_khz_mono() {
        let (rate, seconds) = (44_100u32, 2);
        let frames = rate as usize * seconds;
        let mut wav = Vec::new();
        let data = (frames * 4) as u32;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * 4).to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data.to_le_bytes());
        for i in 0..frames {
            // Left and right in opposite phase average to silence; only the
            // left channel's half survives the mix, so mono is half as loud.
            let left = ((i as f32 * 0.05).sin() * 16_000.0) as i16;
            wav.extend_from_slice(&left.to_le_bytes());
            wav.extend_from_slice(&0i16.to_le_bytes());
        }
        let (samples, total) = decoded(&wav);
        assert_eq!(total, Some(2.0));
        let expected = listen::WHISPER_RATE as usize * seconds;
        assert!(
            samples.len().abs_diff(expected) < 200,
            "{} samples",
            samples.len()
        );
        let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
        assert!((0.2..0.3).contains(&peak), "peak {peak}");
    }

    #[test]
    fn opus_decodes_straight_to_16_khz() {
        let samples: Vec<f32> = (0..SAMPLE_RATE as usize * 3)
            .map(|i| (i as f32 * 0.02).sin() * 0.3)
            .collect();
        let (heard, _) = decoded(&ogg_opus(&samples).unwrap());
        let expected = listen::WHISPER_RATE as usize * 3;
        assert!(
            heard.len().abs_diff(expected) < 1_000,
            "{} samples",
            heard.len()
        );
    }
}
