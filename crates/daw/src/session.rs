//! Keeps the realtime engine and plugin instances in step with the project.

use std::collections::HashMap;
#[cfg(test)]
use std::path::Path;
use std::sync::{Arc, Mutex};

use daw_engine::input::{Recorder, Take};
use daw_engine::output::{self, BitDepth};
#[cfg(test)]
use daw_engine::output::Stem;
use daw_engine::audio;
use daw_engine::song::{EngineTarget, PlayMode, channel_node, compile, engine_target};
use daw_engine::synth::{PARAM_ATTACK, PARAM_CUTOFF, PARAM_RELEASE, PARAM_WAVEFORM, Sample, Sampler, Synth};
use daw_engine::{Command, Engine, EngineHandle, MAX_BLOCK, Node};
use daw_model::time::{Ticks, ticks_to_seconds};
use daw_model::{ChannelId, InsertId, InstanceId, Project, RenderSettings, Source, SynthParams, Target, Waveform};
use daw_plugins::{Controller, ParamInfo, Touch};

pub type PluginParameters = HashMap<InstanceId, Vec<(u32, f32)>>;

#[derive(Clone)]
struct PluginState {
    state: Vec<u8>,
    parameters: Vec<(u32, f32)>,
}

#[derive(Debug, Clone, Copy)]
pub struct RenderOptions {
    pub depth: BitDepth,
    pub range: Option<(Ticks, Ticks)>,
    pub tail_seconds: f64,
}

impl RenderOptions {
    pub fn timing(self, project: &Project, sample_rate: f64) -> Result<(Ticks, usize), String> {
        RenderSettings::check_tail("render tail", self.tail_seconds)?;
        let (start, length) = match self.range {
            Some((start, end)) if end > start => (start, end - start),
            Some(_) => return Err("render range must end after it starts".into()),
            None => (0, project.song_length()),
        };
        let seconds = ticks_to_seconds(length as f64, project.bpm) + self.tail_seconds;
        Ok((start, (seconds * sample_rate).round() as usize))
    }
}

/// What a built-in node was created from, to know when to rebuild it.
#[derive(Debug, Clone, PartialEq)]
enum BuiltIn {
    Synth(SynthParams),
    Sampler(String, u8),
}

pub struct Session {
    engine: Arc<Mutex<Engine>>,
    handle: EngineHandle,
    _stream: Option<cpal::Stream>,
    pub audio_error: Option<String>,
    pub preparation_error: Option<audio::Error>,
    controllers: HashMap<InstanceId, Box<dyn Controller>>,
    params: HashMap<InstanceId, Vec<ParamInfo>>,
    plugin_states: HashMap<InstanceId, PluginState>,
    pub load_errors: HashMap<InstanceId, String>,
    built_in: HashMap<u64, BuiltIn>,
    samples: audio::Cache,
    preparation: Option<crate::processing::Preparation>,
    /// Waveform summaries of loaded samples, from `processing::prepare_peaks`.
    peaks: crate::processing::Peaks,
    /// Microphone input, while audio recording is armed.
    recorder: Option<Recorder>,
    #[cfg(test)]
    pub song_updates: usize,
}

impl Session {
    pub fn new() -> Self {
        let (sample_rate, rate_error) = match output::default_sample_rate() {
            Ok(rate) => (rate, None),
            Err(error) => (48_000.0, Some(error.to_string())),
        };
        let (engine, handle) = daw_engine::create(sample_rate);
        let engine = Arc::new(Mutex::new(engine));
        let (stream, audio_error) = match rate_error {
            Some(error) => (None, Some(error)),
            None => match output::start(engine.clone()) {
                Ok(stream) => (Some(stream), None),
                Err(error) => (None, Some(error.to_string())),
            },
        };
        if let Some(error) = &audio_error {
            log::error!("audio output unavailable: {error}");
        }
        Self {
            engine,
            handle,
            _stream: stream,
            audio_error,
            preparation_error: None,
            controllers: HashMap::new(),
            params: HashMap::new(),
            plugin_states: HashMap::new(),
            load_errors: HashMap::new(),
            built_in: HashMap::new(),
            samples: audio::Cache::default(),
            preparation: None,
            peaks: HashMap::new(),
            recorder: None,
            #[cfg(test)]
            song_updates: 0,
        }
    }

    pub fn sample_rate(&self) -> f64 {
        self.handle.sample_rate
    }

    pub fn shared(&self) -> &Arc<daw_engine::engine::Shared> {
        &self.handle.shared
    }

