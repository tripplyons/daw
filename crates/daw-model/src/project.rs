//! Project document: channels, patterns, playlist, mixer, automation, and plugin instances.

use serde::{Deserialize, Serialize};

use crate::automation::{Envelope, Point, tempo_to_normalized};
use crate::layout::Layout;
use crate::time::{Grid, TICKS_PER_BEAT, TimeSignature, Ticks};

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub struct $name(pub u64);
    };
}

id_type!(ChannelId);
id_type!(PatternId);
id_type!(AutomationId);
id_type!(ClipId);
id_type!(InsertId);
id_type!(InstanceId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PluginFormat {
    Vst3,
    AudioUnit,
}

impl std::fmt::Display for PluginFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PluginFormat::Vst3 => "VST3",
            PluginFormat::AudioUnit => "AU",
        })
    }
}

/// Identifies an installed plugin independent of where it was loaded from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PluginRef {
    pub format: PluginFormat,
    /// VST3: class ID as 32 hex chars. AU: "type subtype manufacturer" four-char codes.
    pub id: String,
    /// VST3 bundle path. Empty for AU, which is found through the component registry.
    pub path: String,
    pub name: String,
    pub vendor: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginInstance {
    pub id: InstanceId,
    pub plugin: PluginRef,
    #[serde(with = "base64_bytes", default)]
    pub state: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Waveform {
    Sine,
    Saw,
    Square,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SynthParams {
    pub waveform: Waveform,
    pub attack: f32,
    pub release: f32,
    pub cutoff: f32,
}

impl Default for SynthParams {
    fn default() -> Self {
        Self { waveform: Waveform::Saw, attack: 0.005, release: 0.15, cutoff: 0.7 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Source {
    Synth(SynthParams),
    /// One-shot sample; the root key plays at original pitch.
    Sampler { path: String, root_key: u8 },
    Plugin(InstanceId),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub id: ChannelId,
    pub name: String,
    pub source: Source,
    pub volume: f32,
    pub pan: f32,
    pub mute: bool,
    pub insert: InsertId,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub start: Ticks,
    pub length: Ticks,
    pub key: u8,
    pub velocity: f32,
}

impl Note {
    pub fn end(&self) -> Ticks {
        self.start + self.length
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lane {
    pub channel: ChannelId,
    pub notes: Vec<Note>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pattern {
    pub id: PatternId,
    pub name: String,
    pub length: Ticks,
    pub lanes: Vec<Lane>,
}

pub const STEP_TICKS: Ticks = TICKS_PER_BEAT as Ticks / 4;
pub const DEFAULT_KEY: u8 = 60;

impl Pattern {
    pub fn notes(&self, channel: ChannelId) -> &[Note] {
        self.lanes.iter().find(|l| l.channel == channel).map(|l| l.notes.as_slice()).unwrap_or(&[])
    }

    pub fn notes_mut(&mut self, channel: ChannelId) -> &mut Vec<Note> {
        let index = match self.lanes.iter().position(|l| l.channel == channel) {
            Some(index) => index,
            None => {
                self.lanes.push(Lane { channel, notes: Vec::new() });
                self.lanes.len() - 1
            }
        };
        &mut self.lanes[index].notes
    }

    /// A step is on when a note starts exactly on it, whatever its key.
    pub fn step_on(&self, channel: ChannelId, step: u64) -> bool {
        self.notes(channel).iter().any(|n| n.start == step * STEP_TICKS)
    }

    pub fn toggle_step(&mut self, channel: ChannelId, step: u64) {
        let start = step * STEP_TICKS;
        let notes = self.notes_mut(channel);
        if let Some(index) = notes.iter().position(|n| n.start == start) {
            notes.remove(index);
        } else {
            notes.push(Note { start, length: STEP_TICKS, key: DEFAULT_KEY, velocity: 0.8 });
            notes.sort_by_key(|n| n.start);
        }
    }

}

/// What an automation clip drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Target {
    Plugin { instance: InstanceId, param: u32 },
    InsertVolume(InsertId),
    InsertPan(InsertId),
    ChannelVolume(ChannelId),
    ChannelPan(ChannelId),
    SynthCutoff(ChannelId),
    Tempo,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutomationClip {
    pub id: AutomationId,
    pub name: String,
    pub target: Target,
    /// Point times are relative to the clip start.
    pub envelope: Envelope,
    pub length: Ticks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipSource {
    Pattern(PatternId),
    Automation(AutomationId),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: ClipId,
    pub track: usize,
    pub start: Ticks,
    pub length: Ticks,
    /// Offset into the source, for clips trimmed at the start.
    pub offset: Ticks,
    pub source: ClipSource,
}

impl Clip {
    pub fn end(&self) -> Ticks {
        self.start + self.length
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub name: String,
    pub mute: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Playlist {
    pub tracks: Vec<Track>,
    pub clips: Vec<Clip>,
    pub loop_range: Option<(Ticks, Ticks)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Insert {
    pub id: InsertId,
    pub name: String,
    /// Linear gain, 0..2 (about +6 dB at the top).
    pub volume: f32,
    /// -1 left .. 1 right.
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    pub effects: Vec<InstanceId>,
    /// Insert this one sends its output to. Ignored on the master.
    #[serde(default = "master")]
    pub output: InsertId,
}

pub const MASTER: InsertId = InsertId(0);

fn master() -> InsertId {
    MASTER
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mixer {
    /// The first insert is the master; others sum into it.
    pub inserts: Vec<Insert>,
}

impl Mixer {
    /// Where an insert's signal goes next, or `None` for the master.
    pub fn output(&self, id: InsertId) -> Option<InsertId> {
        if id == MASTER {
            return None;
        }
        let output = self.insert(id).map_or(MASTER, |i| i.output);
        Some(if self.insert(output).is_some() { output } else { MASTER })
    }

    /// Whether `from`'s signal reaches `to` through insert outputs.
    pub fn feeds(&self, from: InsertId, to: InsertId) -> bool {
        let mut at = from;
        for _ in 0..self.inserts.len() {
            match self.output(at) {
                Some(next) if next == to => return true,
                Some(next) => at = next,
                None => return false,
            }
        }
        false
    }

    /// Whether `from` may send to `to`: not itself, not the master's output,
    /// and not anything that already feeds `from`, which would loop.
    pub fn can_route(&self, from: InsertId, to: InsertId) -> bool {
        from != MASTER && from != to && self.insert(to).is_some() && !self.feeds(to, from)
    }

    /// Route an insert's output. Returns false when the route would loop.
    pub fn set_output(&mut self, from: InsertId, to: InsertId) -> bool {
        if !self.can_route(from, to) {
            return false;
        }
        if let Some(insert) = self.insert_mut(from) {
            insert.output = to;
        }
        true
    }

    pub fn insert(&self, id: InsertId) -> Option<&Insert> {
        self.inserts.iter().find(|i| i.id == id)
    }

    pub fn insert_mut(&mut self, id: InsertId) -> Option<&mut Insert> {
        self.inserts.iter_mut().find(|i| i.id == id)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspaces {
    pub active: usize,
    pub layouts: Vec<Layout>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub bpm: f64,
    pub signature: TimeSignature,
    pub channels: Vec<Channel>,
    pub patterns: Vec<Pattern>,
    pub automation: Vec<AutomationClip>,
    pub playlist: Playlist,
    pub mixer: Mixer,
    pub plugins: Vec<PluginInstance>,
    pub workspaces: Workspaces,
    pub grid: Grid,
    next_id: u64,
}

impl Default for Project {
    fn default() -> Self {
        Project::new()
    }
}

impl Project {
    /// A project with a master insert, 8 mixer inserts, one synth channel, one
    /// pattern, and 16 empty playlist tracks.
    pub fn new() -> Self {
        let mut project = Project {
            name: "untitled".into(),
            bpm: 140.0,
            signature: TimeSignature::default(),
            channels: Vec::new(),
            patterns: Vec::new(),
            automation: Vec::new(),
            playlist: Playlist {
                tracks: (1..=16).map(|i| Track { name: format!("track {i}"), mute: false }).collect(),
                clips: Vec::new(),
                loop_range: None,
            },
            mixer: Mixer { inserts: Vec::new() },
            plugins: Vec::new(),
            workspaces: Workspaces {
                active: 0,
                layouts: (0..9)
                    .map(|i| if i == 0 { Layout::default_workspace() } else { Layout::single(crate::layout::Panel::Playlist) })
                    .collect(),
            },
            grid: Grid::Division(16),
            next_id: 1,
        };
        project.mixer.inserts.push(Insert::new(MASTER, "master"));
        for i in 1..=8 {
            let id = InsertId(project.next_id());
            project.mixer.inserts.push(Insert::new(id, &format!("insert {i}")));
        }
        project.add_channel("synth", Source::Synth(SynthParams::default()));
        project.add_pattern();
        project
    }

    pub fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Add a channel routed to the first free insert (or master when all are used).
    pub fn add_channel(&mut self, name: &str, source: Source) -> ChannelId {
        let id = ChannelId(self.next_id());
        let used: Vec<InsertId> = self.channels.iter().map(|c| c.insert).collect();
        let insert = self.mixer.inserts.iter().skip(1).map(|i| i.id).find(|i| !used.contains(i)).unwrap_or(MASTER);
        self.channels.push(Channel { id, name: name.into(), source, volume: 0.8, pan: 0.0, mute: false, insert });
        id
    }

    pub fn add_pattern(&mut self) -> PatternId {
        let id = PatternId(self.next_id());
        let number = self.patterns.len() + 1;
        let length = self.signature.ticks_per_bar();
        self.patterns.push(Pattern { id, name: format!("pattern {number}"), length, lanes: Vec::new() });
        id
    }

    /// Set a pattern's length. Playlist clips that show the whole pattern
    /// follow it; clips trimmed to a part of it keep their length.
    pub fn set_pattern_length(&mut self, id: PatternId, length: Ticks) {
        let Some(pattern) = self.patterns.iter_mut().find(|p| p.id == id) else { return };
        let old = std::mem::replace(&mut pattern.length, length.max(1));
        self.resize_whole_clips(ClipSource::Pattern(id), old, length.max(1));
    }

    /// Set an automation clip's length, with the same playlist rule as patterns.
    pub fn set_automation_length(&mut self, id: AutomationId, length: Ticks) {
        let Some(clip) = self.automation_clip_mut(id) else { return };
        let old = std::mem::replace(&mut clip.length, length.max(1));
        self.resize_whole_clips(ClipSource::Automation(id), old, length.max(1));
    }

    fn resize_whole_clips(&mut self, source: ClipSource, old: Ticks, new: Ticks) {
        for clip in &mut self.playlist.clips {
            if clip.source == source && clip.offset == 0 && clip.length == old {
                clip.length = new;
            }
        }
    }

    /// Whole bars that reach `end`, for growing a pattern or automation clip
    /// when something is placed past its end.
    pub fn bars_to(&self, end: Ticks) -> Ticks {
        let bar = self.signature.ticks_per_bar();
        end.div_ceil(bar).max(1) * bar
    }

    pub fn add_plugin(&mut self, plugin: PluginRef) -> InstanceId {
        let id = InstanceId(self.next_id());
        self.plugins.push(PluginInstance { id, plugin, state: Vec::new() });
        id
    }

    pub fn add_automation(&mut self, name: &str, target: Target, start_value: f32) -> AutomationId {
        let id = AutomationId(self.next_id());
        let length = self.signature.ticks_per_bar() * 4;
        let envelope = Envelope { points: vec![Point::new(0, start_value), Point::new(length, start_value)] };
        self.automation.push(AutomationClip { id, name: name.into(), target, envelope, length });
        id
    }

    pub fn add_clip(&mut self, track: usize, start: Ticks, source: ClipSource) -> ClipId {
        let id = ClipId(self.next_id());
        let length = match source {
            ClipSource::Pattern(p) => self.pattern(p).map(|p| p.length),
            ClipSource::Automation(a) => self.automation_clip(a).map(|a| a.length),
        }
        .unwrap_or(self.signature.ticks_per_bar());
        self.playlist.clips.push(Clip { id, track, start, length, offset: 0, source });
        id
    }

    pub fn channel(&self, id: ChannelId) -> Option<&Channel> {
        self.channels.iter().find(|c| c.id == id)
    }

    pub fn channel_mut(&mut self, id: ChannelId) -> Option<&mut Channel> {
        self.channels.iter_mut().find(|c| c.id == id)
    }

    pub fn pattern(&self, id: PatternId) -> Option<&Pattern> {
        self.patterns.iter().find(|p| p.id == id)
    }

    pub fn pattern_mut(&mut self, id: PatternId) -> Option<&mut Pattern> {
        self.patterns.iter_mut().find(|p| p.id == id)
    }

    pub fn automation_clip(&self, id: AutomationId) -> Option<&AutomationClip> {
        self.automation.iter().find(|a| a.id == id)
    }

    pub fn automation_clip_mut(&mut self, id: AutomationId) -> Option<&mut AutomationClip> {
        self.automation.iter_mut().find(|a| a.id == id)
    }

    pub fn plugin(&self, id: InstanceId) -> Option<&PluginInstance> {
        self.plugins.iter().find(|p| p.id == id)
    }

    pub fn plugin_mut(&mut self, id: InstanceId) -> Option<&mut PluginInstance> {
        self.plugins.iter_mut().find(|p| p.id == id)
    }

    /// Current normalized value of a built-in target. `None` for plugin
    /// parameters, whose values live in the loaded plugin, and for targets
    /// whose channel or insert is gone.
    pub fn target_value(&self, target: Target) -> Option<f32> {
        match target {
            Target::Plugin { .. } => None,
            Target::InsertVolume(id) => self.mixer.insert(id).map(|i| i.volume / 2.0),
            Target::InsertPan(id) => self.mixer.insert(id).map(|i| (i.pan + 1.0) / 2.0),
            Target::ChannelVolume(id) => self.channel(id).map(|c| c.volume),
            Target::ChannelPan(id) => self.channel(id).map(|c| (c.pan + 1.0) / 2.0),
            Target::SynthCutoff(id) => match &self.channel(id)?.source {
                Source::Synth(params) => Some(params.cutoff),
                _ => None,
            },
            Target::Tempo => Some(tempo_to_normalized(self.bpm)),
        }
    }

    /// Existing automation clip for a target, if any.
    pub fn automation_for(&self, target: Target) -> Option<AutomationId> {
        self.automation.iter().find(|a| a.target == target).map(|a| a.id)
    }

    /// The first playlist track with no clip overlapping `start..end`, or a new track.
    pub fn free_track(&mut self, start: Ticks, end: Ticks) -> usize {
        let busy = |track: usize| {
            self.playlist.clips.iter().any(|c| c.track == track && c.start < end && c.end() > start)
        };
        if let Some(track) = (0..self.playlist.tracks.len()).find(|&t| !busy(t)) {
            return track;
        }
        let number = self.playlist.tracks.len() + 1;
        self.playlist.tracks.push(Track { name: format!("track {number}"), mute: false });
        self.playlist.tracks.len() - 1
    }

    /// End of the last playlist clip.
    pub fn song_length(&self) -> Ticks {
        self.playlist.clips.iter().map(Clip::end).max().unwrap_or(0)
    }

    /// Remove a plugin instance and every reference to it.
    pub fn remove_plugin(&mut self, id: InstanceId) {
        self.plugins.retain(|p| p.id != id);
        for insert in &mut self.mixer.inserts {
            insert.effects.retain(|&e| e != id);
        }
        let dead: Vec<AutomationId> = self
            .automation
            .iter()
            .filter(|a| matches!(a.target, Target::Plugin { instance, .. } if instance == id))
            .map(|a| a.id)
            .collect();
        for automation in dead {
            self.remove_automation(automation);
        }
    }

    /// Remove a pattern and its playlist clips.
    pub fn remove_pattern(&mut self, id: PatternId) {
        self.patterns.retain(|p| p.id != id);
        self.playlist.clips.retain(|c| c.source != ClipSource::Pattern(id));
    }

    pub fn remove_automation(&mut self, id: AutomationId) {
        self.automation.retain(|a| a.id != id);
        self.playlist.clips.retain(|c| c.source != ClipSource::Automation(id));
    }

    pub fn remove_channel(&mut self, id: ChannelId) {
        let Some(channel) = self.channel(id) else { return };
        if let Source::Plugin(instance) = channel.source {
            self.remove_plugin(instance);
        }
        self.channels.retain(|c| c.id != id);
        for pattern in &mut self.patterns {
            pattern.lanes.retain(|l| l.channel != id);
        }
        let dead: Vec<AutomationId> = self
            .automation
            .iter()
            .filter(|a| {
                matches!(a.target, Target::ChannelVolume(c) | Target::ChannelPan(c) | Target::SynthCutoff(c) if c == id)
            })
            .map(|a| a.id)
            .collect();
        for automation in dead {
            self.remove_automation(automation);
        }
    }

    /// Remove a mixer insert with its effects and automation. Channels routed
    /// to it go to the master. The master itself cannot be removed.
    pub fn remove_insert(&mut self, id: InsertId) {
        if id == MASTER {
            return;
        }
        let Some(insert) = self.mixer.insert(id) else { return };
        for effect in insert.effects.clone() {
            self.remove_plugin(effect);
        }
        for channel in &mut self.channels {
            if channel.insert == id {
                channel.insert = MASTER;
            }
        }
        // Inserts that fed this one send to where it went.
        let next = self.mixer.output(id).unwrap_or(MASTER);
        for other in &mut self.mixer.inserts {
            if other.output == id {
                other.output = next;
            }
        }
        let dead: Vec<AutomationId> = self
            .automation
            .iter()
            .filter(|a| matches!(a.target, Target::InsertVolume(i) | Target::InsertPan(i) if i == id))
            .map(|a| a.id)
            .collect();
        for automation in dead {
            self.remove_automation(automation);
        }
        self.mixer.inserts.retain(|i| i.id != id);
    }

    /// Remove a playlist track and its clips; later tracks move up. The
    /// playlist always keeps at least one track.
    pub fn remove_track(&mut self, index: usize) {
        if index >= self.playlist.tracks.len() {
            return;
        }
        self.playlist.tracks.remove(index);
        self.playlist.clips.retain(|c| c.track != index);
        for clip in &mut self.playlist.clips {
            if clip.track > index {
                clip.track -= 1;
            }
        }
        if self.playlist.tracks.is_empty() {
            self.playlist.tracks.push(Track { name: "track 1".into(), mute: false });
        }
    }

    pub fn to_ron(&self) -> Result<String, ron::Error> {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
    }

    pub fn from_ron(text: &str) -> Result<Project, ron::error::SpannedError> {
        ron::from_str(text)
    }
}

impl Insert {
    pub fn new(id: InsertId, name: &str) -> Self {
        Self { id, name: name.into(), volume: 0.8, pan: 0.0, mute: false, solo: false, effects: Vec::new(), output: MASTER }
    }
}

mod base64_bytes {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        STANDARD.decode(text).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::Shape;
    use crate::layout::{Axis, Panel};

    fn plugin(name: &str) -> PluginRef {
        PluginRef { format: PluginFormat::Vst3, id: name.into(), path: String::new(), name: name.into(), vendor: String::new() }
    }

    #[test]
    fn removing_an_insert_reroutes_channels_and_drops_effects() {
        let mut project = Project::new();
        let insert = project.channels[0].insert;
        let effect = project.add_plugin(plugin("fx"));
        project.mixer.insert_mut(insert).unwrap().effects.push(effect);
        project.add_automation("volume", Target::InsertVolume(insert), 0.5);
        project.add_automation("tempo", Target::Tempo, 0.5);
        project.remove_insert(insert);
        assert!(project.mixer.insert(insert).is_none());
        assert_eq!(project.channels[0].insert, MASTER);
        assert!(project.plugins.is_empty());
        assert_eq!(project.automation.len(), 1);
        project.remove_insert(MASTER);
        assert!(project.mixer.insert(MASTER).is_some());
    }

    #[test]
    fn removing_a_track_drops_its_clips_and_shifts_later_ones() {
        let mut project = Project::new();
        let pattern = project.patterns[0].id;
        project.add_clip(0, 0, ClipSource::Pattern(pattern));
        project.add_clip(1, 0, ClipSource::Pattern(pattern));
        project.add_clip(3, 0, ClipSource::Pattern(pattern));
        let tracks = project.playlist.tracks.len();
        project.remove_track(1);
        let lanes: Vec<usize> = project.playlist.clips.iter().map(|c| c.track).collect();
        assert_eq!(lanes, vec![0, 2]);
        assert_eq!(project.playlist.tracks.len(), tracks - 1);
        assert_eq!(project.playlist.tracks[1].name, "track 3");
        while project.playlist.tracks.len() > 1 {
            project.remove_track(0);
        }
        project.remove_track(0);
        assert_eq!(project.playlist.tracks.len(), 1);
        assert!(project.playlist.clips.is_empty());
    }

    #[test]
    fn ron_round_trip_preserves_everything() {
        let mut project = Project::new();
        let channel = project.channels[0].id;
        let pattern = project.patterns[0].id;
        project.pattern_mut(pattern).unwrap().toggle_step(channel, 4);
        let instance = project.add_plugin(PluginRef {
            format: PluginFormat::Vst3,
            id: "0123456789ABCDEF0123456789ABCDEF".into(),
            path: "/Library/Audio/Plug-Ins/VST3/Vital.vst3".into(),
            name: "Vital".into(),
            vendor: "Vital Audio".into(),
        });
        project.plugin_mut(instance).unwrap().state = vec![0, 1, 2, 250, 255];
        let plugin_channel = project.add_channel("vital", Source::Plugin(instance));
        project.pattern_mut(pattern).unwrap().notes_mut(plugin_channel).push(Note {
            start: 0,
            length: 480,
            key: 64,
            velocity: 1.0,
        });
        let automation = project.add_automation("cutoff", Target::Plugin { instance, param: 42 }, 0.5);
        project.automation_clip_mut(automation).unwrap().envelope.insert(Point {
            time: 960,
            value: 0.9,
            shape: Shape::Curve,
            tension: 0.4,
        });
        project.add_clip(0, 0, ClipSource::Pattern(pattern));
        project.add_clip(1, 0, ClipSource::Automation(automation));
        project.mixer.inserts[1].effects.push(instance);
        project.workspaces.layouts[1].split(Axis::Horizontal, Panel::Mixer);

        let text = project.to_ron().unwrap();
        let loaded = Project::from_ron(&text).unwrap();
        assert_eq!(loaded, project);
        assert_eq!(loaded.plugin(instance).unwrap().state, vec![0, 1, 2, 250, 255]);
    }

    #[test]
    fn toggle_step_adds_and_removes() {
        let mut project = Project::new();
        let channel = project.channels[0].id;
        let pattern = project.pattern_mut(project.patterns[0].id).unwrap();
        pattern.toggle_step(channel, 3);
        assert!(pattern.step_on(channel, 3));
        pattern.toggle_step(channel, 3);
        assert!(!pattern.step_on(channel, 3));
    }

    #[test]
    fn channels_get_distinct_inserts() {
        let mut project = Project::new();
        let second = project.add_channel("b", Source::Synth(SynthParams::default()));
        assert_ne!(project.channels[0].insert, project.channel(second).unwrap().insert);
        assert_ne!(project.channels[0].insert, MASTER);
    }

    #[test]
    fn removing_plugin_removes_its_automation() {
        let mut project = Project::new();
        let instance = project.add_plugin(PluginRef {
            format: PluginFormat::AudioUnit,
            id: "aufx Tpe2 AirW".into(),
            path: String::new(),
            name: "Air".into(),
            vendor: "airwindows".into(),
        });
        let automation = project.add_automation("x", Target::Plugin { instance, param: 0 }, 0.0);
        project.add_clip(0, 0, ClipSource::Automation(automation));
        project.remove_plugin(instance);
        assert!(project.automation.is_empty());
        assert!(project.playlist.clips.is_empty());
    }

    #[test]
    fn free_track_skips_busy_tracks() {
        let mut project = Project::new();
        let pattern = project.patterns[0].id;
        project.add_clip(0, 0, ClipSource::Pattern(pattern));
        assert_eq!(project.free_track(0, 100), 1);
        assert_eq!(project.free_track(5000, 6000), 0);
    }

    #[test]
    fn pattern_length_moves_whole_clips_only() {
        let mut project = Project::new();
        let pattern = project.patterns[0].id;
        let bar = project.signature.ticks_per_bar();
        project.add_clip(0, 0, ClipSource::Pattern(pattern));
        project.add_clip(1, 0, ClipSource::Pattern(pattern));
        project.playlist.clips[1].length = bar / 2;
        project.set_pattern_length(pattern, bar * 3);
        assert_eq!(project.patterns[0].length, bar * 3);
        assert_eq!(project.playlist.clips[0].length, bar * 3);
        assert_eq!(project.playlist.clips[1].length, bar / 2);

        assert_eq!(project.bars_to(bar * 2 + 10), bar * 3);
        assert_eq!(project.bars_to(0), bar);
    }

    #[test]
    fn automation_length_moves_whole_clips_only() {
        let mut project = Project::new();
        let bar = project.signature.ticks_per_bar();
        let automation = project.add_automation("x", Target::Tempo, 0.5);
        let length = project.automation_clip(automation).unwrap().length;
        project.add_clip(0, 0, ClipSource::Automation(automation));
        project.add_clip(1, 0, ClipSource::Automation(automation));
        project.playlist.clips[1].offset = 10;
        project.set_automation_length(automation, bar * 5);
        assert_eq!(project.automation_clip(automation).unwrap().length, bar * 5);
        assert_eq!(project.playlist.clips[0].length, bar * 5);
        assert_eq!(project.playlist.clips[1].length, length);
    }

    #[test]
    fn insert_outputs_refuse_loops_and_survive_removal() {
        let mut project = Project::new();
        let [a, b, c] = [1, 2, 3].map(|i| project.mixer.inserts[i].id);
        let mixer = &mut project.mixer;
        assert!(mixer.set_output(a, b));
        assert!(mixer.set_output(b, c));
        assert!(mixer.feeds(a, c) && mixer.feeds(a, MASTER));
        assert!(!mixer.set_output(c, a), "c feeds back into a");
        assert!(!mixer.set_output(a, a));
        assert!(!mixer.set_output(MASTER, a));
        project.remove_insert(b);
        assert_eq!(project.mixer.output(a), Some(c));
    }
}
