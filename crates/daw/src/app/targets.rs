//! Automation targets: how they read, and binding one to a new automation
//! clip.

use daw_model::automation::tempo_from_normalized;
use daw_model::layout::Panel;
use daw_model::time::Ticks;
use daw_model::{AutomationId, ClipSource, SynthParams, Target};

use super::{App, LAST_TOUCHED};
use crate::panels::automation;
use crate::units::{gain_text, pan_text};

impl App {
    pub fn target_name(&self, target: Target) -> String {
        let insert_name = |id| self.project.mixer.insert(id).map(|i| i.name.clone()).unwrap_or_default();
        let channel_name = |id| self.project.channel(id).map(|c| c.name.clone()).unwrap_or_default();
        match target {
            Target::Plugin { instance, param } => {
                let plugin = self.project.plugin(instance).map(|p| p.plugin.name.clone()).unwrap_or_default();
                format!("{plugin}: {}", self.session.param_name(instance, param))
            }
            Target::InsertVolume(id) => format!("{} volume", insert_name(id)),
            Target::InsertPan(id) => format!("{} pan", insert_name(id)),
            Target::ChannelVolume(id) => format!("{} volume", channel_name(id)),
            Target::ChannelPan(id) => format!("{} pan", channel_name(id)),
            Target::SynthCutoff(id) => format!("{} cutoff", channel_name(id)),
            Target::Tempo => "tempo".into(),
        }
    }

    /// Current normalized value of a target, for new automation clips.
    pub fn target_value(&self, target: Target) -> f32 {
        match target {
            Target::Plugin { instance, param } => self.session.param_value(instance, param),
            _ => self.project.target_value(target).unwrap_or(0.5),
        }
    }

    /// Text for a target value in the parameter's own units.
    pub fn value_text(&self, target: Target, value: f32) -> String {
        match target {
            Target::Plugin { instance, param } => self.session.param_text(instance, param, value),
            Target::InsertVolume(_) => gain_text(value * 2.0),
            Target::ChannelVolume(_) => gain_text(value),
            Target::InsertPan(_) | Target::ChannelPan(_) => pan_text(value * 2.0 - 1.0),
            Target::SynthCutoff(_) => format!("{:.0} Hz", SynthParams::cutoff_hz(value)),
            Target::Tempo => format!("{:.1} bpm", tempo_from_normalized(value)),
        }
    }

    pub fn target_steps(&self, target: Target) -> u32 {
        match target {
            Target::Plugin { instance, param } => self.session.param_steps(instance, param),
            _ => 0,
        }
    }

    /// Note that a parameter was touched: feeds the last-touched list, bind
    /// mode, and automation recording.
    pub fn touched(&mut self, target: Target, value: f32) {
        // Plugin parameters live in plugin state, which is saved with the project.
        self.dirty = true;
        if matches!(target, Target::Plugin { .. }) { self.revision += 1; }
        self.last_touched.retain(|(t, _)| *t != target);
        self.last_touched.push_front((target, value));
        self.last_touched.truncate(LAST_TOUCHED);
        if self.bind_mode && self.project.automation_for(target).is_none() {
            self.checkpoint_parameter(target);
            self.bind_at(target, value);
        }
        if self.record && self.playing {
            automation::record(self, target, value);
        }
    }

    /// Create an automation clip for `target` at the playhead (or loop range)
    /// on a free playlist track, and open it in the automation editor.
    pub fn bind(&mut self, target: Target) -> AutomationId {
        let value = self.target_value(target);
        self.bind_at(target, value)
    }

    /// Like `bind`, starting the new clip flat at `value`.
    pub fn bind_at(&mut self, target: Target, value: f32) -> AutomationId {
        if let Some(existing) = self.project.automation_for(target) {
            self.open_automation(existing);
            return existing;
        }
        let name = self.target_name(target);
        let id = self.project.add_automation(&name, target, value);
        let bar = self.project.signature.ticks_per_bar();
        let (start, length) = match self.project.playlist.loop_range {
            Some((start, end)) => (start, end - start),
            None => (self.project.grid.snap_floor(self.position as Ticks, self.project.signature) / bar * bar, bar * 4),
        };
        if let Some(clip) = self.project.automation_clip_mut(id) {
            clip.length = length;
            clip.envelope.points[1].time = length;
        }
        let track = self.project.free_track(start, start + length);
        self.project.add_clip(track, start, ClipSource::Automation(id));
        self.set_status(format!("bound {name}"));
        self.open_automation(id);
        self.edited();
        id
    }

    pub fn open_automation(&mut self, id: AutomationId) {
        self.automation.clip = Some(id);
        self.automation.values = automation::ValueRange::FULL;
        self.automation.selected.clear();
        self.show_panel(Panel::Automation);
    }
}
