//! Offline engines have their own plugin instances and never lock live playback.

use std::path::Path;

use daw_engine::audio::{self, Cache};
use daw_engine::output::{self, Stem};
use daw_engine::song::{PlayMode, channel_node, compile};
use daw_engine::synth::{Sampler, Synth};
use daw_engine::{Command, Engine, EngineHandle, MAX_BLOCK, Node};
use daw_model::{Project, Source};
use daw_plugins::Controller;

use crate::processing::Control;
use crate::session::{PluginParameters, RenderOptions};

pub struct Renderer {
    pub worker: Worker,
    // Keep native controllers alive on the main thread until processing ends.
    pub controllers: Vec<Box<dyn Controller>>,
}

pub struct Worker {
    project: Project,
    samples: Cache,
    engine: Engine,
    handle: EngineHandle,
}

impl Renderer {
    /// Plugin construction and final destruction belong on the main thread.
    pub fn new(project: &Project, samples: Cache, parameters: PluginParameters, sample_rate: f64) -> Result<Self, String> {
        let (mut engine, mut handle) = daw_engine::create(sample_rate);
        let mut controllers = Vec::new();
        for instance in &project.plugins {
            let mut loaded = daw_plugins::load(&instance.plugin, &instance.state, sample_rate, MAX_BLOCK).map_err(|e| format!("{}: {e}", instance.plugin.name))?;
            handle.send(Command::AddNode(Node::new(instance.id.0, loaded.processor))).map_err(|_| "render command queue full")?;
            if let Some(values) = parameters.get(&instance.id) {
                for &(id, value) in values {
                    if loaded.controller.param_value(id) == value { continue; }
                    loaded.controller.set_param(id, value);
                    handle.send(Command::Param { node: instance.id.0, id, value }).map_err(|_| "render parameter queue full")?;
                }
            }
            controllers.push(loaded.controller);
            engine.start_offline(0);
        }
        Ok(Self { worker: Worker { project: project.clone(), samples, engine, handle }, controllers })
    }
}