    fn send(&mut self, command: Command) {
        if self.handle.send(command).is_err() {
            log::warn!("engine command queue full; command dropped");
        }
    }

    /// Instantiate plugins and built-in instruments that are missing, and drop
    /// nodes that no longer belong to the project.
    pub fn sync(&mut self, project: &mut Project) {
        for instance in project.plugins.clone() {
            if self.controllers.contains_key(&instance.id) || self.load_errors.contains_key(&instance.id) {
                continue;
            }
            match daw_plugins::load(&instance.plugin, &instance.state, self.sample_rate(), MAX_BLOCK) {
                Ok(loaded) => {
                    self.params.insert(instance.id, loaded.controller.params());
                    self.controllers.insert(instance.id, loaded.controller);
                    self.remember_plugin(instance.id);
                    self.send(Command::AddNode(Node::new(instance.id.0, loaded.processor)));
                }
                Err(error) => {
                    log::error!("could not load {}: {error}", instance.plugin.name);
                    self.load_errors.insert(instance.id, error.to_string());
                }
            }
        }
        let live: Vec<InstanceId> = self.controllers.keys().copied().collect();
        for id in live {
            if project.plugin(id).is_none() {
                self.controllers.remove(&id);
                self.params.remove(&id);
                self.plugin_states.remove(&id);
                self.send(Command::RemoveNode(id.0));
            }
        }
        self.load_errors.retain(|id, _| project.plugin(*id).is_some());

        for channel in &project.channels {
            let key = channel_node(&channel.source, channel.id);
            let wanted = match &channel.source {
                Source::Synth(params) => BuiltIn::Synth(*params),
                Source::Sampler { path, root_key } => BuiltIn::Sampler(path.clone(), *root_key),
                // Audio channels get a sampler too, so the channel rack can preview them.
                Source::Audio { path } => BuiltIn::Sampler(path.clone(), daw_model::DEFAULT_KEY),
                Source::Plugin(_) => continue,
            };
            match (self.built_in.get(&key), &wanted) {
                (Some(existing), _) if *existing == wanted => {}
                // Synth parameter edits go through events so voices keep ringing.
                (Some(BuiltIn::Synth(old)), BuiltIn::Synth(new)) => {
                    let (old, new) = (*old, *new);
                    self.update_synth(key, old, new);
                    self.built_in.insert(key, wanted);
                }
                _ => {
                    let node = match &wanted {
                        BuiltIn::Synth(params) => Node::new(key, Box::new(Synth::new(*params, self.sample_rate()))),
                        BuiltIn::Sampler(path, root) => match self.samples.get(path) {
                            Some(sample) => Node::new(key, Box::new(Sampler::new(sample.clone(), *root, self.sample_rate()))),
                            None => {
                                if self.built_in.remove(&key).is_some() { self.send(Command::RemoveNode(key)); }
                                continue;
                            }
                        },
                    };
                    self.send(Command::AddNode(node));
                    self.built_in.insert(key, wanted);
                }
            }
        }
        let keys: Vec<u64> = self.built_in.keys().copied().collect();
        for key in keys {
            let alive = project.channels.iter().any(|c| !matches!(c.source, Source::Plugin(_)) && c.id.0 == key);
            if !alive {
                self.built_in.remove(&key);
                self.send(Command::RemoveNode(key));
            }
        }
    }

    fn update_synth(&mut self, key: u64, old: SynthParams, new: SynthParams) {
        let changes = [
            (PARAM_CUTOFF, old.cutoff != new.cutoff, new.cutoff),
            (PARAM_ATTACK, old.attack != new.attack, new.attack / 2.0),
            (PARAM_RELEASE, old.release != new.release, new.release / 4.0),
            (
                PARAM_WAVEFORM,
                old.waveform != new.waveform,
                match new.waveform {
                    Waveform::Sine => 0.0,
                    Waveform::Saw => 0.5,
                    Waveform::Square => 1.0,
                },
            ),
        ];
        for (id, changed, value) in changes {
            if changed {
                if id == PARAM_CUTOFF {
                    self.send(Command::Mix { target: EngineTarget::Node { node: key, param: id }, value });
                } else {
                    self.send(Command::Param { node: key, id, value });
                }
            }
        }
    }

    pub fn set_synth(&mut self, channel: ChannelId, params: SynthParams) {
        if let Some(BuiltIn::Synth(old)) = self.built_in.get(&channel.0) {
            self.update_synth(channel.0, *old, params);
            self.built_in.insert(channel.0, BuiltIn::Synth(params));
        }
    }

