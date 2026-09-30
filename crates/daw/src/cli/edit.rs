//! `daw edit`: one change to a project per call.

use clap::Subcommand;
use daw_model::automation::{Point, Shape};
use daw_model::time::{Grid, TimeSignature, Ticks};
use daw_model::{
    AutomationClip, AutomationId, Channel, ChannelId, Clip, ClipId, ClipSource, Insert, InsertId, InstanceId, MASTER, Note,
    Pattern, PatternId, Project, STEP_TICKS, Source, SynthParams, Target, Track, Waveform,
};
use daw_plugins::PluginKind;
use daw_plugins::scan::Catalog;

use super::parse::{self, Time};
use crate::app::audio::file_length;
use super::{find_plugin, ticks};

#[derive(Subcommand)]
pub enum Op {
    /// Set project-wide values.
    Set {
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        bpm: Option<f64>,
        /// Time signature, e.g. 3/4.
        #[arg(long, value_parser = parse::signature)]
        signature: Option<TimeSignature>,
        /// Snap grid for the editors: off, bar, beat, 1/16, 1/8T, 1/8.
        #[arg(long, value_parser = parse::grid)]
        grid: Option<Grid>,
        /// Song loop range, e.g. 0..8bar.
        #[arg(long = "loop", value_parser = parse::range, conflicts_with = "no_loop")]
        loop_range: Option<(Time, Time)>,
        #[arg(long)]
        no_loop: bool,
    },
    /// Instrument channels in the channel rack.
    #[command(subcommand)]
    Channel(ChannelOp),
    /// Patterns, which hold notes for any channels.
    #[command(subcommand)]
    Pattern(PatternOp),
    /// Notes in a pattern, per channel.
    #[command(subcommand)]
    Note(NoteOp),
    /// Playlist clips that place a pattern or automation clip in the song.
    #[command(subcommand)]
    Clip(ClipOp),
    /// Playlist tracks, addressed by index from 0.
    #[command(subcommand)]
    Track(TrackOp),
    /// Mixer inserts. Insert 0 is the master.
    #[command(subcommand)]
    Insert(InsertOp),
    /// Effect plugins on mixer inserts.
    #[command(subcommand)]
    Effect(EffectOp),
    /// Automation clips: envelopes over one target.
    #[command(subcommand)]
    Automation(AutomationOp),
    /// Envelope points in an automation clip, addressed by index from 0.
    #[command(subcommand)]
    Point(PointOp),
}

#[derive(Subcommand)]
pub enum ChannelOp {
    /// Add a channel routed to the first free insert. Defaults to the built-in saw synth.
    Add {
        name: String,
        /// Built-in synth waveform: sine, saw, or square.
        #[arg(long, value_parser = parse::waveform, group = "source")]
        synth: Option<Waveform>,
        /// One-shot WAV sample.
        #[arg(long, group = "source")]
        sampler: Option<String>,
        /// WAV file for audio clips, which play it at its own speed. Place it with `clip add audio:ID`.
        #[arg(long, group = "source")]
        audio: Option<String>,
        /// Instrument plugin, by id or name from `daw plugins`.
        #[arg(long, group = "source")]
        plugin: Option<String>,
        /// Key that plays the sample at its original pitch.
        #[arg(long, value_parser = parse::key, default_value = "C4", requires = "sampler")]
        root: u8,
    },
    Set {
        id: u64,
        #[arg(long)]
        name: Option<String>,
        /// 0 to 1.
        #[arg(long, value_parser = parse::unit)]
        volume: Option<f32>,
        /// -1 (left) to 1 (right).
        #[arg(long, value_parser = parse::pan, allow_hyphen_values = true)]
        pan: Option<f32>,
        #[arg(long)]
        mute: Option<bool>,
        /// Mixer insert id.
        #[arg(long)]
        insert: Option<u64>,
        /// Synth only: sine, saw, or square.
        #[arg(long, value_parser = parse::waveform)]
        waveform: Option<Waveform>,
        /// Synth only: seconds, 0 to 2.
        #[arg(long, value_parser = |t: &str| parse::bounded(t, 0.0, 2.0))]
        attack: Option<f32>,
        /// Synth only: seconds, 0 to 4.
        #[arg(long, value_parser = |t: &str| parse::bounded(t, 0.0, 4.0))]
        release: Option<f32>,
        /// Synth only: 0 to 1, from 40 Hz to 18 kHz.
        #[arg(long, value_parser = parse::unit)]
        cutoff: Option<f32>,
        /// Sampler or audio channel: WAV path.
        #[arg(long)]
        sample: Option<String>,
        /// Sampler only.
        #[arg(long, value_parser = parse::key)]
        root: Option<u8>,
    },
    /// Remove a channel with its notes, plugin, and automation.
    Remove { id: u64 },
}

