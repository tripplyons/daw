//! Built-in polyphonic synth and one-shot sampler.

use std::sync::Arc;

use daw_model::{SynthParams, Waveform};

use crate::processor::{Event, EventKind, Processor, TransportInfo};

pub const PARAM_CUTOFF: u32 = 0;
pub const PARAM_ATTACK: u32 = 1;
pub const PARAM_RELEASE: u32 = 2;
pub const PARAM_WAVEFORM: u32 = 3;

const VOICES: usize = 16;

#[derive(Clone, Copy, Default)]
struct Voice {
    key: u8,
    active: bool,
    released: bool,
    phase: f64,
    /// Envelope level 0..1.
    level: f32,
    velocity: f32,
    /// Filter state per channel for the synth; playback position for the sampler.
    state: [f32; 2],
    position: f64,
}

fn key_frequency(key: u8) -> f64 {
    440.0 * 2f64.powf((f64::from(key) - 69.0) / 12.0)
}

/// Pick a free voice, or steal the quietest one.
fn allocate(voices: &mut [Voice; VOICES]) -> &mut Voice {
    let index = voices.iter().position(|v| !v.active).unwrap_or_else(|| {
        voices
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.level.total_cmp(&b.1.level))
            .map(|(i, _)| i)
            .unwrap_or(0)
    });
    &mut voices[index]
}

pub struct Synth {
    params: SynthParams,
    sample_rate: f64,
    voices: [Voice; VOICES],
}

impl Synth {
    pub fn new(params: SynthParams, sample_rate: f64) -> Self {
        Self { params, sample_rate, voices: [Voice::default(); VOICES] }
    }

    fn apply(&mut self, kind: EventKind) {
        match kind {
            EventKind::NoteOn { key, velocity } => {
                let voice = allocate(&mut self.voices);
                *voice = Voice { key, active: true, velocity, ..Voice::default() };
            }
            EventKind::NoteOff { key } => {
                for voice in self.voices.iter_mut().filter(|v| v.active && v.key == key) {
                    voice.released = true;
                }
            }
            EventKind::Param { id, value } => match id {
                PARAM_CUTOFF => self.params.cutoff = value,
                PARAM_ATTACK => self.params.attack = value * 2.0,
                PARAM_RELEASE => self.params.release = value * 4.0,
                PARAM_WAVEFORM => {
                    self.params.waveform = match (value * 2.99) as u32 {
                        0 => Waveform::Sine,
                        1 => Waveform::Saw,
                        _ => Waveform::Square,
                    }
                }
                _ => {}
            },
        }
    }
}

impl Processor for Synth {
    fn process(&mut self, _: &TransportInfo, events: &[Event], left: &mut [f32], right: &mut [f32]) {
        left.fill(0.0);
        right.fill(0.0);
        let sr = self.sample_rate as f32;
        let attack_step = 1.0 / (self.params.attack.max(0.001) * sr);
        let release_step = 1.0 / (self.params.release.max(0.001) * sr);
        // One-pole lowpass.
        let cutoff_hz = SynthParams::cutoff_hz(self.params.cutoff);
        let alpha = 1.0 - (-2.0 * std::f32::consts::PI * cutoff_hz / sr).exp();
        let mut next_event = 0;
        for frame in 0..left.len() {
            while next_event < events.len() && events[next_event].offset as usize <= frame {
                self.apply(events[next_event].kind);
                next_event += 1;
            }
            let mut sum = 0.0;
            for voice in self.voices.iter_mut().filter(|v| v.active) {
                let raw = match self.params.waveform {
                    Waveform::Sine => (voice.phase * std::f64::consts::TAU).sin() as f32,
                    Waveform::Saw => (2.0 * voice.phase - 1.0) as f32,
                    Waveform::Square => {
                        if voice.phase < 0.5 {
                            1.0
                        } else {
                            -1.0
                        }
                    }
                };
                voice.phase = (voice.phase + key_frequency(voice.key) / self.sample_rate).fract();
                voice.state[0] += alpha * (raw - voice.state[0]);
                if voice.released {
                    voice.level -= release_step;
                    if voice.level <= 0.0 {
                        voice.active = false;
                        continue;
                    }
                } else {
                    voice.level = (voice.level + attack_step).min(1.0);
                }
                sum += voice.state[0] * voice.level * voice.velocity;
            }
            left[frame] = sum * 0.25;
            right[frame] = sum * 0.25;
        }
        for event in &events[next_event..] {
            self.apply(event.kind);
        }
    }

    fn reset(&mut self) {
        self.voices = [Voice::default(); VOICES];
    }
}

/// Decoded sample data, shared between the UI and the audio thread.
pub struct Sample {
    pub sample_rate: f64,
    pub left: Vec<f32>,
    pub right: Vec<f32>,
}

impl Sample {
    pub fn load(path: &str) -> Result<Sample, hound::Error> {
        let mut reader = hound::WavReader::open(path)?;
        let spec = reader.spec();
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
            hound::SampleFormat::Int => {
                let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
                reader.samples::<i32>().map(|s| s.map(|s| s as f32 * scale)).collect::<Result<_, _>>()?
            }
        };
        let channels = usize::from(spec.channels.max(1));
        let left: Vec<f32> = samples.iter().step_by(channels).copied().collect();
        let right = if channels > 1 { samples.iter().skip(1).step_by(channels).copied().collect() } else { left.clone() };
        Ok(Sample { sample_rate: f64::from(spec.sample_rate), left, right })
    }
}

pub struct Sampler {
    sample: Arc<Sample>,
    root_key: u8,
    sample_rate: f64,
    voices: [Voice; VOICES],
}

impl Sampler {
    pub fn new(sample: Arc<Sample>, root_key: u8, sample_rate: f64) -> Self {
        Self { sample, root_key, sample_rate, voices: [Voice::default(); VOICES] }
    }

    fn apply(&mut self, kind: EventKind) {
        match kind {
            EventKind::NoteOn { key, velocity } => {
                let ratio = 2f64.powf((f64::from(key) - f64::from(self.root_key)) / 12.0);
                let step = ratio * self.sample.sample_rate / self.sample_rate;
                let voice = allocate(&mut self.voices);
                // `phase` holds the playback increment for sampler voices.
                *voice = Voice { key, active: true, velocity, level: 1.0, phase: step, ..Voice::default() };
            }
            // One-shot: note-off does not cut the sample.
            EventKind::NoteOff { .. } | EventKind::Param { .. } => {}
        }
    }
}

impl Processor for Sampler {
    fn process(&mut self, _: &TransportInfo, events: &[Event], left: &mut [f32], right: &mut [f32]) {
        left.fill(0.0);
        right.fill(0.0);
        let length = self.sample.left.len();
        let mut next_event = 0;
        for frame in 0..left.len() {
            while next_event < events.len() && events[next_event].offset as usize <= frame {
                self.apply(events[next_event].kind);
                next_event += 1;
            }
            for voice in self.voices.iter_mut().filter(|v| v.active) {
                let index = voice.position as usize;
                if index + 1 >= length {
                    voice.active = false;
                    continue;
                }
                let t = (voice.position - index as f64) as f32;
                let l = self.sample.left[index] + (self.sample.left[index + 1] - self.sample.left[index]) * t;
                let r = self.sample.right[index] + (self.sample.right[index + 1] - self.sample.right[index]) * t;
                left[frame] += l * voice.velocity;
                right[frame] += r * voice.velocity;
                voice.position += voice.phase;
            }
        }
        for event in &events[next_event..] {
            self.apply(event.kind);
        }
    }

    fn reset(&mut self) {
        self.voices = [Voice::default(); VOICES];
    }
}
