//! Compiled playback plan. Built on the UI thread from a `Project` and handed to
//! the audio thread whole, so the audio thread never walks the project model.

use std::collections::HashMap;
use std::sync::Arc;

use daw_model::automation::Segment;
use daw_model::tempo::TempoMap;
use daw_model::time::{TimeSignature, Ticks};
use daw_model::{ClipSource, Project, Source, Target};

use crate::synth::Sample;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteEvent {
    pub tick: Ticks,
    pub key: u8,
    /// Zero for note-off.
    pub velocity: f32,
}

pub struct ChannelPlan {
    /// Node key of the channel's instrument.
    pub node: u64,
    pub insert: usize,
    pub volume: f32,
    pub pan: f32,
    pub mute: bool,
    /// Sorted by tick; note-offs sort before note-ons at the same tick.
    pub events: Vec<NoteEvent>,
    /// The file and playlist clips of an audio channel.
    pub audio: Option<AudioPlan>,
}

pub struct AudioPlan {
    /// Sorted by start.
    pub clips: Vec<AudioClip>,
}

#[derive(Clone)]
pub struct AudioClip {
    pub sample: Arc<Sample>,
    pub start: Ticks,
    pub end: Ticks,
    /// Ticks into the file at the clip's start.
    pub offset: Ticks,
}

pub struct InsertPlan {
    pub effects: Vec<u64>,
    pub volume: f32,
    pub pan: f32,
    /// Muted, or left out by a solo elsewhere.
    pub silent: bool,
    /// Index of the insert this one sums into; 0 is the master.
    pub output: usize,
    pub sends: Vec<SendPlan>,
    pub side_left: Box<[f32]>,
    pub side_right: Box<[f32]>,
    pub left: Box<[f32]>,
    pub right: Box<[f32]>,
}

