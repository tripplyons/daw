//! Undo snapshots include plugin state and values still queued for audio processing.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use daw_model::{InstanceId, Project, Target};
use daw_plugins::Touch;

use super::{App, UNDO_LIMIT};
use crate::session::PluginParameters;

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
                if !value.is_finite() || !(0.0..=1.0).contains(&value) || self.session.previous_parameter(id, param) == Some(value) { return; }
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