#[derive(Subcommand)]
pub enum PatternOp {
    /// Add an empty one-bar pattern.
    Add {
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        length: Option<Time>,
    },
    Set {
        id: u64,
        #[arg(long)]
        name: Option<String>,
        /// Playlist clips that show the whole pattern follow the new length.
        #[arg(long)]
        length: Option<Time>,
    },
    /// Remove a pattern and its playlist clips. A project keeps at least one pattern.
    Remove { id: u64 },
}

#[derive(Subcommand)]
pub enum NoteOp {
    /// Add one note. The pattern grows to whole bars when the note ends past it.
    Add {
        pattern: u64,
        channel: u64,
        #[arg(value_parser = parse::key)]
        key: u8,
        start: Time,
        length: Time,
        #[arg(long, value_parser = parse::unit, default_value = "0.8")]
        velocity: f32,
    },
    /// Replace a channel's notes with one sixteenth-note per `x` in STEPS, e.g. "x...x...x...x...".
    /// Any other character is a rest; spaces and `|` are ignored.
    Steps {
        pattern: u64,
        channel: u64,
        steps: String,
        #[arg(long, value_parser = parse::key, default_value = "C4")]
        key: u8,
        #[arg(long, value_parser = parse::unit, default_value = "0.8")]
        velocity: f32,
    },
    /// Remove a channel's notes that match every filter given; with none, all of them.
    Remove {
        pattern: u64,
        channel: u64,
        #[arg(long, value_parser = parse::key)]
        key: Option<u8>,
        #[arg(long)]
        start: Option<Time>,
    },
}

#[derive(Subcommand)]
pub enum ClipOp {
    /// Place a pattern, automation, or audio clip, whole by default. Missing tracks are added.
    Add {
        /// pattern:ID, automation:ID, or audio:CHANNEL.
        #[arg(value_parser = parse::clip_source)]
        source: ClipSource,
        track: usize,
        start: Time,
        #[arg(long)]
        length: Option<Time>,
        /// Skip this much of the source, for a clip trimmed at the start.
        #[arg(long)]
        offset: Option<Time>,
    },
    Set {
        id: u64,
        #[arg(long)]
        track: Option<usize>,
        #[arg(long)]
        start: Option<Time>,
        #[arg(long)]
        length: Option<Time>,
        #[arg(long)]
        offset: Option<Time>,
    },
    Remove { id: u64 },
}

#[derive(Subcommand)]
pub enum TrackOp {
    /// Add a track at the bottom and print its index.
    Add {
        #[arg(long)]
        name: Option<String>,
    },
    Set {
        index: usize,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        mute: Option<bool>,
    },
    /// Remove a track and its clips; later tracks move up.
    Remove { index: usize },
}

#[derive(Subcommand)]
pub enum InsertOp {
    /// Add an insert that sends to the master.
    Add {
        #[arg(long)]
        name: Option<String>,
    },
    Set {
        id: u64,
        #[arg(long)]
        name: Option<String>,
        /// Linear gain, 0 to 2.
        #[arg(long, value_parser = |t: &str| parse::bounded(t, 0.0, 2.0))]
        volume: Option<f32>,
        #[arg(long, value_parser = parse::pan, allow_hyphen_values = true)]
        pan: Option<f32>,
        #[arg(long)]
        mute: Option<bool>,
        #[arg(long)]
        solo: Option<bool>,
        /// Insert this one sends to. Routes that would loop are refused.
        #[arg(long)]
        output: Option<u64>,
    },
    /// Remove an insert with its effects and automation. Its channels go to the master.
    Remove { id: u64 },
}

