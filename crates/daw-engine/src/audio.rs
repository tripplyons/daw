//! Audio edits are prepared off the audio thread and shared by playback plans.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use daw_model::{AudioEdit, ClipSource, Project, Source};
use crate::synth::Sample;

pub fn cache_key(path: &str, edit: AudioEdit) -> String {
    if edit == AudioEdit::default() { return path.to_owned(); }
    format!("{path}\0{}:{}:{}", edit.stretch.to_bits(), edit.semitones.to_bits(), edit.reverse)
}

pub fn transform(sample: &Sample, edit: AudioEdit) -> Result<Sample, String> {
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
        return Ok(Sample { sample_rate: sample.sample_rate, left, right });
    }
    let input: Vec<f32> = left.iter().zip(&right).flat_map(|(&l, &r)| [l, r]).collect();
    let frames = (left.len() as f64 * edit.stretch).round().max(1.0) as usize;
    let mut output = vec![0.0; frames * 2];
    let mut stretch = signalsmith_stretch::Stretch::preset_default(2, sample.sample_rate as u32);
    stretch.set_transpose_factor_semitones(edit.semitones, None);
    if !stretch.exact(&input, &mut output) { return Err("could not stretch the audio file".into()); }
    Ok(Sample {
        sample_rate: sample.sample_rate,
        left: output.iter().step_by(2).copied().collect(),
        right: output.iter().skip(1).step_by(2).copied().collect(),
    })
}

pub fn prepare(project: &Project, samples: &mut HashMap<String, Arc<Sample>>) -> Result<(), String> {
    let mut needed: HashSet<String> = project.channels.iter().filter_map(|c| match &c.source {
        Source::Audio { path } | Source::Sampler { path, .. } => Some(path.clone()),
        _ => None,
    }).collect();
    for clip in &project.playlist.clips {
        if let ClipSource::Audio(channel) = clip.source
            && let Some(Source::Audio { path }) = project.channel(channel).map(|c| &c.source) {
            needed.insert(cache_key(path, clip.audio));
        }
    }
    samples.retain(|key, _| needed.contains(key));
    for clip in &project.playlist.clips {
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
        samples.insert(key, Arc::new(transform(&source, clip.audio)?));
    }
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
    fn reverse_preserves_both_channels_and_rejects_invalid_edits() {
        let sample = Sample { sample_rate: 48_000.0, left: vec![1.0, 2.0, 3.0], right: vec![4.0, 5.0, 6.0] };
        let reversed = transform(&sample, AudioEdit { reverse: true, ..Default::default() }).unwrap();
        assert_eq!(reversed.left, vec![3.0, 2.0, 1.0]);
        assert_eq!(reversed.right, vec![6.0, 5.0, 4.0]);
        assert!(transform(&sample, AudioEdit { stretch: f64::NAN, ..Default::default() }).is_err());
        assert!(transform(&sample, AudioEdit { semitones: 49.0, ..Default::default() }).is_err());
    }
}