impl Worker {
    pub fn run(&mut self, path: &Path, options: RenderOptions, stems: &[Stem], control: &Control) -> Result<bool, String> {
        let (start, frames) = options.timing(&self.project, self.engine.sample_rate())?;
        self.project.playlist.loop_range = None;
        if let Some((_, end)) = options.range {
            self.project.playlist.clips.retain(|c| c.start < end);
            for clip in &mut self.project.playlist.clips { clip.length = clip.length.min(end - clip.start); }
        }
        audio::prepare_with_progress(&self.project, &mut self.samples, |p| control.update(p * 0.35))?;
        for channel in &self.project.channels {
            if control.cancelled() { return Ok(false); }
            let node = channel_node(&channel.source, channel.id);
            let processor: Box<dyn daw_engine::Processor> = match &channel.source {
                Source::Synth(params) => Box::new(Synth::new(*params, self.engine.sample_rate())),
                Source::Audio { path } => Box::new(Sampler::new(self.samples[path].clone(), daw_model::DEFAULT_KEY, self.engine.sample_rate())),
                Source::Sampler { path, root_key } => Box::new(Sampler::new(self.samples[path].clone(), *root_key, self.engine.sample_rate())),
                Source::Plugin(_) => continue,
            };
            self.handle.send(Command::AddNode(Node::new(node, processor))).map_err(|_| "render command queue full")?;
            self.engine.start_offline(0);
        }
        self.handle.send(Command::Song(Box::new(compile(&self.project, PlayMode::Song, MAX_BLOCK, &self.samples)))).map_err(|_| "render command queue full")?;
        output::export_wav_with_progress(&mut self.engine, path, start, frames, options.depth, stems, |p| control.update(0.35 + p * 0.65)).map_err(|e| e.to_string())
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::sync::Arc;
    use daw_engine::synth::Sample;
    use daw_model::{PluginFormat, PluginRef, Send};

    fn native_render(format: PluginFormat, detector: f32, boost: bool) -> f64 {
        let au = format == PluginFormat::AudioUnit;
        let plugin = PluginRef {
            format,
            id: if au { "61756678-6b736370-206b4873" } else { "34ABB87000624821003D227500C57453" }.into(),
            path: if au { "" } else { "/Library/Audio/Plug-Ins/VST3/Kilohearts/kHs Compressor.vst3" }.into(),
            name: "kHs Compressor".into(), vendor: "Kilohearts".into(),
        };
        let mut project = Project::new();
        project.bpm = 120.0;
        let instance = project.add_plugin(plugin.clone());
        project.plugin_mut(instance).unwrap().state = if au {
            include_bytes!("../../daw-plugins/tests/fixtures/khs-compressor-au.bin").to_vec()
        } else {
            include_bytes!("../../daw-plugins/tests/fixtures/khs-compressor-vst3.bin").to_vec()
        };
        for insert in &mut project.mixer.inserts { insert.volume = 1.0; }
        project.mixer.inserts[1].effects.push(instance);
        let destination = project.mixer.inserts[1].id;
        let source = project.mixer.inserts[2].id;
        let muted = project.mixer.inserts[3].id;
        project.mixer.inserts[3].volume = 0.0;
        project.mixer.set_output(source, muted);
        project.mixer.set_send(source, Send { to: destination, level: 1.0, sidechain: true });
        let mut samples = Cache::default();
        for (name, amplitude, insert) in [("main.wav", 0.02, destination), ("detector.wav", detector, source)] {
            let channel = project.add_channel(name, Source::Audio { path: name.into() });
            let channel_data = project.channel_mut(channel).unwrap();
            channel_data.volume = 1.0;
            channel_data.insert = insert;
            project.add_audio_clip(0, 0, channel, 3840);
            let left: Vec<_> = (0..96_000).map(|i| amplitude * (i as f32 * std::f32::consts::TAU * 220.0 / 48_000.0).sin()).collect();
            samples.insert(name.into(), Arc::new(Sample { sample_rate: 48_000.0, right: left.clone(), left }));
        }
        let mut parameters = PluginParameters::new();
        if boost {
            let loaded = daw_plugins::load(&plugin, &project.plugin(instance).unwrap().state, 48_000.0, MAX_BLOCK).unwrap();
            let params = loaded.controller.params();
            let makeup = params.iter().find(|p| p.name.eq_ignore_ascii_case("makeup")).unwrap_or_else(|| panic!("{format:?} makeup parameter: {:?}", params.iter().filter(|p| p.automatable).map(|p| &p.name).collect::<Vec<_>>())).id;
            parameters.insert(instance, vec![(makeup, 0.5)]);
        }
        let Renderer { mut worker, controllers } = Renderer::new(&project, samples, parameters, 48_000.0).unwrap();
        let path = std::env::temp_dir().join(format!("daw-native-worker-{}.wav", crate::project_files::stamp()));
        let output = path.clone();
        let thread = std::thread::spawn(move || {
            let result = worker.run(&output, RenderOptions { depth: output::BitDepth::Float32, range: None, tail_seconds: 0.0 }, &[], &Control::default());
            (worker, result)
        });
        let (worker, result) = thread.join().unwrap();
        assert!(result.unwrap());
        drop(worker);
        drop(controllers);
        let sample = Sample::load(path.to_str().unwrap()).unwrap();
        std::fs::remove_file(path).unwrap();
        (sample.left[32_768..65_536].iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / 32_768.0).sqrt()
    }

    #[test]
    #[ignore = "requires kHs Compressor VST3 and Audio Unit"]
    fn native_background_renders_copy_state_and_pending_parameter_values() {
        for format in [PluginFormat::Vst3, PluginFormat::AudioUnit] {
            let quiet = native_render(format, 0.0, false);
            let loud = native_render(format, 0.8, false);
            assert!((0.013..0.015).contains(&quiet), "{format:?} main RMS {quiet}");
            assert!((0.18..0.23).contains(&(loud / quiet)), "{format:?} sidechain gain {}", loud / quiet);
            let boosted = native_render(format, 0.8, true);
            assert!(boosted > loud * 2.0, "{format:?} pending makeup parameter was not copied: {boosted} vs {loud}");
        }
    }
}