#[derive(Subcommand)]
pub enum EffectOp {
    /// Append an effect plugin to an insert's chain and print its instance id.
    Add {
        insert: u64,
        /// Effect plugin, by id or name from `daw plugins`.
        plugin: String,
    },
    /// Remove an effect instance with its automation.
    Remove { instance: u64 },
}

#[derive(Subcommand)]
pub enum AutomationOp {
    /// Add a flat automation clip. It plays once placed with `clip add automation:ID`.
    Add {
        /// tempo, channel-volume:ID, channel-pan:ID, cutoff:ID, insert-volume:ID, insert-pan:ID, or plugin:INSTANCE:PARAM.
        #[arg(value_parser = parse::target)]
        target: Target,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value = "4bar")]
        length: Time,
        /// Starting value, 0 to 1. Defaults to the target's current value, or 0.5 for plugins.
        #[arg(long, value_parser = parse::unit)]
        value: Option<f32>,
    },
    Set {
        id: u64,
        #[arg(long)]
        name: Option<String>,
        /// The envelope repeats past its length. Whole-clip playlist clips follow it.
        #[arg(long)]
        length: Option<Time>,
    },
    /// Remove an automation clip and its playlist clips.
    Remove { id: u64 },
}

#[derive(Subcommand)]
pub enum PointOp {
    /// Add a point and print its index. The clip grows to whole bars when the point is past its end.
    Add {
        automation: u64,
        time: Time,
        #[arg(value_parser = parse::unit)]
        value: f32,
        /// Shape of the segment after this point: linear, curve, s-curve, hold, stairs:N, pulse:N.
        #[arg(long, value_parser = parse::shape, default_value = "linear")]
        shape: Shape,
        /// -1 to 1, for curve and s-curve.
        #[arg(long, value_parser = parse::pan, allow_hyphen_values = true, default_value = "0")]
        tension: f32,
    },
    /// Change a point and print its index after re-sorting by time.
    Set {
        automation: u64,
        index: usize,
        #[arg(long)]
        time: Option<Time>,
        #[arg(long, value_parser = parse::unit)]
        value: Option<f32>,
        #[arg(long, value_parser = parse::shape)]
        shape: Option<Shape>,
        #[arg(long, value_parser = parse::pan, allow_hyphen_values = true)]
        tension: Option<f32>,
    },
    Remove { automation: u64, index: usize },
}

/// Apply one change. Returns the text to print: the new id for commands that
/// create something. `catalog` is read only for commands that add plugins.
pub fn apply(project: &mut Project, op: Op, catalog: impl FnOnce() -> Catalog) -> Result<String, String> {
    match op {
        Op::Set { name, bpm, signature, grid, loop_range, no_loop } => {
            if let Some(bpm) = bpm {
                project.bpm = Project::check_bpm(bpm)?;
            }
            if let Some(name) = name {
                project.name = name;
            }
            if let Some(signature) = signature {
                project.signature = signature;
            }
            if let Some(grid) = grid {
                project.grid = grid;
            }
            if let Some((start, end)) = loop_range {
                let (start, end) = (ticks(project, start), ticks(project, end));
                if end <= start {
                    return Err("the loop must end after it starts".into());
                }
                project.playlist.loop_range = Some((start, end));
            }
            if no_loop {
                project.playlist.loop_range = None;
            }
            Ok(String::new())
        }
        Op::Channel(op) => channel(project, op, catalog),
        Op::Pattern(op) => pattern(project, op),
        Op::Note(op) => note(project, op),
        Op::Clip(op) => clip(project, op),
        Op::Track(op) => track(project, op),
        Op::Insert(op) => insert(project, op),
        Op::Effect(op) => effect(project, op, catalog),
        Op::Automation(op) => automation(project, op),
        Op::Point(op) => point(project, op),
    }
}

