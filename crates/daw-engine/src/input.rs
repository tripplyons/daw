//! Recording from the default input device. The input callback copies audio
//! into a ring buffer while the song plays, marking where each take starts
//! and ends; the UI drains the ring into takes.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::engine::Shared;

/// Seconds of audio the ring holds between drains.
const RING_SECONDS: usize = 30;

#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("no audio input device")]
    NoDevice,
    #[error("audio input: {0}")]
    Device(#[from] cpal::Error),
}

/// Audio recorded during one stretch of playback.
#[derive(Debug, Clone, PartialEq)]
pub struct Take {
    /// Song position of the first frame, in ticks. Negative when the input
    /// latency reaches back before the song start.
    pub start: f64,
    pub channels: u16,
    pub sample_rate: u32,
    /// Interleaved samples.
    pub samples: Vec<f32>,
}

#[derive(Debug, Clone, Copy)]
enum Marker {
    /// A take starts at this sample count.
    Start { at: u64, tick: f64 },
    End { at: u64 },
}

impl Marker {
    fn at(&self) -> u64 {
        match *self {
            Marker::Start { at, .. } | Marker::End { at } => at,
        }
    }
}

pub struct Recorder {
    _stream: cpal::Stream,
    takes: Takes,
    overflowed: Arc<AtomicBool>,
}

impl Recorder {
    /// Open the default input device. Audio is kept only while `shared`
    /// reports that the song is playing.
    pub fn start(shared: Arc<Shared>) -> Result<Recorder, InputError> {
        let device = cpal::default_host().default_input_device().ok_or(InputError::NoDevice)?;
        let config = device.default_input_config()?.config();
        let channels = config.channels;
        let sample_rate = config.sample_rate;
        let (mut sample_tx, samples) = rtrb::RingBuffer::new(sample_rate as usize * usize::from(channels) * RING_SECONDS);
        let (mut marker_tx, markers) = rtrb::RingBuffer::new(64);
        let overflowed = Arc::new(AtomicBool::new(false));
        let overflow = overflowed.clone();
        let mut pushed = 0u64;
        let mut recording = false;
        let stream = device.build_input_stream::<f32, _, _>(
            config,
            move |data: &[f32], info: &cpal::InputCallbackInfo| {
                let clock = shared.clock().filter(|c| c.playing);
                let Some(clock) = clock else {
                    if recording && marker_tx.push(Marker::End { at: pushed }).is_ok() {
                        recording = false;
                    }
                    return;
                };
                if !recording {
                    let tick = clock.tick_at(info.timestamp().capture.as_nanos() as u64);
                    if marker_tx.push(Marker::Start { at: pushed, tick }).is_err() {
                        return;
                    }
                    recording = true;
                }
                // Whole frames only, so channels stay interleaved in order.
                let room = sample_tx.slots();
                let room = room - room % usize::from(channels);
                let (written, _) = sample_tx.push_partial_slice(&data[..data.len().min(room)]);
                pushed += written.len() as u64;
                if written.len() < data.len() {
                    overflow.store(true, Ordering::Relaxed);
                }
            },
            |error| log::error!("audio input: {error}"),
            None,
        )?;
        stream.play()?;
        let takes = Takes { samples, markers, channels, sample_rate, drained: 0, take: None };
        Ok(Recorder { _stream: stream, takes, overflowed })
    }

    /// Move recorded audio out of the ring, returning the takes that ended.
    pub fn poll(&mut self) -> Vec<Take> {
        self.takes.poll()
    }

    /// The take being recorded, as drained so far.
    pub fn current(&self) -> Option<&Take> {
        self.takes.take.as_ref()
    }

    /// Whether the ring filled up and dropped audio since the last call.
    pub fn take_overflow(&self) -> bool {
        self.overflowed.swap(false, Ordering::Relaxed)
    }

    /// Stop the input, returning the takes not yet returned by `poll`,
    /// including the one in progress.
    pub fn finish(mut self) -> Vec<Take> {
        let mut ended = self.takes.poll();
        ended.extend(self.takes.take.take());
        ended
    }
}

/// The receiving end of the input callback's rings.
struct Takes {
    samples: rtrb::Consumer<f32>,
    markers: rtrb::Consumer<Marker>,
    channels: u16,
    sample_rate: u32,
    /// Samples taken out of the ring so far, counted like the markers.
    drained: u64,
    take: Option<Take>,
}

impl Takes {
    fn poll(&mut self) -> Vec<Take> {
        let mut ended = Vec::new();
        // Count samples before looking at markers: the callback pushes a
        // marker before the samples after it, so every marker for these
        // samples is visible below.
        let mut available = self.samples.slots() as u64;
        loop {
            let next = self.markers.peek().ok().map(Marker::at);
            let n = next.map_or(available, |at| at.saturating_sub(self.drained).min(available));
            if let Ok(chunk) = self.samples.read_chunk(n as usize) {
                if let Some(take) = &mut self.take {
                    let (a, b) = chunk.as_slices();
                    take.samples.extend_from_slice(a);
                    take.samples.extend_from_slice(b);
                }
                chunk.commit_all();
            }
            self.drained += n;
            available -= n;
            if next != Some(self.drained) {
                break;
            }
            match self.markers.pop() {
                Ok(Marker::Start { tick, .. }) => {
                    ended.extend(self.take.take());
                    self.take = Some(Take { start: tick, channels: self.channels, sample_rate: self.sample_rate, samples: Vec::new() });
                }
                Ok(Marker::End { .. }) => ended.extend(self.take.take()),
                Err(_) => break,
            }
        }
        ended
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_split_at_markers_and_drop_audio_between_them() {
        let (mut sample_tx, samples) = rtrb::RingBuffer::new(64);
        let (mut marker_tx, markers) = rtrb::RingBuffer::new(8);
        let mut takes = Takes { samples, markers, channels: 1, sample_rate: 48_000, drained: 0, take: None };
        let take = |start, samples: &[f32]| Take { start, channels: 1, sample_rate: 48_000, samples: samples.to_vec() };

        marker_tx.push(Marker::Start { at: 0, tick: 10.0 }).unwrap();
        sample_tx.push_entire_slice(&[1.0, 2.0]).unwrap();
        assert!(takes.poll().is_empty());
        sample_tx.push_entire_slice(&[3.0]).unwrap();
        marker_tx.push(Marker::End { at: 3 }).unwrap();
        // Audio after an end marker belongs to no take until the next start.
        marker_tx.push(Marker::Start { at: 3, tick: 99.0 }).unwrap();
        sample_tx.push_entire_slice(&[4.0, 5.0]).unwrap();
        assert_eq!(takes.poll(), vec![take(10.0, &[1.0, 2.0, 3.0])]);
        assert_eq!(takes.take, Some(take(99.0, &[4.0, 5.0])));
    }
}