    #[cfg(test)]
    pub fn test_controller(&mut self, instance: InstanceId, controller: Box<dyn Controller>) {
        self.params.insert(instance, controller.params());
        self.controllers.insert(instance, controller);
        self.remember_plugin(instance);
    }

    /// Change a normalized value in the playing plan without recompiling it.
    pub fn set_mix(&mut self, project: &Project, target: Target, value: f32) {
        if let Some(target) = engine_target(project, target) { self.send(Command::Mix { target, value }); }
    }

    pub fn set_send_level(&mut self, project: &Project, from: InsertId, to: InsertId, value: f32) {
        let from = project.mixer.inserts.iter().position(|i| i.id == from);
        let to = project.mixer.inserts.iter().position(|i| i.id == to);
        if let (Some(from), Some(to)) = (from, to) { self.send(Command::SendLevel { from, to, value }); }
    }

    /// A loaded file and its waveform peaks, for drawing.
    pub fn waveform(&self, path: &str) -> Option<(&Sample, &[f32])> {
        Some((self.samples.get(path)?, self.peaks.get(path)?))
    }

    pub fn update_song(&mut self, project: &Project, mode: PlayMode) -> Result<(), String> {
        #[cfg(test)]
        { self.song_updates += 1; }
        let needed = audio::Cache::required(project);
        if self.samples.ready(project) && needed.iter().all(|key| self.peaks.contains_key(key)) {
            self.preparation = None;
            self.preparation_error = None;
            self.samples.trim(project);
        } else if self.preparation.as_ref().is_none_or(|job| job.needed != needed) {
            self.preparation_error = None;
            self.preparation = Some(crate::processing::Preparation::start(project.clone(), self.samples.clone(), self.peaks.clone())?);
        }
        self.publish_song(project, mode);
        Ok(())
    }

    fn publish_song(&mut self, project: &Project, mode: PlayMode) {
        self.peaks.retain(|key, _| self.samples.contains_key(key));
        let song = compile(project, mode, MAX_BLOCK, &self.samples);
        self.send(Command::Song(Box::new(song)));
    }

    pub fn audio_progress(&self) -> Option<f32> { self.preparation.as_ref().map(|job| job.control.progress()) }

    pub fn cancel_audio(&mut self) {
        if let Some(job) = &self.preparation { job.control.cancel(); }
    }

    pub fn poll_preparation(&mut self, project: &mut Project, mode: PlayMode) -> Option<Result<(), audio::Error>> {
        let job = self.preparation.as_ref()?;
        let polled = job.poll();
        if matches!(&polled, Ok(None)) { return None; }
        let cancelled = job.control.cancelled();
        self.preparation = None;
        let result = match polled {
            Ok(Some(finished)) if !cancelled => { self.samples = finished.cache; self.peaks = finished.peaks; finished.result }
            Ok(_) => Err(audio::Error::Cancelled),
            Err(error) => Err(audio::Error::Failed(error)),
        };
        self.preparation_error = result.as_ref().err().cloned();
        self.sync(project);
        self.publish_song(project, mode);
        Some(result)
    }

    #[cfg(test)]
    pub fn wait_preparation(&mut self, project: &mut Project, mode: PlayMode) -> Result<(), audio::Error> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while self.audio_progress().is_some() {
            if let Some(result) = self.poll_preparation(project, mode) { return result; }
            assert!(std::time::Instant::now() < deadline, "audio preparation timed out");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        Ok(())
    }

    pub fn play(&mut self) {
        self.send(Command::Play);
    }

    pub fn stop(&mut self) {
        self.send(Command::Stop);
    }

    pub fn seek(&mut self, tick: f64) {
        self.send(Command::Seek(tick));
    }

    pub fn attach_midi(&mut self, input: rtrb::Consumer<daw_engine::engine::LiveNote>) {
        self.send(Command::MidiInput(input));
    }

    pub fn note(&mut self, node: u64, key: u8, velocity: f32) {
        self.send(Command::Note { node, key, velocity });
    }

    pub fn release_notes(&mut self) { self.send(Command::ReleaseNotes); }

    /// Open the default input device; audio is recorded whenever the song plays.
    pub fn arm_recording(&mut self) -> Result<(), String> {
        if self.recorder.is_none() {
            self.recorder = Some(Recorder::start(self.handle.shared.clone()).map_err(|e| e.to_string())?);
        }
        Ok(())
    }