pub struct SendPlan {
    pub to: usize,
    pub level: f32,
    pub sidechain: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EngineTarget {
    Node { node: u64, param: u32 },
    InsertVolume(usize),
    InsertPan(usize),
    ChannelVolume(usize),
    ChannelPan(usize),
}

pub struct AutomationPlan {
    pub target: EngineTarget,
    /// Sorted by start, non-overlapping as far as the playlist allows.
    pub segments: Vec<Segment>,
    /// The target's value outside its clips when playback starts, as FL
    /// Studio restores it. `None` for plugin parameters, whose values live
    /// in the plugin.
    pub base: Option<f32>,
    /// Last value applied, to skip redundant parameter events.
    pub last: f32,
}

impl AutomationPlan {
    pub fn value_at(&self, tick: f64) -> Option<f32> {
        self.segments.iter().rev().find(|s| (s.start as f64) <= tick && tick < s.end as f64)?.value_at(tick)
    }
}

pub struct Song {
    pub tempo: TempoMap,
    pub signature: TimeSignature,
    pub loop_range: Option<(Ticks, Ticks)>,
    pub channels: Vec<ChannelPlan>,
    /// Index 0 is the master insert.
    pub inserts: Vec<InsertPlan>,
    /// Non-master insert indices, each before the insert it sends to.
    pub order: Vec<usize>,
    pub automation: Vec<AutomationPlan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayMode {
    /// Loop one pattern, as in FL Studio's pattern mode.
    Pattern(daw_model::PatternId),
    Song,
}

/// Node key for a channel's instrument: the plugin instance id, or the channel
/// id for built-in sources. Ids share one counter, so keys never collide.
pub fn channel_node(source: &Source, channel: daw_model::ChannelId) -> u64 {
    match source {
        Source::Plugin(instance) => instance.0,
        Source::Synth(_) | Source::Sampler { .. } | Source::Audio { .. } => channel.0,
    }
}

/// Build the plan for `project`. `samples` holds the loaded files of audio
/// channels by path; channels whose file is missing stay silent.
pub fn compile(project: &Project, mode: PlayMode, max_block: usize, samples: &HashMap<String, Arc<Sample>>) -> Song {
    let insert_index = |id| project.mixer.inserts.iter().position(|i| i.id == id).unwrap_or(0);
    let mut channels: Vec<ChannelPlan> = project
        .channels
        .iter()
        .map(|c| ChannelPlan {
            node: channel_node(&c.source, c.id),
            insert: insert_index(c.insert),
            volume: c.volume,
            pan: c.pan,
            mute: c.mute,
            events: Vec::new(),
            audio: match &c.source {
                Source::Audio { path } => samples.get(path).map(|_| AudioPlan { clips: Vec::new() }),
                _ => None,
            },
        })
        .collect();
    let channel_index = |id| project.channels.iter().position(|c| c.id == id);

    let mut push_notes = |pattern: &daw_model::Pattern, start: Ticks, offset: Ticks, length: Ticks| {
        for lane in &pattern.lanes {
            let Some(index) = channel_index(lane.channel) else { continue };
            for note in &lane.notes {
                if note.start < offset || note.start >= offset + length {
                    continue;
                }
                let end = note.end().min(offset + length);
                let events = &mut channels[index].events;
                events.push(NoteEvent { tick: start + note.start - offset, key: note.key, velocity: note.velocity.max(0.001) });
                events.push(NoteEvent { tick: start + end - offset, key: note.key, velocity: 0.0 });
            }
        }
    };

    let mut audio_clips = Vec::new();
    let loop_range = match mode {
        PlayMode::Pattern(id) => {
            let pattern = project.pattern(id);
            if let Some(pattern) = pattern {
                push_notes(pattern, 0, 0, pattern.length);
            }
            Some((0, pattern.map(|p| p.length).unwrap_or(project.signature.ticks_per_bar())))
        }
        PlayMode::Song => {
            for clip in &project.playlist.clips {
                if clip.muted || project.playlist.tracks.get(clip.track).is_some_and(|t| t.mute) {
                    continue;
                }
                match clip.source {
                    ClipSource::Pattern(id) => {
                        if let Some(pattern) = project.pattern(id) {
                            push_notes(pattern, clip.start, clip.offset, clip.length);
                        }
                    }
                    ClipSource::Automation(_) => {}
                    ClipSource::Audio(id) => {
                        let Some(index) = channel_index(id) else { continue };
                        let Some(Source::Audio { path }) = project.channel(id).map(|c| &c.source) else { continue };
                        let Some(sample) = samples.get(&crate::audio::cache_key(path, clip.audio)) else { continue };
                        audio_clips.push((index, AudioClip { sample: sample.clone(), start: clip.start, end: clip.end(), offset: clip.offset }));
                    }
                }
            }
            project.playlist.loop_range
        }
    };
    let (automation, tempo) = match mode {
        PlayMode::Pattern(_) => (Vec::new(), TempoMap::new(project.bpm, &[])),
        PlayMode::Song => (automation_plans(project), project.tempo_map()),
    };
    for (index, clip) in audio_clips {
        if let Some(audio) = &mut channels[index].audio {
            audio.clips.push(clip);
        }
    }
    for channel in &mut channels {
        if let Some(audio) = &mut channel.audio {
            audio.clips.sort_by_key(|c| c.start);
        }
        channel.events.sort_by(|a, b| a.tick.cmp(&b.tick).then((a.velocity > 0.0).cmp(&(b.velocity > 0.0))));
    }

    let mixer = &project.mixer;
    let soloed: Vec<_> = mixer.inserts.iter().filter(|i| i.solo && i.id != daw_model::MASTER).map(|i| i.id).collect();
    // A solo keeps the inserts feeding it and the ones it feeds.
    let audible = |id| soloed.is_empty() || soloed.iter().any(|&s| s == id || mixer.feeds(id, s) || mixer.feeds(s, id));
    let inserts: Vec<InsertPlan> = mixer
        .inserts
        .iter()
        .enumerate()
        .map(|(index, i)| {
            let output = mixer.output(i.id).map_or(0, insert_index);
            InsertPlan {
                effects: i.effects.iter().map(|e| e.0).collect(),
                volume: i.volume,
                pan: i.pan,
                silent: i.mute || (index > 0 && !audible(i.id)),
                output: if output == index { 0 } else { output },
                sends: i.sends.iter().filter(|s| mixer.can_route(i.id, s.to)).map(|s| SendPlan {
                    to: insert_index(s.to), level: s.level, sidechain: s.sidechain,
                }).collect(),
                side_left: vec![0.0; max_block].into_boxed_slice(),
                side_right: vec![0.0; max_block].into_boxed_slice(),
                left: vec![0.0; max_block].into_boxed_slice(),
                right: vec![0.0; max_block].into_boxed_slice(),
            }
        })
        .collect();
    // Topological order includes detector routes, so sidechains reach the
    // destination before its effects run.
    let mut order = Vec::new();
    let mut incoming = vec![0usize; inserts.len()];
    for insert in inserts.iter().skip(1) {
        incoming[insert.output] += 1;
        for send in &insert.sends { incoming[send.to] += 1; }
    }
    let mut ready: Vec<usize> = (1..inserts.len()).filter(|&i| incoming[i] == 0).collect();
    while let Some(index) = ready.pop() {
        order.push(index);
        for destination in std::iter::once(inserts[index].output).chain(inserts[index].sends.iter().map(|s| s.to)) {
            incoming[destination] -= 1;
            if destination != 0 && incoming[destination] == 0 { ready.push(destination); }
        }
    }

    Song { tempo, signature: project.signature, loop_range, channels, inserts, order, automation }
}

/// One plan per automated target. Tempo automation plays through the
/// song's tempo map instead.
fn automation_plans(project: &Project) -> Vec<AutomationPlan> {
    let mut plans: Vec<AutomationPlan> = Vec::new();
    for (target, segment) in project.automation_segments(|t| t != Target::Tempo) {
        let Some(engine) = engine_target(project, target) else { continue };
        match plans.iter_mut().find(|p| p.target == engine) {
            Some(plan) => plan.segments.push(segment),
            None => plans.push(AutomationPlan { target: engine, segments: vec![segment], base: project.target_value(target), last: f32::NAN }),
        }
    }
    plans
}

/// Where `target` lives in a plan compiled from `project`. `None` when its
/// channel or insert is gone, and for the tempo, which the song's tempo map
/// sets.
pub fn engine_target(project: &Project, target: Target) -> Option<EngineTarget> {
    let insert = |id| project.mixer.inserts.iter().position(|i| i.id == id);
    let channel = |id| project.channels.iter().position(|c| c.id == id);
    Some(match target {
        Target::Plugin { instance, param } => EngineTarget::Node { node: instance.0, param },
        Target::InsertVolume(id) => EngineTarget::InsertVolume(insert(id)?),
        Target::InsertPan(id) => EngineTarget::InsertPan(insert(id)?),
        Target::ChannelVolume(id) => EngineTarget::ChannelVolume(channel(id)?),
        Target::ChannelPan(id) => EngineTarget::ChannelPan(channel(id)?),
        Target::SynthCutoff(id) => EngineTarget::Node { node: id.0, param: crate::synth::PARAM_CUTOFF },
        Target::Tempo => return None,
    })
}
