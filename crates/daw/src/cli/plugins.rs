//! `daw params` and `daw presets`: inspect and change plugin settings.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::Path;

use daw_model::{ChannelId, InstanceId, PluginRef, Project, Source};
use daw_plugins::{Controller, Loaded, ParamInfo};

use super::parse;

/// Plugins loaded for one command or batch, so each loads once. `store`
/// writes the changed ones back into the project.
#[derive(Default)]
pub struct Plugins {
    loaded: HashMap<InstanceId, Plugin>,
}

struct Plugin {
    plugin: PluginRef,
    loaded: Loaded,
    changed: bool,
}

impl Plugins {
    pub fn list(&mut self, project: &Project, id: u64, find: Option<&str>) -> Result<String, String> {
        let controller = &*self.get(project, id)?.loaded.controller;
        let lines: Vec<String> = controller
            .params()
            .iter()
            .filter(|p| find.is_none_or(|f| contains_ignore_case(&p.name, f)))
            .map(|p| param_line(controller, p))
            .collect();
        Ok(lines.join("\n"))
    }

    pub fn describe(&mut self, project: &Project, id: u64, query: &str) -> Result<String, String> {
        let controller = &*self.get(project, id)?.loaded.controller;
        let params = controller.params();
        let param = find_param(&params, query)?;
        // Discrete parameters show each step; continuous ones every 0.05.
        let count = if param.steps > 0 && param.steps <= 64 { param.steps } else { 20 };
        let mut lines = vec![format!("{}  {:?}", param.id, param.name)];
        for i in 0..=count {
            let value = i as f32 / count as f32;
            lines.push(format!("  {value:.4}  {}", controller.param_text(param.id, value)));
        }
        Ok(lines.join("\n"))
    }

    /// Set parameters. Values go through the processor as well as the
    /// controller, because VST3 saves the processor's copy.
    pub fn set(&mut self, project: &Project, id: u64, set: &[(String, f32)]) -> Result<String, String> {
        let entry = self.get(project, id)?;
        let params = entry.loaded.controller.params();
        let mut events = Vec::new();
        let mut changed = Vec::new();
        for (name, value) in set {
            let param = find_param(&params, name)?;
            events.push(daw_engine::Event { offset: 0, kind: daw_engine::EventKind::Param { id: param.id, value: *value } });
            entry.loaded.controller.set_param(param.id, *value);
            changed.push(param);
        }
        let transport = daw_engine::TransportInfo {
            sample_rate: 48_000.0,
            bpm: project.bpm,
            beats: 0.0,
            frames: 0,
            playing: false,
            numerator: project.signature.numerator,
            denominator: project.signature.denominator,
            bar_start: 0.0,
        };
        let (mut left, mut right) = (vec![0.0; 64], vec![0.0; 64]);
        entry.loaded.processor.process(&transport, &events, &mut left, &mut right);
        entry.changed = true;
        let lines: Vec<String> = changed.iter().map(|p| param_line(&*entry.loaded.controller, p)).collect();
        Ok(lines.join("\n"))
    }

    pub fn presets(&mut self, project: &Project, id: u64, find: Option<&str>) -> Result<String, String> {
        let entry = self.get(project, id)?;
        let names = entry.loaded.controller.presets();
        if names.is_empty() {
            return Err(format!("{} has no factory presets the host can load", entry.plugin.name));
        }
        let lines: Vec<String> = names
            .iter()
            .enumerate()
            .filter(|(_, name)| find.is_none_or(|f| contains_ignore_case(name, f)))
            .map(|(index, name)| format!("{index}  {name:?}"))
            .collect();
        Ok(lines.join("\n"))
    }

    pub fn load_preset(&mut self, project: &Project, id: u64, preset: &str) -> Result<String, String> {
        let entry = self.get(project, id)?;
        let names = entry.loaded.controller.presets();
        let index = match preset.parse::<usize>() {
            Ok(index) => index,
            Err(_) => {
                let matches: Vec<usize> = (0..names.len()).filter(|&i| names[i].eq_ignore_ascii_case(preset)).collect();
                match matches.as_slice() {
                    [index] => *index,
                    [] => return Err(format!("{} has no preset named {preset:?}; list them with `daw presets`", entry.plugin.name)),
                    _ => return Err(format!("several presets are named {preset:?}; use an index")),
                }
            }
        };
        entry.loaded.controller.load_preset(index).map_err(|e| e.to_string())?;
        entry.changed = true;
        Ok(format!("loaded preset {index} {:?}", names.get(index).map_or("", String::as_str)))
    }