fn channel(project: &mut Project, op: ChannelOp, catalog: impl FnOnce() -> Catalog) -> Result<String, String> {
    match op {
        ChannelOp::Add { name, synth, sampler, audio, plugin, root } => {
            let source = match (synth, sampler, audio, plugin) {
                (_, Some(path), _, _) => Source::Sampler { path, root_key: root },
                (_, _, Some(path), _) => {
                    // The app may run from another folder, so keep the absolute path.
                    let path = std::fs::canonicalize(&path).map_err(|e| format!("{path}: {e}"))?;
                    let path = path.to_string_lossy().into_owned();
                    file_length(&path, project.bpm)?;
                    Source::Audio { path }
                }
                (_, _, _, Some(query)) => {
                    let plugin = find_plugin(&catalog(), &query, PluginKind::Instrument)?;
                    Source::Plugin(project.add_plugin(plugin))
                }
                (waveform, None, None, None) => {
                    Source::Synth(SynthParams { waveform: waveform.unwrap_or(Waveform::Saw), ..SynthParams::default() })
                }
            };
            Ok(project.add_channel(&name, source).0.to_string())
        }
        ChannelOp::Set { id, name, volume, pan, mute, insert, waveform, attack, release, cutoff, sample, root } => {
            if let Some(insert) = insert {
                find_insert(project, insert)?;
            }
            let channel = find_channel(project, id)?;
            if let Some(name) = name {
                channel.name = name;
            }
            if let Some(volume) = volume {
                channel.volume = volume;
            }
            if let Some(pan) = pan {
                channel.pan = pan;
            }
            if let Some(mute) = mute {
                channel.mute = mute;
            }
            if let Some(insert) = insert {
                channel.insert = InsertId(insert);
            }
            let synth_edit = waveform.is_some() || attack.is_some() || release.is_some() || cutoff.is_some();
            let sampler_edit = sample.is_some() || root.is_some();
            match &mut channel.source {
                Source::Synth(params) => {
                    if sampler_edit {
                        return Err(format!("channel {id} is a synth; --sample and --root are for samplers"));
                    }
                    params.waveform = waveform.unwrap_or(params.waveform);
                    params.attack = attack.unwrap_or(params.attack);
                    params.release = release.unwrap_or(params.release);
                    params.cutoff = cutoff.unwrap_or(params.cutoff);
                }
                Source::Sampler { path, root_key } => {
                    if synth_edit {
                        return Err(format!("channel {id} is a sampler; synth options do not apply"));
                    }
                    *path = sample.unwrap_or(std::mem::take(path));
                    *root_key = root.unwrap_or(*root_key);
                }
                Source::Audio { path } => {
                    if synth_edit || root.is_some() {
                        return Err(format!("channel {id} is audio; only --sample applies"));
                    }
                    *path = sample.unwrap_or(std::mem::take(path));
                }
                Source::Plugin(_) if synth_edit || sampler_edit => {
                    return Err(format!("channel {id} is a plugin; its settings live in the plugin"));
                }
                Source::Plugin(_) => {}
            }
            Ok(String::new())
        }
        ChannelOp::Remove { id } => {
            find_channel(project, id)?;
            project.remove_channel(ChannelId(id));
            Ok(String::new())
        }
    }
}

fn pattern(project: &mut Project, op: PatternOp) -> Result<String, String> {
    match op {
        PatternOp::Add { name, length } => {
            let id = project.add_pattern();
            set_pattern(project, id, name, length)?;
            Ok(id.0.to_string())
        }
        PatternOp::Set { id, name, length } => {
            find_pattern(project, id)?;
            set_pattern(project, PatternId(id), name, length)?;
            Ok(String::new())
        }
        PatternOp::Remove { id } => {
            find_pattern(project, id)?;
            if project.patterns.len() == 1 {
                return Err("a project keeps at least one pattern".into());
            }
            project.remove_pattern(PatternId(id));
            Ok(String::new())
        }
    }
}

fn set_pattern(project: &mut Project, id: PatternId, name: Option<String>, length: Option<Time>) -> Result<(), String> {
    if let Some(length) = length {
        project.set_pattern_length(id, nonzero(ticks(project, length))?);
    }
    if let Some(name) = name {
        find_pattern(project, id.0)?.name = name;
    }
    Ok(())
}