    /// Close the input device, returning the takes not yet collected.
    pub fn disarm_recording(&mut self) -> Vec<Take> {
        self.recorder.take().map(Recorder::finish).unwrap_or_default()
    }

    pub fn recording_armed(&self) -> bool {
        self.recorder.is_some()
    }

    /// Takes that ended since the last call, and whether input was dropped.
    pub fn collect_takes(&mut self) -> (Vec<Take>, bool) {
        match &mut self.recorder {
            Some(recorder) => (recorder.poll(), recorder.take_overflow()),
            None => (Vec::new(), false),
        }
    }

    /// The take being recorded, as collected so far.
    pub fn current_take(&self) -> Option<&Take> {
        self.recorder.as_ref()?.current()
    }

    /// Set a plugin parameter from the app: updates the plugin's controller and
    /// sends the change to the audio thread.
    pub fn set_plugin_param(&mut self, instance: InstanceId, param: u32, value: f32) {
        if let Some(controller) = self.controllers.get_mut(&instance) {
            controller.set_param(param, value);
        }
        self.send(Command::Param { node: instance.0, id: param, value });
    }

    pub fn params(&self, instance: InstanceId) -> &[ParamInfo] {
        self.params.get(&instance).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn param_value(&self, instance: InstanceId, param: u32) -> f32 {
        self.controllers.get(&instance).map(|c| c.param_value(param)).unwrap_or(0.0)
    }

    pub fn param_text(&self, instance: InstanceId, param: u32, value: f32) -> String {
        self.controllers.get(&instance).map(|c| c.param_text(param, value)).unwrap_or_else(|| format!("{value:.3}"))
    }

    pub fn param_name(&self, instance: InstanceId, param: u32) -> String {
        self.params(instance).iter().find(|p| p.id == param).map(|p| p.name.clone()).unwrap_or_else(|| format!("param {param}"))
    }

    pub fn param_steps(&self, instance: InstanceId, param: u32) -> u32 {
        self.params(instance).iter().find(|p| p.id == param).map(|p| p.steps).unwrap_or(0)
    }

    pub fn has_editor(&self, instance: InstanceId) -> bool {
        self.controllers.get(&instance).is_some_and(|c| c.has_editor())
    }

    /// Show the editor, or bring it to the front when it is already showing.
    pub fn open_editor(&mut self, instance: InstanceId, title: &str) -> Result<(), String> {
        let controller = self.controllers.get_mut(&instance).ok_or("plugin is not loaded")?;
        controller.open_editor(title).map_err(|e| e.to_string())
    }

    /// Hide the editor if it is showing, otherwise show it. Returns whether
    /// it is now showing.
    pub fn toggle_editor(&mut self, instance: InstanceId, title: &str) -> Result<bool, String> {
        let controller = self.controllers.get_mut(&instance).ok_or("plugin is not loaded")?;
        if controller.editor_open() {
            controller.hide_editor();
            return Ok(false);
        }
        controller.open_editor(title).map_err(|e| e.to_string())?;
        Ok(true)
    }

    pub fn editor_open(&self, instance: InstanceId) -> bool {
        self.controllers.get(&instance).is_some_and(|c| c.editor_open())
    }

    /// Parameter edits made in plugin editors since the last poll.
    pub fn poll(&mut self) -> Vec<(InstanceId, Touch)> {
        self.handle.collect_garbage();
        let mut touches = Vec::new();
        for (id, controller) in &mut self.controllers {
            touches.extend(controller.take_touches().into_iter().map(|t| (*id, t)));
        }
        touches
    }

    /// Copy plugin states into the project before saving.
    pub fn store_states(&mut self, project: &mut Project) {
        self.capture_plugins(project, None);
    }

    fn remember_plugin(&mut self, id: InstanceId) {
        let Some(controller) = self.controllers.get(&id) else { return };
        if let Ok(state) = controller.save_state() {
            let parameters = self.current_parameters(id);
            self.plugin_states.insert(id, PluginState { state, parameters });
        }
    }

    /// Every parameter of a loaded plugin with its controller's value.
    fn current_parameters(&self, id: InstanceId) -> Vec<(u32, f32)> {
        let Some(controller) = self.controllers.get(&id) else { return Vec::new() };
        self.params(id).iter().map(|p| (p.id, controller.param_value(p.id))).collect()
    }

    /// Native callbacks are polled after the plugin has already changed.
    /// Use the last observed values for that plugin's pre-gesture snapshot.
    pub fn capture_plugins(&mut self, project: &mut Project, previous: Option<InstanceId>) -> PluginParameters {
        let mut parameters = HashMap::new();
        for instance in &mut project.plugins {
            if previous == Some(instance.id) && let Some(saved) = self.plugin_states.get(&instance.id) {
                instance.state = saved.state.clone();
                parameters.insert(instance.id, saved.parameters.clone());
                continue;
            }
            if let Some(controller) = self.controllers.get(&instance.id) {
                match controller.save_state() {
                    Ok(state) => {
                        let values = self.current_parameters(instance.id);
                        instance.state = state.clone();
                        parameters.insert(instance.id, values.clone());
                        self.plugin_states.insert(instance.id, PluginState { state, parameters: values });
                    }
                    Err(error) => log::warn!("could not save state of {}: {error}", instance.plugin.name),
                }
            }
        }
        parameters
    }

    pub fn previous_parameter(&self, id: InstanceId, param: u32) -> Option<f32> {
        self.plugin_states.get(&id)?.parameters.iter().find(|(p, _)| *p == param).map(|(_, value)| *value)
    }

    pub fn record_parameter(&mut self, id: InstanceId, param: u32, value: f32) {
        if let Some(saved) = self.plugin_states.get_mut(&id) {
            if let Some((_, previous)) = saved.parameters.iter_mut().find(|(p, _)| *p == param) { *previous = value; }
            else { saved.parameters.push((param, value)); }
        }
    }

    pub fn finish_plugin_gesture(&mut self, project: &mut Project, id: InstanceId) {
        let Some(controller) = self.controllers.get(&id) else { return };
        match controller.save_state() {
            Ok(state) => {
                if let Some(instance) = project.plugin_mut(id) { instance.state = state.clone(); }
                if let Some(saved) = self.plugin_states.get_mut(&id) { saved.state = state; }
            }
            Err(error) => log::warn!("could not save plugin gesture: {error}"),
        }
    }

    pub fn restore_plugins(&mut self, project: &Project, parameters: &PluginParameters) -> Result<(), String> {
        let mut commands = Vec::new();
        let mut errors = Vec::new();
        {
            let _engine = self.engine.lock().map_err(|_| "audio engine lock poisoned")?;
            for instance in &project.plugins {
                let values = parameters.get(&instance.id).cloned().unwrap_or_default();
                if self.plugin_states.get(&instance.id).is_some_and(|s| s.state == instance.state && s.parameters == values) { continue; }
                let Some(controller) = self.controllers.get_mut(&instance.id) else { continue };
                if !instance.state.is_empty() && let Err(error) = controller.restore_state(&instance.state) {
                    errors.push(format!("{}: {error}", instance.plugin.name));
                    continue;
                }
                for &(param, value) in &values {
                    controller.set_param(param, value);
                    commands.push(Command::Param { node: instance.id.0, id: param, value });
                }
                self.params.insert(instance.id, controller.params());
                self.plugin_states.insert(instance.id, PluginState { state: instance.state.clone(), parameters: values });
            }
        }
        for command in commands { self.send(command); }
        if errors.is_empty() { Ok(()) } else { Err(errors.join("; ")) }
    }

    /// Drop every plugin and built-in node, e.g. before opening another project.
    pub fn clear(&mut self) {
        self.preparation = None;
        for id in self.controllers.keys().copied().collect::<Vec<_>>() {
            self.send(Command::RemoveNode(id.0));
        }
        for key in self.built_in.keys().copied().collect::<Vec<_>>() {
            self.send(Command::RemoveNode(key));
        }
        self.controllers.clear();
        self.params.clear();
        self.plugin_states.clear();
        self.load_errors.clear();
        self.built_in.clear();
        self.stop();
    }

    pub fn renderer(&self, project: &Project) -> Result<crate::render::Renderer, String> {
        let parameters = self.controllers.keys().map(|&id| (id, self.current_parameters(id))).collect();
        crate::render::Renderer::new(project, self.samples.clone(), parameters, self.sample_rate())
    }

    /// Synchronous adapter for audio comparisons.
    #[cfg(test)]
    pub fn export(&self, project: &Project, path: &Path, options: RenderOptions, stems: &[Stem]) -> Result<(), String> {
        let mut renderer = self.renderer(project)?;
        renderer.worker.run(path, options, stems, &crate::processing::Control::default()).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_cancellation_retry_and_undo_ignore_obsolete_workers() {
        let mut session = Session::new();
        let mut project = Project::new();
        let path = "cached.wav";
        let channel = project.add_channel("audio", Source::Audio { path: path.into() });
        project.add_audio_clip(0, 0, channel, 3840);
        let left: Vec<_> = (0..48_000).map(|i| (i as f32 * 0.1).sin()).collect();
        let original = Arc::new(Sample { sample_rate: 48_000.0, right: left.clone(), left });
        session.samples.insert(path.into(), original.clone());
        session.update_song(&project, PlayMode::Song).unwrap();
        session.wait_preparation(&mut project, PlayMode::Song).unwrap();
        project.playlist.clips[0].audio.semitones = 3.0;
        session.update_song(&project, PlayMode::Song).unwrap();
        let cancelled_key = audio::cache_key(path, project.playlist.clips[0].audio);
        session.cancel_audio();
        assert!(session.wait_preparation(&mut project, PlayMode::Song).is_err());
        assert!(!session.samples.contains_key(&cancelled_key));
        assert!(Arc::ptr_eq(&original, &session.samples[path]));
        session.update_song(&project, PlayMode::Song).unwrap();
        session.wait_preparation(&mut project, PlayMode::Song).unwrap();
        assert!(session.samples.contains_key(&cancelled_key));

        project.playlist.clips[0].audio.semitones = 7.0;
        session.update_song(&project, PlayMode::Song).unwrap();
        let obsolete_key = audio::cache_key(path, project.playlist.clips[0].audio);
        project.playlist.clips[0].audio.semitones = 12.0;
        session.update_song(&project, PlayMode::Song).unwrap();
        session.wait_preparation(&mut project, PlayMode::Song).unwrap();
        assert!(!session.samples.contains_key(&obsolete_key));
        assert!(session.samples.contains_key(&audio::cache_key(path, project.playlist.clips[0].audio)));
        project.playlist.clips[0].audio = daw_model::AudioEdit::default();
        session.update_song(&project, PlayMode::Song).unwrap();
        assert!(session.audio_progress().is_none(), "undo reuses the cached audio and peaks immediately");
        assert!(Arc::ptr_eq(&original, &session.samples[path]));
    }

    #[test]
    fn independent_export_does_not_lock_or_move_live_playback() {
        let mut session = Session::new();
        let mut project = Project::new();
        session.sync(&mut project);
        session.update_song(&project, PlayMode::Song).unwrap();
        let path = std::env::temp_dir().join(format!("daw-unlocked-render-{}.wav", crate::project_files::stamp()));
        let mut live = session.engine.lock().unwrap();
        live.start_offline(960);
        let before = live.position();
        // Rendering while the live mutex is held would deadlock a shared-engine export.
        session.export(&project, &path, RenderOptions { depth: BitDepth::Float32, range: Some((0, 960)), tail_seconds: 0.0 }, &[]).unwrap();
        assert_eq!(live.position(), before);
        live.render_offline(512, |_, _| {});
        assert!(live.position() > before, "live playback still advances after the export");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn missing_audio_replaces_the_previous_playback_plan() {
        let mut session = Session::new();
        let mut project = Project::new();
        let path = "cached.wav";
        let channel = project.add_channel("audio", Source::Audio { path: path.into() });
        project.add_audio_clip(0, 0, channel, 3840);
        session.samples.insert(path.into(), Arc::new(Sample { sample_rate: 48_000.0, left: vec![0.5; 48_000], right: vec![0.5; 48_000] }));
        session.update_song(&project, PlayMode::Song).unwrap();
        {
            let mut engine = session.engine.lock().unwrap();
            engine.start_offline(0);
            let mut audible = false;
            engine.render_offline(512, |l, _| audible |= l.iter().any(|s| s.abs() > 0.01));
            assert!(audible);
        }
        project.channel_mut(channel).unwrap().source = Source::Audio {
            path: std::env::temp_dir().join(format!("missing-{}.wav", crate::project_files::stamp())).to_string_lossy().into_owned(),
        };
        session.update_song(&project, PlayMode::Song).unwrap();
        assert!(session.wait_preparation(&mut project, PlayMode::Song).is_err());
        let mut engine = session.engine.lock().unwrap();
        engine.start_offline(0);
        engine.render_offline(512, |l, r| assert!(l.iter().chain(r).all(|s| *s == 0.0)));
    }
}
