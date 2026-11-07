//! Small projects for headless design review: `daw --snap out.png --scene demo`.

use daw_model::automation::Point;
use daw_model::{ClipSource, Note, Source, SynthParams, Target, Waveform};

use super::App;

/// A short song: drums, bass, and chords in two patterns, arranged on the
/// playlist, with a filter sweep.
pub fn demo(app: &mut App) {
    app.scan = None;
    app.toast = None;
    let project = &mut app.project;
    project.name = "Night drive".into();
    project.bpm = 112.0;
    let synth = |waveform| Source::Synth(SynthParams { waveform, ..SynthParams::default() });
    let kick = project.add_channel("Kick", synth(Waveform::Sine));
    let hats = project.add_channel("Hats", synth(Waveform::Square));
    let bass = project.add_channel("Bass", synth(Waveform::Saw));
    let keys = project.add_channel("Keys", synth(Waveform::Square));
    let beat = project.patterns[0].id;
    let bar = project.signature.ticks_per_bar();
    project.set_pattern_length(beat, bar * 2);
    if let Some(pattern) = project.pattern_mut(beat) {
        pattern.name = "Beat".into();
        for step in [0, 4, 8, 11, 12, 16, 20, 24, 27, 28] { pattern.toggle_step(kick, step); }
        for step in (2..32).step_by(4) { pattern.toggle_step(hats, step); }
        let quarter = bar / 4;
        let line = [(36, 0, 3), (36, 3, 1), (43, 4, 2), (41, 6, 2)];
        let notes = pattern.notes_mut(bass);
        for (bar_index, base) in [(0, 0i32), (1, -2)] {
            for &(key, at, len) in &line {
                notes.push(Note { start: bar_index * bar + at * quarter / 2, length: len * quarter / 2, key: (key + base) as u8, velocity: 0.75 + 0.05 * at as f32 % 0.2 });
            }
        }
    }
    let chords = project.add_pattern();
    project.set_pattern_length(chords, bar * 2);
    if let Some(pattern) = project.pattern_mut(chords) {
        pattern.name = "Chords".into();
        let notes = pattern.notes_mut(keys);
        for (index, chord) in [[60u8, 64, 67, 71], [57, 60, 64, 67]].iter().enumerate() {
            for (voice, &key) in chord.iter().enumerate() {
                notes.push(Note { start: index as u64 * bar, length: bar - bar / 8, key, velocity: 0.55 + voice as f32 * 0.1 });
            }
        }
    }
    for index in 0..8u64 {
        project.add_clip(0, index * bar * 2, ClipSource::Pattern(beat));
    }
    for start in [bar * 4, bar * 8, bar * 12] {
        project.add_clip(1, start, ClipSource::Pattern(chords));
    }
    let sweep = project.add_automation("Bass filter", Target::SynthCutoff(bass), 0.2);
    if let Some(clip) = project.automation_clip_mut(sweep) {
        let length = clip.length;
        clip.envelope.points = vec![Point::new(0, 0.2), Point::new(length / 2, 0.85), Point::new(length, 0.35)];
    }
    project.add_clip(2, bar * 4, ClipSource::Automation(sweep));
    if let Some(track) = project.playlist.tracks.get_mut(0) { track.name = "Drums".into(); }
    if let Some(track) = project.playlist.tracks.get_mut(1) { track.name = "Keys".into(); }
    if let Some(track) = project.playlist.tracks.get_mut(2) { track.name = "Filter".into(); }
    app.selected_pattern = beat;
    app.selected_channel = Some(bass);
    app.automation.clip = Some(sweep);
    app.piano_roll.top_key = 52;
    app.position = f64::from(bar as u32) * 2.5;
    app.dirty = true;
    app.refresh();
}

/// The demo with the mixer in front and levels on the meters.
pub fn mixer(app: &mut App) {
    demo(app);
    app.show_panel(daw_model::layout::Panel::Mixer);
    app.meters = (0..app.project.mixer.inserts.len()).map(|i| (0.2 + 0.1 * (i % 5) as f32, 0.18 + 0.1 * (i % 4) as f32)).collect();
}