    /// Replace a JUCE plugin's own state with a file, such as a Vital preset.
    pub fn load_file(&mut self, project: &Project, id: u64, path: &Path) -> Result<String, String> {
        let juce = std::fs::read(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
        let instance = instance_id(project, id)?;
        let entry = self.get(project, id)?;
        let current = save_state(entry)?;
        let state = daw_plugins::replace_juce_state(&entry.plugin, &current, &juce).map_err(|e| e.to_string())?;
        // Load the new state, so the plugin checks it and saves its own encoding.
        let loaded = daw_plugins::load(&entry.plugin, &state, 48_000.0, daw_engine::MAX_BLOCK)
            .map_err(|e| format!("{} rejected {}: {e}", entry.plugin.name, path.display()))?;
        let plugin = entry.plugin.clone();
        self.loaded.insert(instance, Plugin { plugin, loaded, changed: true });
        Ok(format!("loaded {}", path.display()))
    }

    /// Save the changed plugins' states into the project.
    pub fn store(&self, project: &mut Project) -> Result<(), String> {
        for (id, entry) in self.loaded.iter().filter(|(_, e)| e.changed) {
            let state = save_state(entry)?;
            if let Some(instance) = project.plugin_mut(*id) {
                instance.state = state;
            }
        }
        Ok(())
    }

    fn get(&mut self, project: &Project, id: u64) -> Result<&mut Plugin, String> {
        let instance = instance_id(project, id)?;
        match self.loaded.entry(instance) {
            Entry::Occupied(entry) => Ok(entry.into_mut()),
            Entry::Vacant(entry) => {
                let saved = project.plugin(instance).ok_or(format!("no plugin instance {}", instance.0))?;
                let loaded = daw_plugins::load(&saved.plugin, &saved.state, 48_000.0, daw_engine::MAX_BLOCK)
                    .map_err(|e| format!("could not load {}: {e}", saved.plugin.name))?;
                Ok(entry.insert(Plugin { plugin: saved.plugin.clone(), loaded, changed: false }))
            }
        }
    }
}

/// A plugin instance id, or the id of a channel whose source is a plugin.
fn instance_id(project: &Project, id: u64) -> Result<InstanceId, String> {
    if project.plugin(InstanceId(id)).is_some() {
        return Ok(InstanceId(id));
    }
    match project.channel(ChannelId(id)).map(|c| &c.source) {
        Some(Source::Plugin(instance)) => Ok(*instance),
        Some(_) => Err(format!("channel {id} is not a plugin channel")),
        None => Err(format!("no plugin instance or plugin channel {id}")),
    }
}

fn save_state(entry: &Plugin) -> Result<Vec<u8>, String> {
    entry.loaded.controller.save_state().map_err(|e| format!("could not save {} state: {e}", entry.plugin.name))
}

fn param_line(controller: &dyn Controller, p: &ParamInfo) -> String {
    let value = controller.param_value(p.id);
    let steps = if p.steps > 0 { format!("  steps {}", p.steps) } else { String::new() };
    let automatable = if p.automatable { "" } else { "  not automatable" };
    format!("{}  {:?}  value {value:.3} ({}){steps}{automatable}", p.id, p.name, controller.param_text(p.id, value))
}

pub fn find_param<'a>(params: &'a [ParamInfo], query: &str) -> Result<&'a ParamInfo, String> {
    if let Some(param) = query.parse::<u32>().ok().and_then(|id| params.iter().find(|p| p.id == id)) {
        return Ok(param);
    }
    let matches: Vec<_> = params.iter().filter(|p| p.name.eq_ignore_ascii_case(query)).collect();
    match matches.as_slice() {
        [param] => Ok(param),
        [] => Err(format!("no parameter {query:?}; list them with `daw params FILE INSTANCE`")),
        _ => Err(format!("several parameters are named {query:?}; use an id")),
    }
}

/// PARAM=VALUE, with VALUE normalized 0..1.
pub fn parse_assignment(text: &str) -> Result<(String, f32), String> {
    let (name, value) = text.rsplit_once('=').ok_or(format!("expected PARAM=VALUE, got {text:?}"))?;
    Ok((name.to_string(), parse::unit(value)?))
}

fn contains_ignore_case(text: &str, part: &str) -> bool {
    text.to_lowercase().contains(&part.to_lowercase())
}
