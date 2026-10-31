//! Transcription on this machine: an audio file is decoded, resampled to the
//! 16 kHz mono Whisper listens to, and transcribed five minutes at a time, so
//! a long recording never sits in memory whole.

use std::io::Cursor;
use std::path::Path;

use loqui_whisper::{Device, Options, Whisper};
use opus_pure::OpusDecoder;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use crate::{Error, Result};

pub const WHISPER_RATE: u32 = 16_000;
pub(crate) const REPOSITORY: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";
/// Whisper small, multilingual, at 5 bits: 190 MB.
pub(crate) const MODEL: &str = "ggml-small-q5_1.bin";
/// A chunk can cut a word in two where it ends; splitting at a pause would
/// avoid that if it shows in transcripts.
const CHUNK_SECONDS: usize = 300;
/// Longest Opus frame, 120 ms, at 16 kHz.
const OPUS_FRAME: usize = 1920;

/// A stretch of speech and where it starts, in seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub start: f64,
    pub text: String,
}

/// The transcription model, loaded once and used for every chunk.
pub struct Listener {
    whisper: Whisper,
}

impl Listener {
    pub fn load(dir: &Path) -> Result<Self> {
        let whisper = Whisper::load(&dir.join(MODEL), Device::Cpu)
            .map_err(|e| Error::Listen(e.to_string()))?;
        Ok(Listener { whisper })
    }

    /// Transcribes an audio file in any language Whisper knows.
    /// `progress(seconds done, seconds in all when the file says)`.
    pub fn transcribe(
        &self,
        bytes: &[u8],
        mut progress: impl FnMut(f64, Option<f64>),
    ) -> Result<Vec<Segment>> {
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get() as u16);
        let options = Options {
            threads: Some(threads),
            ..Options::default()
        };
        let mut segments = Vec::new();
        let mut offset = 0.0;
        decode(bytes, |chunk, total| {
            let heard = self
                .whisper
                .transcribe(chunk, &options)
                .map_err(|e| Error::Listen(e.to_string()))?;
            segments.extend(heard.segments.into_iter().map(|s| Segment {
                start: offset + s.start,
                text: s.text.trim().to_string(),
            }));
            offset += chunk.len() as f64 / f64::from(WHISPER_RATE);
            progress(offset, total);
            Ok(())
        })?;
        Ok(segments)
    }
}

/// Decodes `bytes` to 16 kHz mono and hands it on in chunks of about five
/// minutes, with the length in seconds when the file states it.
pub(crate) fn decode(
    bytes: &[u8],
    mut chunk: impl FnMut(&[f32], Option<f64>) -> Result<()>,
) -> Result<()> {
    let unreadable = |e: &dyn std::fmt::Display| Error::Decode(e.to_string());
    let source = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), Default::default());
    let mut format = symphonia::default::get_probe()
        .probe(
            &Hint::new(),
            source,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| unreadable(&e))?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| Error::Decode("it has no audio track".into()))?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| Error::Decode("its audio track has no codec".into()))?
        .clone();
    let total = match (track.num_frames, params.sample_rate) {
        (Some(frames), Some(rate)) => Some(frames as f64 / f64::from(rate)),
        _ => None,
    };
    let channels = params.channels.as_ref().map_or(1, |c| c.count()).max(1);
    // Symphonia reads Opus inside Ogg and WebM but has no Opus decoder.
    let mut decoder = if params.codec == CODEC_ID_OPUS {
        Decoder::Opus(
            Box::new(OpusDecoder::new(WHISPER_RATE as i32, channels).map_err(|e| unreadable(&e))?),
            vec![0.0; OPUS_FRAME * channels],
        )
    } else {
        Decoder::Symphonia(
            symphonia::default::get_codecs()
                .make_audio_decoder(&params, &AudioDecoderOptions::default())
                .map_err(|e| unreadable(&e))?,
        )
    };

    let mut pending: Vec<f32> = Vec::new();
    let mut interleaved: Vec<f32> = Vec::new();
    let mut rate = None;
    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(e) => return Err(unreadable(&e)),
        };
        if packet.track_id != track_id {
            continue;
        }
        // A damaged packet is skipped, as players skip it.
        let (samples, width, packet_rate): (&[f32], usize, u32) = match &mut decoder {
            Decoder::Symphonia(decoder) => match decoder.decode(&packet) {
                Ok(decoded) => {
                    decoded.copy_to_vec_interleaved::<f32>(&mut interleaved);
                    (&interleaved, decoded.num_planes(), decoded.spec().rate())
                }
                Err(SymphoniaError::DecodeError(_)) => continue,
                Err(SymphoniaError::ResetRequired) => {
                    decoder.reset();
                    continue;
                }
                Err(e) => return Err(unreadable(&e)),
            },
            Decoder::Opus(decoder, out) => match decoder.decode(&packet.data, OPUS_FRAME, out) {
                Ok(n) => (&out[..n * channels], channels, WHISPER_RATE),
                Err(_) => continue,
            },
        };
        if width == 0 {
            continue;
        }
        let rate = *rate.get_or_insert(packet_rate);
        let scale = 1.0 / width as f32;
        pending.extend(
            samples
                .chunks_exact(width)
                .map(|frame| frame.iter().sum::<f32>() * scale),
        );
        if pending.len() >= rate as usize * CHUNK_SECONDS {
            chunk(&to_whisper(std::mem::take(&mut pending), rate)?, total)?;
        }
    }
    match rate {
        Some(rate) if !pending.is_empty() => chunk(&to_whisper(pending, rate)?, total),
        Some(_) => Ok(()),
        None => Err(Error::Decode("it has no audio in it".into())),
    }
}

enum Decoder {
    Symphonia(Box<dyn AudioDecoder>),
    Opus(Box<OpusDecoder>, Vec<f32>),
}

/// Mono samples at `rate` resampled to 16 kHz and kept within [-1, 1].
fn to_whisper(samples: Vec<f32>, rate: u32) -> Result<Vec<f32>> {
    let mut out = if rate == WHISPER_RATE {
        samples
    } else {
        let failed =
            |e: &dyn std::fmt::Display| Error::Decode(format!("resampling {rate} Hz: {e}"));
        let mut resampler = Fft::<f32>::new(
            rate as usize,
            WHISPER_RATE as usize,
            1024,
            1,
            FixedSync::Input,
        )
        .map_err(|e| failed(&e))?;
        let len = samples.len();
        let capacity = resampler.process_all_needed_output_len(len);
        let mut out = vec![0.0f32; capacity];
        let input = InterleavedSlice::new(&samples[..], 1, len).map_err(|e| failed(&e))?;
        let mut output =
            InterleavedSlice::new_mut(&mut out[..], 1, capacity).map_err(|e| failed(&e))?;
        let (_, written) = resampler
            .process_all_into_buffer(&input, &mut output, len, None)
            .map_err(|e| failed(&e))?;
        out.truncate(written);
        out
    };
    for s in &mut out {
        *s = s.clamp(-1.0, 1.0);
    }
    Ok(out)
}
