//! `daw show` and `daw plugins`: text for reading projects and the plugin catalog.
//! Values print in the forms the edit commands accept.

use std::fmt::Write;

use daw_model::{AutomationId, ClipSource, PatternId, Project, Source, Target};
use daw_plugins::PluginKind;
use daw_plugins::scan::Catalog;

use super::parse::{clip_source_name, key_name, shape_name, target_name, time, waveform_name};
use crate::app::pan_text;

pub fn show(project: &Project, pattern: Option<u64>, automation: Option<u64>, json: bool) -> Result<String, String> {
    if let Some(id) = pattern {
        let pattern = project.pattern(PatternId(id)).ok_or(format!("no pattern {id}"))?;
        return if json { to_json(pattern) } else { Ok(show_pattern(project, pattern)) };
    }
    if let Some(id) = automation {
        let clip = project.automation_clip(AutomationId(id)).ok_or(format!("no automation clip {id}"))?;
        return if json { to_json(clip) } else { Ok(show_automation(project, clip)) };
    }
    if json { to_json(project) } else { Ok(overview(project)) }
}

fn to_json(value: &impl serde::Serialize) -> Result<String, String> {
    serde_json::to_string_pretty(value).map_err(|e| e.to_string())
}

fn overview(project: &Project) -> String {
    let sig = project.signature;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "project {:?}: {} bpm, {}/{}, grid {}, song length {}",
        project.name,
        project.bpm,
        sig.numerator,
        sig.denominator,
        project.grid,
        time(project.song_length(), sig)
    );
    if let Some((start, end)) = project.playlist.loop_range {
        let _ = writeln!(out, "loop {}..{}", time(start, sig), time(end, sig));
    }

    let _ = writeln!(out, "\nchannels (id, name, source, mixer insert)");
    for channel in &project.channels {
        let source = match &channel.source {
            Source::Synth(p) => format!(
                "synth {} attack {} release {} cutoff {}",
                waveform_name(p.waveform),
                p.attack,
                p.release,
                p.cutoff
            ),
            Source::Sampler { path, root_key } => format!("sampler {path:?} root {}", key_name(*root_key)),
            Source::Audio { path } => format!("audio {path:?}"),
            Source::Plugin(instance) => match project.plugin(*instance) {
                Some(p) => format!("plugin {} {:?} ({} {})", instance.0, p.plugin.name, p.plugin.format, p.plugin.id),
                None => format!("missing plugin {}", instance.0),
            },
        };
        let mute = if channel.mute { " muted" } else { "" };
        let _ = writeln!(
            out,
            "  {} {:?}  {source}  volume {} pan {}{mute}  -> insert {}",
            channel.id.0,
            channel.name,
            channel.volume,
            pan_text(channel.pan),
            channel.insert.0
        );
    }

    let _ = writeln!(out, "\npatterns (id, name, length, notes per channel)");
    for pattern in &project.patterns {
        let lanes: Vec<String> = pattern
            .lanes
            .iter()
            .filter(|l| !l.notes.is_empty())
            .map(|l| format!("channel {}: {}", l.channel.0, l.notes.len()))
            .collect();
        let lanes = if lanes.is_empty() { "empty".into() } else { lanes.join(", ") };
        let _ = writeln!(out, "  {} {:?}  length {}  {lanes}", pattern.id.0, pattern.name, time(pattern.length, sig));
    }

    let _ = writeln!(out, "\nautomation (id, name, target, length, points)");
    if project.automation.is_empty() {
        let _ = writeln!(out, "  none");
    }
    for clip in &project.automation {
        let _ = writeln!(
            out,
            "  {} {:?}  {}  length {}  {} points",
            clip.id.0,
            clip.name,
            target_name(clip.target),
            time(clip.length, sig),
            clip.envelope.points.len()
        );
    }

    let _ = writeln!(out, "\nplaylist (track index, name; clips as id source start length)");
    for (index, track) in project.playlist.tracks.iter().enumerate() {
        let mut clips: Vec<_> = project.playlist.clips.iter().filter(|c| c.track == index).collect();
        clips.sort_by_key(|c| c.start);
        let mute = if track.mute { " muted" } else { "" };
        if clips.is_empty() {
            // Unused default tracks would bury the ones with clips.
            if track.name != format!("track {}", index + 1) || track.mute {
                let _ = writeln!(out, "  {index} {:?}{mute}: empty", track.name);
            }
            continue;
        }
        let _ = writeln!(out, "  {index} {:?}{mute}:", track.name);
        for clip in clips {
            let offset = if clip.offset > 0 { format!(" offset {}", time(clip.offset, sig)) } else { String::new() };
            let _ = writeln!(
                out,
                "    {} {} start {} length {}{offset}",
                clip.id.0,
                clip_source_name(clip.source),
                time(clip.start, sig),
                time(clip.length, sig)
            );
        }
    }
    let _ = writeln!(out, "  ({} tracks)", project.playlist.tracks.len());

    let _ = writeln!(out, "\nmixer (id, name, volume 0..2, pan, output, effect instances)");
    for insert in &project.mixer.inserts {
        let mut flags = String::new();
        if insert.mute {
            flags.push_str(" muted");
        }
        if insert.solo {
            flags.push_str(" solo");
        }
        let output = match project.mixer.output(insert.id) {
            Some(output) => format!("  -> insert {}", output.0),
            None => String::new(),
        };
        let effects: Vec<String> = insert
            .effects
            .iter()
            .map(|e| match project.plugin(*e) {
                Some(p) => format!("{} {:?}", e.0, p.plugin.name),
                None => format!("{} missing", e.0),
            })
            .collect();
        let effects = if effects.is_empty() { String::new() } else { format!("  effects: {}", effects.join(", ")) };
        let _ = writeln!(
            out,
            "  {} {:?}  volume {} pan {}{flags}{output}{effects}",
            insert.id.0,
            insert.name,
            insert.volume,
            pan_text(insert.pan)
        );
    }
    out.trim_end().to_string()
}

