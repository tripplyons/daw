//! Live output through cpal and offline export to WAV.

use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::engine::{Engine, MAX_BLOCK};

#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("no audio output device")]
    NoDevice,
    #[error("audio device: {0}")]
    Device(#[from] cpal::Error),
}

/// Sample rate of the default output device.
pub fn default_sample_rate() -> Result<f64, OutputError> {
    let device = cpal::default_host().default_output_device().ok_or(OutputError::NoDevice)?;
    Ok(f64::from(device.default_output_config()?.sample_rate()))
}

/// Start the default output device. The callback only `try_lock`s the engine,
/// so it never blocks; it outputs silence while an export holds the lock.
pub fn start(engine: Arc<Mutex<Engine>>) -> Result<cpal::Stream, OutputError> {
    let device = cpal::default_host().default_output_device().ok_or(OutputError::NoDevice)?;
    let supported = device.default_output_config()?;
    let mut config = supported.config();
    config.buffer_size = cpal::BufferSize::Fixed(256);
    let channels = usize::from(config.channels);
    let mut left = vec![0.0f32; MAX_BLOCK];
    let mut right = vec![0.0f32; MAX_BLOCK];
    let build = |config: cpal::StreamConfig, mut left: Vec<f32>, mut right: Vec<f32>| {
        let engine = engine.clone();
        device.build_output_stream::<f32, _, _>(
            config,
            move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                let Ok(mut engine) = engine.try_lock() else {
                    data.fill(0.0);
                    return;
                };
                let mut heard = info.timestamp().playback.as_nanos() as u64;
                let nanos_per_frame = 1e9 / engine.sample_rate();
                for chunk in data.chunks_mut(MAX_BLOCK * channels) {
                    let frames = chunk.len() / channels;
                    engine.render_live(&mut left[..frames], &mut right[..frames], heard);
                    heard += (frames as f64 * nanos_per_frame) as u64;
                    for (frame, out) in chunk.chunks_mut(channels).enumerate() {
                        out[0] = left[frame];
                        if channels > 1 {
                            out[1] = right[frame];
                        }
                        out.iter_mut().skip(2).for_each(|s| *s = 0.0);
                    }
                }
            },
            |error| log::error!("audio stream: {error}"),
            None,
        )
    };
    let stream = match build(config, left.clone(), right.clone()) {
        Ok(stream) => stream,
        Err(error) => {
            log::warn!("fixed 256-frame buffer rejected ({error}); using device default");
            config.buffer_size = cpal::BufferSize::Default;
            build(config, std::mem::take(&mut left), std::mem::take(&mut right))?
        }
    };
    stream.play()?;
    Ok(stream)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitDepth {
    Int24,
    Float32,
}

/// A mixer insert's output to write as its own WAV file.
pub struct Stem {
    /// Index into the song's inserts; 0 is the master.
    pub insert: usize,
    pub path: std::path::PathBuf,
}

/// Render `frames` from `start` into a WAV file, and each stem into its own
/// file from the same pass. The engine must already hold a song-mode plan.
pub fn export_wav(
    engine: &mut Engine,
    path: &std::path::Path,
    start: daw_model::time::Ticks,
    frames: usize,
    depth: BitDepth,
    stems: &[Stem],
) -> Result<(), hound::Error> {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: engine.sample_rate() as u32,
        bits_per_sample: match depth {
            BitDepth::Int24 => 24,
            BitDepth::Float32 => 32,
        },
        sample_format: match depth {
            BitDepth::Int24 => hound::SampleFormat::Int,
            BitDepth::Float32 => hound::SampleFormat::Float,
        },
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    let mut stem_writers = stems.iter().map(|s| hound::WavWriter::create(&s.path, spec)).collect::<Result<Vec<_>, _>>()?;
    engine.set_capture(!stems.is_empty());
    engine.start_offline(start);
    let (mut left, mut right) = (vec![0.0; MAX_BLOCK], vec![0.0; MAX_BLOCK]);
    let mut result = Ok(());
    let mut done = 0;
    while done < frames && result.is_ok() {
        let n = (frames - done).min(MAX_BLOCK);
        engine.render(&mut left[..n], &mut right[..n]);
        result = write_block(&mut writer, depth, &left[..n], &right[..n]);
        for (stem, stem_writer) in stems.iter().zip(&mut stem_writers) {
            if let (Ok(()), Some([l, r])) = (&result, engine.captured().get(stem.insert)) {
                result = write_block(stem_writer, depth, &l[..n], &r[..n]);
            }
        }
        done += n;
    }
    engine.stop_offline();
    engine.set_capture(false);
    result?;
    for stem_writer in stem_writers {
        stem_writer.finalize()?;
    }
    writer.finalize()
}

fn write_block<W: std::io::Write + std::io::Seek>(
    writer: &mut hound::WavWriter<W>,
    depth: BitDepth,
    left: &[f32],
    right: &[f32],
) -> Result<(), hound::Error> {
    for (&l, &r) in left.iter().zip(right) {
        match depth {
            BitDepth::Int24 => {
                let scale = 8_388_607.0;
                writer.write_sample((l.clamp(-1.0, 1.0) * scale) as i32)?;
                writer.write_sample((r.clamp(-1.0, 1.0) * scale) as i32)?;
            }
            BitDepth::Float32 => {
                writer.write_sample(l)?;
                writer.write_sample(r)?;
            }
        }
    }
    Ok(())
}
