//! Audio edits are prepared off the audio thread and shared by playback plans.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use daw_model::{AudioEdit, ClipSource, Project, Source};
use crate::synth::Sample;

/// Active project audio is retained. Inactive results share a 256 MiB,
/// 64-entry budget, with the least recently used result evicted first.
#[derive(Clone)]
pub struct Cache {
    samples: HashMap<String, Arc<Sample>>,
    recent: VecDeque<String>,
    max_bytes: usize,
    max_entries: usize,
}

impl Default for Cache {
    fn default() -> Self { Self { samples: HashMap::new(), recent: VecDeque::new(), max_bytes: 256 * 1024 * 1024, max_entries: 64 } }
}

impl Deref for Cache {
    type Target = HashMap<String, Arc<Sample>>;
    fn deref(&self) -> &Self::Target { &self.samples }
}

impl DerefMut for Cache {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.samples }
}

impl Cache {
    pub fn required(project: &Project) -> HashSet<String> {
        let mut needed: HashSet<String> = project.channels.iter().filter_map(|c| match &c.source {
            Source::Audio { path } | Source::Sampler { path, .. } => Some(path.clone()), _ => None,
        }).collect();
        for clip in &project.playlist.clips {
            if let ClipSource::Audio(channel) = clip.source && let Some(Source::Audio { path }) = project.channel(channel).map(|c| &c.source) {
                needed.insert(cache_key(path, clip.audio));
            }
        }
        needed
    }

    pub fn ready(&self, project: &Project) -> bool { Self::required(project).iter().all(|key| self.samples.contains_key(key)) }

    pub fn trim(&mut self, project: &Project) { self.retain_recent(&Self::required(project)); }

    fn retain_recent(&mut self, active: &HashSet<String>) {
        self.recent.retain(|key| self.samples.contains_key(key) && !active.contains(key));
        for key in self.samples.keys() {
            if !active.contains(key) && !self.recent.contains(key) { self.recent.push_front(key.clone()); }
        }
        for key in active { self.recent.push_back(key.clone()); }
        let bytes = |s: &Sample| (s.left.len() + s.right.len()) * size_of::<f32>();
        let mut inactive_bytes: usize = self.samples.iter().filter(|(k, _)| !active.contains(*k)).map(|(_, s)| bytes(s)).sum();
        let mut inactive_count = self.samples.keys().filter(|k| !active.contains(*k)).count();
        while inactive_bytes > self.max_bytes || inactive_count > self.max_entries {
            let Some(key) = self.recent.pop_front() else { break };
            if active.contains(&key) { continue; }
            if let Some(sample) = self.samples.remove(&key) {
                inactive_bytes -= bytes(&sample);
                inactive_count -= 1;
            }
        }
    }
}

pub fn cache_key(path: &str, edit: AudioEdit) -> String {
    if edit == AudioEdit::default() { return path.to_owned(); }
    format!("{path}\0{}:{}:{}", edit.stretch.to_bits(), edit.semitones.to_bits(), edit.reverse)
}

pub fn transform(sample: &Sample, edit: AudioEdit) -> Result<Sample, String> {
    transform_with_progress(sample, edit, |_| true)
}

/// Returning false stops between processing blocks, before publishing a result.
pub fn transform_with_progress(sample: &Sample, edit: AudioEdit, mut progress: impl FnMut(f32) -> bool) -> Result<Sample, String> {
    if !progress(0.0) { return Err("cancelled".into()); }
    if !edit.stretch.is_finite() || !(0.125..=8.0).contains(&edit.stretch) {
        return Err("audio stretch must be between 0.125 and 8".into());
    }
    if !edit.semitones.is_finite() || !(-48.0..=48.0).contains(&edit.semitones) {
        return Err("audio pitch must be between -48 and 48 semitones".into());
    }
    let mut left = sample.left.clone();
    let mut right = sample.right.clone();
    if edit.reverse { left.reverse(); right.reverse(); }
    if edit.stretch == 1.0 && edit.semitones == 0.0 {
        if !progress(1.0) { return Err("cancelled".into()); }
        return Ok(Sample { sample_rate: sample.sample_rate, left, right });
    }
    let frames = (left.len() as f64 * edit.stretch).round().max(1.0) as usize;
    let mut stretch = signalsmith_stretch::Stretch::preset_default(2, sample.sample_rate as u32);
    stretch.set_transpose_factor_semitones(edit.semitones, None);
    let input_latency = stretch.input_latency();
    let latency = stretch.output_latency();
    if frames < latency * 2 { return Err("audio is too short to stretch".into()); }
    let mut input: Vec<f32> = left.iter().zip(&right).flat_map(|(&l, &r)| [l, r]).collect();
    input.resize((left.len() + input_latency) * 2, 0.0);
    let mut output = vec![0.0; frames * 2];
    // Follow Signalsmith's exact-length padding and latency compensation,
    // feeding bounded blocks so a long file can report progress and stop.
    stretch.seek(&input[..input_latency * 2], left.len() as f64 / frames as f64);
    let (mut input_done, mut output_done) = (0, 0);
    while output_done < frames {
        if !progress(output_done as f32 / frames as f32) { return Err("cancelled".into()); }
        let output_end = (output_done + 4096).min(frames);
        let input_end = (output_end as f64 * left.len() as f64 / frames as f64).round() as usize;
        stretch.process(&input[(input_latency + input_done) * 2..(input_latency + input_end) * 2], &mut output[output_done * 2..output_end * 2]);
        input_done = input_end;
        output_done = output_end;
    }
    for i in 0..latency {
        for channel in 0..2 { output[(i + latency) * 2 + channel] -= output[(latency - 1 - i) * 2 + channel]; }
    }
    output.copy_within(latency * 2.., 0);
    stretch.flush(&mut output[(frames - latency) * 2..]);
    if !progress(1.0) { return Err("cancelled".into()); }
    Ok(Sample {
        sample_rate: sample.sample_rate,
        left: output.iter().step_by(2).copied().collect(),
        right: output.iter().skip(1).step_by(2).copied().collect(),
    })
}

