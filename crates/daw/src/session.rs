//! Keeps the realtime engine and plugin instances in step with the project.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use daw_engine::input::{Recorder, Take};
use daw_engine::output::{self, BitDepth, Stem};
use daw_engine::song::{PlayMode, channel_node, compile};
use daw_engine::synth::{PARAM_ATTACK, PARAM_CUTOFF, PARAM_RELEASE, PARAM_WAVEFORM, Sample, Sampler, Synth};
use daw_engine::{Command, Engine, EngineHandle, MAX_BLOCK, Node};
use daw_model::time::{Ticks, ticks_to_seconds};
use daw_model::{InstanceId, Project, Source, SynthParams, Waveform};
use daw_plugins::{Controller, ParamInfo, Touch};

/// Frames summarized by each waveform peak.
pub const PEAK_FRAMES: usize = 256;

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
    controllers: HashMap<InstanceId, Box<dyn Controller>>,
    params: HashMap<InstanceId, Vec<ParamInfo>>,
    pub load_errors: HashMap<InstanceId, String>,
    built_in: HashMap<u64, BuiltIn>,
    samples: HashMap<String, Arc<Sample>>,
    /// Waveform summaries of loaded samples: the largest absolute value per
    /// `PEAK_FRAMES` frames, across both sides.
    peaks: HashMap<String, Vec<f32>>,
    /// Microphone input, while audio recording is armed.
    recorder: Option<Recorder>,
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
            controllers: HashMap::new(),
            params: HashMap::new(),
            load_errors: HashMap::new(),
            built_in: HashMap::new(),
            samples: HashMap::new(),
            peaks: HashMap::new(),
            recorder: None,
        }
    }

    pub fn sample_rate(&self) -> f64 {
        self.handle.sample_rate
    }

    pub fn shared(&self) -> &daw_engine::engine::Shared {
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
                        BuiltIn::Synth(params) => Some(Node::new(key, Box::new(Synth::new(*params, self.sample_rate())))),
                        BuiltIn::Sampler(path, root) => match self.sample(path) {
                            Ok(sample) => Some(Node::new(key, Box::new(Sampler::new(sample, *root, self.sample_rate())))),
                            Err(error) => {
                                log::error!("{error}");
                                None
                            }
                        },
                    };
                    if let Some(node) = node {
                        self.send(Command::AddNode(node));
                    }
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
                self.send(Command::Param { node: key, id, value });
            }
        }
    }

    /// Load a WAV file, or take it from the cache.
    pub fn sample(&mut self, path: &str) -> Result<Arc<Sample>, String> {
        if let Some(sample) = self.samples.get(path) {
            return Ok(sample.clone());
        }
        let sample = Arc::new(Sample::load(path).map_err(|e| format!("could not load {path}: {e}"))?);
        let peaks = sample
            .left
            .chunks(PEAK_FRAMES)
            .zip(sample.right.chunks(PEAK_FRAMES))
            .map(|(l, r)| l.iter().chain(r).fold(0.0f32, |m, s| m.max(s.abs())))
            .collect();
        self.peaks.insert(path.to_owned(), peaks);
        self.samples.insert(path.to_owned(), sample.clone());
        Ok(sample)
    }

    /// A loaded file and its waveform peaks, for drawing.
    pub fn waveform(&self, path: &str) -> Option<(&Sample, &[f32])> {
        Some((self.samples.get(path)?, self.peaks.get(path)?))
    }

    pub fn update_song(&mut self, project: &Project, mode: PlayMode) {
        let song = compile(project, mode, MAX_BLOCK, &self.samples);
        self.send(Command::Song(Box::new(song)));
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

    pub fn note(&mut self, node: u64, key: u8, velocity: f32) {
        self.send(Command::Note { node, key, velocity });
    }

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
    pub fn store_states(&self, project: &mut Project) {
        for instance in &mut project.plugins {
            if let Some(controller) = self.controllers.get(&instance.id) {
                match controller.save_state() {
                    Ok(state) => instance.state = state,
                    Err(error) => log::warn!("could not save state of {}: {error}", instance.plugin.name),
                }
            }
        }
    }

    /// Drop every plugin and built-in node, e.g. before opening another project.
    pub fn clear(&mut self) {
        for id in self.controllers.keys().copied().collect::<Vec<_>>() {
            self.send(Command::RemoveNode(id.0));
        }
        for key in self.built_in.keys().copied().collect::<Vec<_>>() {
            self.send(Command::RemoveNode(key));
        }
        self.controllers.clear();
        self.params.clear();
        self.load_errors.clear();
        self.built_in.clear();
        self.stop();
    }

    /// Render the song to a WAV file, then restore the live plan. With a range,
    /// render only those ticks, without a tail; notes that start before it are
    /// not heard. Stems come from the same pass.
    pub fn export(
        &mut self,
        project: &Project,
        path: &Path,
        depth: BitDepth,
        live_mode: PlayMode,
        range: Option<(Ticks, Ticks)>,
        stems: &[Stem],
    ) -> Result<(), String> {
        self.stop();
        self.update_song(project, PlayMode::Song);
        let (start, seconds) = match range {
            Some((start, end)) => (start, ticks_to_seconds(end.saturating_sub(start) as f64, project.bpm)),
            None => (0, ticks_to_seconds(project.song_length() as f64, project.bpm) + 2.0),
        };
        let frames = (seconds * self.sample_rate()) as usize;
        let result = {
            let mut engine = self.engine.lock().map_err(|_| "audio engine lock poisoned")?;
            output::export_wav(&mut engine, path, start, frames, depth, stems).map_err(|e| e.to_string())
        };
        self.update_song(project, live_mode);
        self.seek(0.0);
        result
    }
}
