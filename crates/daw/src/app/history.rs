//! Undo snapshots include plugin state and values still queued for audio processing.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use daw_model::{InstanceId, Project, Target};
use daw_plugins::Touch;

use super::{App, UNDO_LIMIT};
use crate::session::PluginParameters;

/// What an edit changed, from the project before it to the project after
/// it, such as "added channel synth 2" or "changed pattern chords and
/// playlist clips". `None` when the projects match, as after an edit that
/// only moved plugin parameters.
pub fn describe(older: &Project, newer: &Project) -> Option<String> {
    let mut parts = Vec::new();
    by_id(&mut parts, "channel", &older.channels, &newer.channels, |c| c.id, |c| c.name.clone());
    by_id(&mut parts, "pattern", &older.patterns, &newer.patterns, |p| p.id, |p| p.name.clone());
    if older.playlist.tracks != newer.playlist.tracks { parts.push("changed playlist tracks".to_string()); }
    clips(&mut parts, older, newer);
    by_id(&mut parts, "automation clip", &older.automation, &newer.automation, |a| a.id, |a| a.name.clone());
    let before = parts.len();
    by_id(&mut parts, "insert", &older.mixer.inserts, &newer.mixer.inserts, |i| i.id, |i| newer.insert_label(i.id));
    if parts.len() == before && older.mixer != newer.mixer { parts.push("changed mixer".to_string()); }
    // Plugin state is captured anew for each snapshot and can differ by a
    // few bytes without an edit, so it only explains an edit with no other change.
    if parts.is_empty() {
        by_id(&mut parts, "plugin", &older.plugins, &newer.plugins, |p| p.id, |p| p.plugin.name.clone());
    }
    if older.bpm != newer.bpm || older.signature != newer.signature { parts.push("changed tempo".to_string()); }
    if older.grid != newer.grid { parts.push("changed snap".to_string()); }
    if older.render != newer.render { parts.push("changed render settings".to_string()); }
    if older.name != newer.name { parts.push("renamed project".to_string()); }
    match parts.len() {
        0 => None,
        1 | 2 => Some(parts.join(" and ")),
        n => Some(format!("{} and {} more changes", parts[0], n - 1)),
    }
}

/// Playlist clip changes: one clip by name, several by count.
fn clips(parts: &mut Vec<String>, older: &Project, newer: &Project) {
    let (old, new) = (&older.playlist.clips, &newer.playlist.clips);
    let added: Vec<_> = new.iter().filter(|c| !old.iter().any(|o| o.id == c.id)).collect();
    let deleted: Vec<_> = old.iter().filter(|o| !new.iter().any(|c| c.id == o.id)).collect();
    let changed: Vec<_> = new.iter().filter(|c| old.iter().any(|o| o.id == c.id && o != *c)).collect();
    let name = |project: &Project, clip: &daw_model::Clip| -> String {
        match clip.source {
            daw_model::ClipSource::Pattern(id) => project.pattern(id).map(|p| p.name.clone()),
            daw_model::ClipSource::Automation(id) => project.automation_clip(id).map(|a| a.name.clone()),
            daw_model::ClipSource::Audio(id) => project.channel(id).map(|c| c.name.clone()),
        }
        .unwrap_or_default()
    };
    for (verb, list, project) in [("added", &added, newer), ("deleted", &deleted, older), ("changed", &changed, newer)] {
        match list.as_slice() {
            [] => {}
            [clip] => parts.push(format!("{verb} clip {}", name(project, clip))),
            many => parts.push(format!("{verb} {} clips", many.len())),
        }
    }
}

fn by_id<T: PartialEq, K: PartialEq>(
    parts: &mut Vec<String>,
    kind: &str,
    older: &[T],
    newer: &[T],
    id: impl Fn(&T) -> K,
    name: impl Fn(&T) -> String,
) {
    for item in newer {
        match older.iter().find(|o| id(o) == id(item)) {
            None => parts.push(format!("added {kind} {}", name(item))),
            Some(old) if old != item => parts.push(format!("changed {kind} {}", name(item))),
            Some(_) => {}
        }
    }
    for item in older.iter().filter(|o| !newer.iter().any(|n| id(n) == id(o))) {
        parts.push(format!("deleted {kind} {}", name(item)));
    }
}

pub struct Snapshot {
    pub project: Project,
    pub parameters: PluginParameters,
}

#[derive(Default)]
pub struct State {
    gestures: HashMap<InstanceId, Gesture>,
}

struct Gesture {
    held: HashSet<u32>,
    changed: bool,
    last: Instant,
}

impl Default for Gesture {
    fn default() -> Self { Self { held: HashSet::new(), changed: false, last: Instant::now() } }
}

impl App {
    pub(super) fn snapshot(&mut self, previous: Option<InstanceId>) -> Snapshot {
        let parameters = self.session.capture_plugins(&mut self.project, previous);
        Snapshot { project: self.project.clone(), parameters }
    }

    fn push_checkpoint(&mut self, snapshot: Snapshot) {
        self.undo.push(snapshot);
        if self.undo.len() > UNDO_LIMIT { self.undo.remove(0); }
        self.redo.clear();
        self.dirty = true;
    }

    pub fn checkpoint(&mut self) {
        self.finish_plugin_edits();
        let snapshot = self.snapshot(None);
        self.push_checkpoint(snapshot);
    }

    pub fn checkpoint_parameter(&mut self, target: Target) {
        let native_drag = matches!(target, Target::Plugin { instance, .. }
            if self.history.gestures.get(&instance).is_some_and(|g| g.changed));
        if !self.editing && !native_drag { self.checkpoint(); }
    }

    pub(super) fn finish_edit(&mut self) {
        if self.editing { self.session.store_states(&mut self.project); }
        self.editing = false;
    }

    pub(super) fn plugin_touch(&mut self, id: InstanceId, touch: Touch) {
        if self.project.plugin(id).is_none() { return; }
        match touch {
            Touch::Begin(param) => { self.history.gestures.entry(id).or_default().held.insert(param); }
            Touch::Value { param, value } => {
                if !(0.0..=1.0).contains(&value) || self.session.previous_parameter(id, param) == Some(value) { return; }
                let changed = self.history.gestures.get(&id).is_some_and(|g| g.changed);
                if !changed {
                    let snapshot = self.snapshot(Some(id));
                    self.push_checkpoint(snapshot);
                }
                let gesture = self.history.gestures.entry(id).or_default();
                gesture.changed = true;
                gesture.last = Instant::now();
                self.session.record_parameter(id, param, value);
                self.touched(Target::Plugin { instance: id, param }, value);
                if self.param_instance.is_none() { self.param_instance = Some(id); }
            }
            Touch::End(param) => {
                if let Some(gesture) = self.history.gestures.get_mut(&id) {
                    gesture.held.remove(&param);
                    if gesture.held.is_empty() {
                        self.history.gestures.remove(&id);
                        self.session.finish_plugin_gesture(&mut self.project, id);
                    }
                }
            }
        }
    }

    pub(super) fn finish_idle_plugin_edits(&mut self) {
        // Editors that only report values have no release notification.
        let idle: Vec<_> = self.history.gestures.iter().filter(|(_, g)| g.held.is_empty() && g.last.elapsed() >= Duration::from_millis(500))
            .map(|(id, _)| *id).collect();
        for id in idle {
            self.history.gestures.remove(&id);
            self.session.finish_plugin_gesture(&mut self.project, id);
        }
    }

    pub(super) fn finish_plugin_edits(&mut self) {
        for (id, _) in self.history.gestures.drain() { self.session.finish_plugin_gesture(&mut self.project, id); }
    }
}