fn note(project: &mut Project, op: NoteOp) -> Result<String, String> {
    match op {
        NoteOp::Add { pattern, channel, key, start, length, velocity } => {
            find_channel(project, channel)?;
            let note = Note { start: ticks(project, start), length: nonzero(ticks(project, length))?, key, velocity };
            let notes = find_pattern(project, pattern)?.notes_mut(ChannelId(channel));
            let index = notes.partition_point(|n| n.start <= note.start);
            notes.insert(index, note);
            grow_pattern(project, pattern, note.end());
            Ok(String::new())
        }
        NoteOp::Steps { pattern, channel, steps, key, velocity } => {
            find_channel(project, channel)?;
            let steps: Vec<char> = steps.chars().filter(|c| !c.is_whitespace() && *c != '|').collect();
            let notes: Vec<Note> = (0..)
                .zip(&steps)
                .filter(|(_, c)| matches!(c, 'x' | 'X'))
                .map(|(step, _)| Note { start: step * STEP_TICKS, length: STEP_TICKS, key, velocity })
                .collect();
            *find_pattern(project, pattern)?.notes_mut(ChannelId(channel)) = notes;
            grow_pattern(project, pattern, steps.len() as Ticks * STEP_TICKS);
            Ok(String::new())
        }
        NoteOp::Remove { pattern, channel, key, start } => {
            find_channel(project, channel)?;
            let start = start.map(|s| ticks(project, s));
            let notes = find_pattern(project, pattern)?.notes_mut(ChannelId(channel));
            let before = notes.len();
            notes.retain(|n| !(key.is_none_or(|k| n.key == k) && start.is_none_or(|s| n.start == s)));
            let removed = before - notes.len();
            Ok(format!("removed {removed} note{}", if removed == 1 { "" } else { "s" }))
        }
    }
}

/// Grow a pattern to whole bars reaching `end`, as the piano roll does.
fn grow_pattern(project: &mut Project, id: u64, end: Ticks) {
    let length = project.pattern(PatternId(id)).map_or(0, |p| p.length);
    if end > length {
        project.set_pattern_length(PatternId(id), project.bars_to(end));
    }
}

fn clip(project: &mut Project, op: ClipOp) -> Result<String, String> {
    match op {
        ClipOp::Add { source, track, start, length, offset } => {
            let source_length = match source {
                ClipSource::Pattern(id) => find_pattern(project, id.0)?.length,
                ClipSource::Automation(id) => find_automation(project, id.0)?.length,
                ClipSource::Audio(id) => match find_channel(project, id.0)?.source.clone() {
                    Source::Audio { path } => file_length(&path, project.bpm)?,
                    _ => return Err(format!("channel {} is not an audio channel", id.0)),
                },
            };
            while project.playlist.tracks.len() <= track {
                let number = project.playlist.tracks.len() + 1;
                project.playlist.tracks.push(Track { name: format!("track {number}"), mute: false });
            }
            let offset = offset.map_or(0, |o| ticks(project, o));
            let length = match length {
                Some(length) => nonzero(ticks(project, length))?,
                None => nonzero(source_length.saturating_sub(offset))?,
            };
            let id = project.add_clip(track, ticks(project, start), source);
            let clip = find_clip(project, id.0)?;
            clip.length = length;
            clip.offset = offset;
            Ok(id.0.to_string())
        }
        ClipOp::Set { id, track, start, length, offset } => {
            if track.is_some_and(|t| t >= project.playlist.tracks.len()) {
                return Err(format!("no track {}; add it with `track add`", track.unwrap_or_default()));
            }
            let start = start.map(|t| ticks(project, t));
            let length = length.map(|t| nonzero(ticks(project, t))).transpose()?;
            let offset = offset.map(|t| ticks(project, t));
            let clip = find_clip(project, id)?;
            clip.track = track.unwrap_or(clip.track);
            clip.start = start.unwrap_or(clip.start);
            clip.length = length.unwrap_or(clip.length);
            clip.offset = offset.unwrap_or(clip.offset);
            Ok(String::new())
        }
        ClipOp::Remove { id } => {
            find_clip(project, id)?;
            project.playlist.clips.retain(|c| c.id != ClipId(id));
            Ok(String::new())
        }
    }
}

