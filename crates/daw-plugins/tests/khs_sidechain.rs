//! Native sidechain checks. With the free kHs Compressor installed on macOS:
//! `cargo test -p daw-plugins --test khs_sidechain -- --ignored --test-threads=1`
//! Fixtures set threshold to -17 dB, ratio to 21.35:1, and external sidechain on.

#![cfg(target_os = "macos")]

use std::collections::HashMap;
use std::sync::Arc;

use daw_engine::song::{PlayMode, compile};
use daw_engine::synth::Sample;
use daw_engine::{Command, MAX_BLOCK, Node, create};
use daw_model::{ClipSource, PluginFormat, PluginRef, Project, Send, Source};

fn render(plugin: &PluginRef, state: &[u8], detector: f32) -> (f64, Vec<u8>) {
    let loaded = daw_plugins::load(plugin, state, 48_000.0, MAX_BLOCK).expect("load compressor");
    let mut project = Project::new();
    project.bpm = 120.0;
    for insert in &mut project.mixer.inserts { insert.volume = 1.0; }
    let instance = project.add_plugin(plugin.clone());
    project.mixer.inserts[1].effects.push(instance);
    let main = project.add_channel("main", Source::Audio { path: "main.wav".into() });
    let side = project.add_channel("detector", Source::Audio { path: "detector.wav".into() });
    let source_insert = project.mixer.inserts[2].id;
    let destination = project.mixer.inserts[1].id;
    let muted_output = project.mixer.inserts[3].id;
    project.channel_mut(main).unwrap().insert = destination;
    project.channel_mut(main).unwrap().volume = 1.0;
    project.channel_mut(side).unwrap().insert = source_insert;
    project.channel_mut(side).unwrap().volume = 1.0;
    project.mixer.inserts[3].volume = 0.0;
    assert!(project.mixer.set_output(source_insert, muted_output));
    assert!(project.mixer.set_send(source_insert, Send { to: destination, level: 1.0, sidechain: true }));
    project.add_clip(0, 0, ClipSource::Audio(main));
    project.add_clip(1, 0, ClipSource::Audio(side));
    let tone = |amplitude: f32| {
        let left: Vec<_> = (0..96_000).map(|i| amplitude * (i as f32 * std::f32::consts::TAU * 220.0 / 48_000.0).sin()).collect();
        Arc::new(Sample { sample_rate: 48_000.0, right: left.clone(), left })
    };
    let samples = HashMap::from([("main.wav".into(), tone(0.02)), ("detector.wav".into(), tone(detector))]);
    let (mut engine, mut handle) = create(48_000.0);
    assert!(handle.send(Command::AddNode(Node::new(instance.0, loaded.processor))).is_ok());
    assert!(handle.send(Command::Song(Box::new(compile(&project, PlayMode::Song, MAX_BLOCK, &samples)))).is_ok());
    engine.start_offline(0);
    let mut signal = Vec::new();
    engine.render_offline(65_536, |l, _| signal.extend_from_slice(l));
    let rms = (signal[32_768..].iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / 32_768.0).sqrt();
    (rms, loaded.controller.save_state().expect("save compressor state"))
}

fn verify(format: PluginFormat, state: &[u8]) {
    let au = format == PluginFormat::AudioUnit;
    let plugin = PluginRef {
        format,
        id: if au { "61756678-6b736370-206b4873" } else { "34ABB87000624821003D227500C57453" }.into(),
        path: if au { "" } else { "/Library/Audio/Plug-Ins/VST3/Kilohearts/kHs Compressor.vst3" }.into(),
        name: "kHs Compressor".into(), vendor: "Kilohearts".into(),
    };
    let (quiet, saved) = render(&plugin, state, 0.0);
    let (loud, _) = render(&plugin, &saved, 0.8);
    assert!((0.013..0.015).contains(&quiet), "main RMS {quiet}");
    assert!((0.18..0.23).contains(&(loud / quiet)), "detector gain ratio {}", loud / quiet);
    let (off, _) = render(&plugin, &[], 0.0);
    let (off_with_detector, _) = render(&plugin, &[], 0.8);
    assert!((off_with_detector - off).abs() < 0.00001, "detector leaked into main audio");
}

#[test]
#[ignore = "requires kHs Compressor VST3"]
fn vst3_external_sidechain_survives_state_restore() {
    verify(PluginFormat::Vst3, include_bytes!("fixtures/khs-compressor-vst3.bin"));
}

#[test]
#[ignore = "requires kHs Compressor Audio Unit"]
fn au_external_sidechain_survives_state_restore() {
    verify(PluginFormat::AudioUnit, include_bytes!("fixtures/khs-compressor-au.bin"));
}