fn show_pattern(project: &Project, pattern: &daw_model::Pattern) -> String {
    let sig = project.signature;
    let mut out = format!("pattern {} {:?}  length {}", pattern.id.0, pattern.name, time(pattern.length, sig));
    for lane in pattern.lanes.iter().filter(|l| !l.notes.is_empty()) {
        let name = project.channel(lane.channel).map_or("missing", |c| c.name.as_str());
        let _ = write!(out, "\nchannel {} {name:?} (key start length velocity)", lane.channel.0);
        for note in &lane.notes {
            let _ = write!(
                out,
                "\n  {} {} {} {}",
                key_name(note.key),
                time(note.start, sig),
                time(note.length, sig),
                note.velocity
            );
        }
    }
    out
}

fn show_automation(project: &Project, clip: &daw_model::AutomationClip) -> String {
    let sig = project.signature;
    let mut out = format!(
        "automation {} {:?}  {}  length {}",
        clip.id.0,
        clip.name,
        target_name(clip.target),
        time(clip.length, sig)
    );
    if let Target::Tempo = clip.target {
        let _ = write!(out, "\nvalues map to {}..{} bpm", Project::BPM.start(), Project::BPM.end());
    }
    let placed: Vec<String> = project
        .playlist
        .clips
        .iter()
        .filter(|c| c.source == ClipSource::Automation(clip.id))
        .map(|c| format!("track {} at {}", c.track, time(c.start, sig)))
        .collect();
    let placed = if placed.is_empty() { "not placed in the playlist".into() } else { format!("placed on {}", placed.join(", ")) };
    let _ = write!(out, "\n{placed}\npoints (index time value shape tension)");
    for (index, point) in clip.envelope.points.iter().enumerate() {
        let tension = if point.shape.uses_tension() { format!(" {}", point.tension) } else { String::new() };
        let _ = write!(out, "\n  {index} {} {} {}{tension}", time(point.time, sig), point.value, shape_name(point.shape));
    }
    out
}

pub fn plugins(catalog: &Catalog, json: bool) -> Result<String, String> {
    if json {
        return to_json(&catalog.plugins);
    }
    if catalog.plugins.is_empty() && catalog.failed.is_empty() {
        return Ok("no plugins cached; run `daw plugins --scan`".into());
    }
    let mut out = String::from("kind, format, name, vendor, id");
    for info in &catalog.plugins {
        let kind = match info.kind {
            PluginKind::Instrument => "instrument",
            PluginKind::Effect => "effect",
        };
        let plugin = &info.plugin;
        let _ = write!(out, "\n{kind:<10}  {:<4}  {:?}  {:?}  {}", plugin.format.to_string(), plugin.name, plugin.vendor, plugin.id);
    }
    for failure in &catalog.failed {
        let _ = write!(out, "\nfailed      {:<4}  {:?}: {}", failure.format.to_string(), failure.name, failure.error);
    }
    Ok(out)
}