fn track(project: &mut Project, op: TrackOp) -> Result<String, String> {
    let tracks = &mut project.playlist.tracks;
    match op {
        TrackOp::Add { name } => {
            let name = name.unwrap_or_else(|| format!("track {}", tracks.len() + 1));
            tracks.push(Track { name, mute: false });
            Ok((tracks.len() - 1).to_string())
        }
        TrackOp::Set { index, name, mute } => {
            let count = tracks.len();
            let track = tracks.get_mut(index).ok_or(format!("no track {index}; there are {count}"))?;
            if let Some(name) = name {
                track.name = name;
            }
            if let Some(mute) = mute {
                track.mute = mute;
            }
            Ok(String::new())
        }
        TrackOp::Remove { index } => {
            if index >= tracks.len() {
                return Err(format!("no track {index}; there are {}", tracks.len()));
            }
            project.remove_track(index);
            Ok(String::new())
        }
    }
}

fn insert(project: &mut Project, op: InsertOp) -> Result<String, String> {
    match op {
        InsertOp::Add { name } => {
            let id = InsertId(project.next_id());
            let name = name.unwrap_or_else(|| format!("insert {}", project.mixer.inserts.len()));
            project.mixer.inserts.push(Insert::new(id, &name));
            Ok(id.0.to_string())
        }
        InsertOp::Set { id, name, volume, pan, mute, solo, output } => {
            if let Some(output) = output {
                find_insert(project, output)?;
                if !project.mixer.set_output(InsertId(id), InsertId(output)) {
                    return Err(format!("insert {id} cannot send to insert {output}: it is the master, itself, or would loop"));
                }
            }
            let insert = find_insert(project, id)?;
            if let Some(name) = name {
                insert.name = name;
            }
            if let Some(volume) = volume {
                insert.volume = volume;
            }
            if let Some(pan) = pan {
                insert.pan = pan;
            }
            if let Some(mute) = mute {
                insert.mute = mute;
            }
            if let Some(solo) = solo {
                insert.solo = solo;
            }
            Ok(String::new())
        }
        InsertOp::Remove { id } => {
            find_insert(project, id)?;
            if InsertId(id) == MASTER {
                return Err("the master insert cannot be removed".into());
            }
            project.remove_insert(InsertId(id));
            Ok(String::new())
        }
    }
}

fn effect(project: &mut Project, op: EffectOp, catalog: impl FnOnce() -> Catalog) -> Result<String, String> {
    match op {
        EffectOp::Add { insert, plugin } => {
            find_insert(project, insert)?;
            let plugin = find_plugin(&catalog(), &plugin, PluginKind::Effect)?;
            let instance = project.add_plugin(plugin);
            find_insert(project, insert)?.effects.push(instance);
            Ok(instance.0.to_string())
        }
        EffectOp::Remove { instance } => {
            let instance = InstanceId(instance);
            if !project.mixer.inserts.iter().any(|i| i.effects.contains(&instance)) {
                return Err(format!("no effect instance {}", instance.0));
            }
            project.remove_plugin(instance);
            Ok(String::new())
        }
    }
}

fn automation(project: &mut Project, op: AutomationOp) -> Result<String, String> {
    match op {
        AutomationOp::Add { target, name, length, value } => {
            let current = match target {
                Target::Plugin { instance, .. } => {
                    project.plugin(instance).ok_or(format!("no plugin instance {}", instance.0))?;
                    None
                }
                _ => Some(project.target_value(target).ok_or(format!("target {} does not exist", parse::target_name(target)))?),
            };
            if let Some(existing) = project.automation_for(target) {
                return Err(format!("target {} already has automation clip {}", parse::target_name(target), existing.0));
            }
            let name = name.unwrap_or_else(|| default_name(project, target));
            let length = nonzero(ticks(project, length))?;
            let id = project.add_automation(&name, target, value.or(current).unwrap_or(0.5));
            let clip = find_automation(project, id.0)?;
            clip.length = length;
            clip.envelope.points[1].time = length;
            Ok(id.0.to_string())
        }
        AutomationOp::Set { id, name, length } => {
            if let Some(length) = length {
                find_automation(project, id)?;
                project.set_automation_length(AutomationId(id), nonzero(ticks(project, length))?);
            }
            if let Some(name) = name {
                find_automation(project, id)?.name = name;
            }
            Ok(String::new())
        }
        AutomationOp::Remove { id } => {
            find_automation(project, id)?;
            project.remove_automation(AutomationId(id));
            Ok(String::new())
        }
    }
}