pub fn prepare(project: &Project, samples: &mut Cache) -> Result<(), String> {
    prepare_with_progress(project, samples, |_| true)
}

pub fn prepare_with_progress(project: &Project, samples: &mut Cache, mut progress: impl FnMut(f32) -> bool) -> Result<(), String> {
    let needed = Cache::required(project);
    let work = project.channels.len() + project.playlist.clips.len();
    let mut done = 0;
    for channel in &project.channels {
        if !progress(done as f32 / work.max(1) as f32) { return Err("cancelled".into()); }
        if let Source::Audio { path } | Source::Sampler { path, .. } = &channel.source && !samples.contains_key(path) {
            samples.insert(path.clone(), Arc::new(Sample::load(path).map_err(|e| format!("could not load {path}: {e}"))?));
        }
        done += 1;
    }
    for clip in &project.playlist.clips {
        if !progress(done as f32 / work.max(1) as f32) { return Err("cancelled".into()); }
        let base = done;
        done += 1;
        let ClipSource::Audio(channel) = clip.source else { continue };
        let Some(Source::Audio { path }) = project.channel(channel).map(|c| &c.source) else { continue };
        let key = cache_key(path, clip.audio);
        if samples.contains_key(&key) { continue; }
        let source = match samples.get(path) {
            Some(sample) => sample.clone(),
            None => {
                let sample = Arc::new(Sample::load(path).map_err(|e| format!("could not load {path}: {e}"))?);
                samples.insert(path.clone(), sample.clone());
                sample
            }
        };
        let sample = transform_with_progress(&source, clip.audio, |p| progress((base as f32 + p) / work.max(1) as f32))?;
        samples.insert(key, Arc::new(sample));
    }
    samples.retain_recent(&needed);
    if !progress(1.0) { return Err("cancelled".into()); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone() -> Sample {
        let left: Vec<_> = (0..48_000).map(|i| (i as f32 * std::f32::consts::TAU * 440.0 / 48_000.0).sin()).collect();
        Sample { sample_rate: 48_000.0, right: left.clone(), left }
    }

    fn frequency(sample: &Sample) -> f64 {
        let signal = &sample.left[12_000..sample.left.len() - 12_000];
        let crossings = signal.windows(2).filter(|s| s[0] <= 0.0 && s[1] > 0.0).count();
        crossings as f64 * sample.sample_rate / signal.len() as f64
    }

    #[test]
    fn undo_reuses_transforms_and_eviction_preserves_active_audio() {
        let mut cache = Cache { max_bytes: 384_000, max_entries: 2, ..Default::default() };
        cache.insert("tone.wav".into(), Arc::new(tone()));
        let mut project = Project::new();
        let channel = project.add_channel("tone", Source::Audio { path: "tone.wav".into() });
        let clip = project.add_audio_clip(0, 0, channel, 1920);
        let edit = AudioEdit { semitones: 2.0, ..Default::default() };
        project.playlist.clips.iter_mut().find(|c| c.id == clip).unwrap().audio = edit;
        prepare(&project, &mut cache).unwrap();
        let original = cache[&cache_key("tone.wav", edit)].clone();
        project.playlist.clips[0].audio.semitones = 4.0;
        prepare(&project, &mut cache).unwrap();
        project.playlist.clips[0].audio = edit;
        prepare(&project, &mut cache).unwrap();
        assert!(Arc::ptr_eq(&original, &cache[&cache_key("tone.wav", edit)]));
        for pitch in [6.0, 8.0] {
            project.playlist.clips[0].audio.semitones = pitch;
            prepare(&project, &mut cache).unwrap();
        }
        assert!(!cache.contains_key(&cache_key("tone.wav", edit)));
        assert!(cache.contains_key("tone.wav"));
        assert!(cache.contains_key(&cache_key("tone.wav", project.playlist.clips[0].audio)));
        assert_eq!(cache.len(), 3); // Two active samples plus one inactive result.
        cache.max_bytes = 0;
        prepare(&project, &mut cache).unwrap();
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn stretch_preserves_pitch_and_pitch_preserves_duration() {
        let tone = tone();
        let stretched = transform(&tone, AudioEdit { stretch: 2.0, ..Default::default() }).unwrap();
        assert_eq!(stretched.left.len(), 96_000);
        assert!((frequency(&stretched) - 440.0).abs() < 5.0);
        let shifted = transform(&tone, AudioEdit { semitones: 12.0, ..Default::default() }).unwrap();
        assert_eq!(shifted.left.len(), tone.left.len());
        assert!((frequency(&shifted) - 880.0).abs() < 5.0);
    }

    #[test]
    fn block_processing_preserves_exact_length_stereo_pitch_and_level() {
        let mut sample = tone();
        sample.right = (0..48_000).map(|i| (i as f32 * std::f32::consts::TAU * 880.0 / 48_000.0).sin() * 0.25).collect();
        for ratio in [0.125, 0.333333, 1.0, 2.375, 8.0] {
            let edit = AudioEdit { stretch: ratio, semitones: 3.25, reverse: true };
            let mut updates = Vec::new();
            let actual = transform_with_progress(&sample, edit, |p| { updates.push(p); true }).unwrap();
            assert_eq!((*updates.first().unwrap(), *updates.last().unwrap()), (0.0, 1.0));
            assert!(updates.windows(2).all(|p| p[1] >= p[0]));
            let input: Vec<_> = sample.left.iter().rev().zip(sample.right.iter().rev()).flat_map(|(&l, &r)| [l, r]).collect();
            let mut expected = vec![0.0; actual.left.len() * 2];
            let mut stretch = signalsmith_stretch::Stretch::preset_default(2, 48_000);
            stretch.set_transpose_factor_semitones(edit.semitones, None);
            assert!(stretch.exact(&input, &mut expected));
            for (channel, offset) in [(&actual.left, 0), (&actual.right, 1)] {
                let reference: Vec<_> = expected.iter().skip(offset).step_by(2).copied().collect();
                let middle = |signal: &[f32]| signal[signal.len() / 4..signal.len() * 3 / 4].to_vec();
                let (actual, reference) = (middle(channel), middle(&reference));
                let frequency = |s: &[f32]| s.windows(2).filter(|v| v[0] <= 0.0 && v[1] > 0.0).count() as f64 * 48_000.0 / s.len() as f64;
                let rms = |s: &[f32]| (s.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / s.len() as f64).sqrt();
                assert!((frequency(&actual) - frequency(&reference)).abs() < 17.0, "stretch {ratio}, channel {offset}, pitch changed");
                let gain = rms(&actual) / rms(&reference);
                // Signalsmith randomizes phases above 2x, so independent runs differ in level.
                if ratio <= 2.0 { assert!((0.9..1.1).contains(&gain), "stretch {ratio}, channel {offset}, gain {gain}"); }
                assert!(actual.iter().all(|v| v.is_finite()));
                assert!(rms(&actual) > if offset == 0 { 0.4 } else { 0.1 });
            }
        }
    }

    #[test]
    fn cancellation_stops_before_publishing_a_partial_transform() {
        let edit = AudioEdit { stretch: 8.0, ..Default::default() };
        let mut updates = Vec::new();
        let result = transform_with_progress(&tone(), edit, |p| { updates.push(p); p < 0.1 });
        assert_eq!(result.err().as_deref(), Some("cancelled"));
        assert!(*updates.last().unwrap() >= 0.1 && *updates.last().unwrap() < 0.2);
        assert_eq!(transform_with_progress(&tone(), edit, |_| false).err().as_deref(), Some("cancelled"));
    }

    #[test]
    fn reverse_preserves_both_channels_and_rejects_invalid_edits() {
        let sample = Sample { sample_rate: 48_000.0, left: vec![1.0, 2.0, 3.0], right: vec![4.0, 5.0, 6.0] };
        let reversed = transform(&sample, AudioEdit { reverse: true, ..Default::default() }).unwrap();
        assert_eq!(reversed.left, vec![3.0, 2.0, 1.0]);
        assert_eq!(reversed.right, vec![6.0, 5.0, 4.0]);
        assert!(transform(&sample, AudioEdit { stretch: f64::NAN, ..Default::default() }).is_err());
        assert!(transform(&sample, AudioEdit { semitones: 49.0, ..Default::default() }).is_err());
    }
}
