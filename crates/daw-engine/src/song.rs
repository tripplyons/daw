//! Compiled playback plan. Built on the UI thread from a `Project` and handed to
//! the audio thread whole, so the audio thread never walks the project model.

use daw_model::automation::Envelope;
use daw_model::time::{TimeSignature, Ticks};
use daw_model::{ClipSource, Project, Source, Target};

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
}

pub struct InsertPlan {
    pub effects: Vec<u64>,
    pub volume: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    pub left: Box<[f32]>,
    pub right: Box<[f32]>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EngineTarget {
    Node { node: u64, param: u32 },
    InsertVolume(usize),
    InsertPan(usize),
    ChannelVolume(usize),
    ChannelPan(usize),
    Tempo,
}

pub struct Segment {
    pub start: Ticks,
    pub end: Ticks,
    pub offset: Ticks,
    /// Loop length of the automation clip; the envelope repeats past it.
    pub length: Ticks,
    pub envelope: Envelope,
}

pub struct AutomationPlan {
    pub target: EngineTarget,
    /// Sorted by start, non-overlapping as far as the playlist allows.
    pub segments: Vec<Segment>,
    /// Last value applied, to skip redundant parameter events.
    pub last: f32,
}

impl AutomationPlan {
    pub fn value_at(&self, tick: f64) -> Option<f32> {
        let segment = self.segments.iter().rev().find(|s| (s.start as f64) <= tick && tick < s.end as f64)?;
        let local = tick - segment.start as f64 + segment.offset as f64;
        let local = if segment.length > 0 { local % segment.length as f64 } else { local };
        segment.envelope.value_at(local)
    }
}

pub struct Song {
    pub bpm: f64,
    pub signature: TimeSignature,
    pub loop_range: Option<(Ticks, Ticks)>,
    pub channels: Vec<ChannelPlan>,
    /// Index 0 is the master insert.
    pub inserts: Vec<InsertPlan>,
    pub automation: Vec<AutomationPlan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayMode {
    /// Loop one pattern, as in FL Studio's pattern mode.
    Pattern(daw_model::PatternId),
    Song,
}

pub const TEMPO_MIN: f64 = 40.0;
pub const TEMPO_MAX: f64 = 240.0;

pub fn tempo_from_normalized(value: f32) -> f64 {
    TEMPO_MIN + f64::from(value) * (TEMPO_MAX - TEMPO_MIN)
}

pub fn tempo_to_normalized(bpm: f64) -> f32 {
    ((bpm - TEMPO_MIN) / (TEMPO_MAX - TEMPO_MIN)).clamp(0.0, 1.0) as f32
}

/// Node key for a channel's instrument: the plugin instance id, or the channel
/// id for built-in sources. Ids share one counter, so keys never collide.
pub fn channel_node(source: &Source, channel: daw_model::ChannelId) -> u64 {
    match source {
        Source::Plugin(instance) => instance.0,
        Source::Synth(_) | Source::Sampler { .. } => channel.0,
    }
}

pub fn compile(project: &Project, mode: PlayMode, max_block: usize) -> Song {
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

    let mut automation: Vec<AutomationPlan> = Vec::new();
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
                if project.playlist.tracks.get(clip.track).is_some_and(|t| t.mute) {
                    continue;
                }
                match clip.source {
                    ClipSource::Pattern(id) => {
                        if let Some(pattern) = project.pattern(id) {
                            push_notes(pattern, clip.start, clip.offset, clip.length);
                        }
                    }
                    ClipSource::Automation(id) => {
                        let Some(source) = project.automation_clip(id) else { continue };
                        let Some(target) = engine_target(project, source.target) else { continue };
                        let segment = Segment {
                            start: clip.start,
                            end: clip.end(),
                            offset: clip.offset,
                            length: source.length,
                            envelope: source.envelope.clone(),
                        };
                        match automation.iter_mut().find(|a| a.target == target) {
                            Some(plan) => plan.segments.push(segment),
                            None => automation.push(AutomationPlan { target, segments: vec![segment], last: f32::NAN }),
                        }
                    }
                }
            }
            project.playlist.loop_range
        }
    };
    for plan in &mut automation {
        plan.segments.sort_by_key(|s| s.start);
    }
    for channel in &mut channels {
        channel.events.sort_by(|a, b| a.tick.cmp(&b.tick).then((a.velocity > 0.0).cmp(&(b.velocity > 0.0))));
    }

    let inserts = project
        .mixer
        .inserts
        .iter()
        .map(|i| InsertPlan {
            effects: i.effects.iter().map(|e| e.0).collect(),
            volume: i.volume,
            pan: i.pan,
            mute: i.mute,
            solo: i.solo,
            left: vec![0.0; max_block].into_boxed_slice(),
            right: vec![0.0; max_block].into_boxed_slice(),
        })
        .collect();

    Song { bpm: project.bpm, signature: project.signature, loop_range, channels, inserts, automation }
}

fn engine_target(project: &Project, target: Target) -> Option<EngineTarget> {
    let insert = |id| project.mixer.inserts.iter().position(|i| i.id == id);
    let channel = |id| project.channels.iter().position(|c| c.id == id);
    Some(match target {
        Target::Plugin { instance, param } => EngineTarget::Node { node: instance.0, param },
        Target::InsertVolume(id) => EngineTarget::InsertVolume(insert(id)?),
        Target::InsertPan(id) => EngineTarget::InsertPan(insert(id)?),
        Target::ChannelVolume(id) => EngineTarget::ChannelVolume(channel(id)?),
        Target::ChannelPan(id) => EngineTarget::ChannelPan(channel(id)?),
        Target::SynthCutoff(id) => EngineTarget::Node { node: id.0, param: crate::synth::PARAM_CUTOFF },
        Target::Tempo => EngineTarget::Tempo,
    })
}