/// A name like the app's: the channel or insert name and the parameter.
fn default_name(project: &Project, target: Target) -> String {
    let channel = |id| project.channel(id).map_or(String::new(), |c| c.name.clone());
    let insert = |id| project.mixer.insert(id).map_or(String::new(), |i| i.name.clone());
    match target {
        Target::Tempo => "tempo".into(),
        Target::ChannelVolume(id) => format!("{} volume", channel(id)),
        Target::ChannelPan(id) => format!("{} pan", channel(id)),
        Target::SynthCutoff(id) => format!("{} cutoff", channel(id)),
        Target::InsertVolume(id) => format!("{} volume", insert(id)),
        Target::InsertPan(id) => format!("{} pan", insert(id)),
        Target::Plugin { instance, param } => {
            let plugin = project.plugin(instance).map_or(String::new(), |p| p.plugin.name.clone());
            format!("{plugin}: param {param}")
        }
    }
}

fn point(project: &mut Project, op: PointOp) -> Result<String, String> {
    match op {
        PointOp::Add { automation, time, value, shape, tension } => {
            let time = ticks(project, time);
            let index = find_automation(project, automation)?.envelope.insert(Point { time, value, shape, tension });
            grow_automation(project, automation, time);
            Ok(index.to_string())
        }
        PointOp::Set { automation, index, time, value, shape, tension } => {
            let time = time.map(|t| ticks(project, t));
            let envelope = &mut find_automation(project, automation)?.envelope;
            let count = envelope.points.len();
            let point = envelope.points.get_mut(index).ok_or(format!("no point {index}; there are {count}"))?;
            point.time = time.unwrap_or(point.time);
            point.value = value.unwrap_or(point.value);
            point.shape = shape.unwrap_or(point.shape);
            point.tension = tension.unwrap_or(point.tension);
            let moved = envelope.points.remove(index);
            let index = envelope.insert(moved);
            grow_automation(project, automation, moved.time);
            Ok(index.to_string())
        }
        PointOp::Remove { automation, index } => {
            let points = &mut find_automation(project, automation)?.envelope.points;
            if index >= points.len() {
                return Err(format!("no point {index}; there are {}", points.len()));
            }
            points.remove(index);
            Ok(String::new())
        }
    }
}

/// Grow an automation clip to whole bars reaching `time`, as the editor does.
fn grow_automation(project: &mut Project, id: u64, time: Ticks) {
    let length = project.automation_clip(AutomationId(id)).map_or(0, |a| a.length);
    if time > length {
        project.set_automation_length(AutomationId(id), project.bars_to(time));
    }
}

fn nonzero(ticks: Ticks) -> Result<Ticks, String> {
    if ticks == 0 { Err("length must be greater than zero".into()) } else { Ok(ticks) }
}

fn find_channel(project: &mut Project, id: u64) -> Result<&mut Channel, String> {
    project.channel_mut(ChannelId(id)).ok_or(format!("no channel {id}"))
}

fn find_pattern(project: &mut Project, id: u64) -> Result<&mut Pattern, String> {
    project.pattern_mut(PatternId(id)).ok_or(format!("no pattern {id}"))
}

fn find_automation(project: &mut Project, id: u64) -> Result<&mut AutomationClip, String> {
    project.automation_clip_mut(AutomationId(id)).ok_or(format!("no automation clip {id}"))
}

fn find_insert(project: &mut Project, id: u64) -> Result<&mut Insert, String> {
    project.mixer.insert_mut(InsertId(id)).ok_or(format!("no insert {id}"))
}

fn find_clip(project: &mut Project, id: u64) -> Result<&mut Clip, String> {
    project.playlist.clips.iter_mut().find(|c| c.id == ClipId(id)).ok_or(format!("no playlist clip {id}"))
}
