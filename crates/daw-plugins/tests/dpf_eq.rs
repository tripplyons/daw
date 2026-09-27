//! Checks against a real plugin, DISTRHO's 3 Band EQ. Opt in with:
//!
//! ```sh
//! sudo apt-get install dpf-plugins-vst3
//! cargo test -p daw-plugins --test dpf_eq -- --ignored
//! ```
//!
//! `DAW_DPF_EQ_BUNDLE` overrides the bundle path.

#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::time::Duration;

use daw_engine::{Event, EventKind, TransportInfo};
use daw_plugins::{Loaded, PluginInfo, PluginKind};

fn eq() -> PluginInfo {
    let bundle = std::env::var_os("DAW_DPF_EQ_BUNDLE").map(PathBuf::from).unwrap_or("/usr/lib/vst3/3BandEQ.vst3".into());
    let infos = daw_plugins::vst3::scan_bundle(&bundle).expect("scan 3BandEQ");
    infos.into_iter().find(|info| info.plugin.name == "3 Band EQ").expect("3 Band EQ class")
}

/// RMS level of a stereo sine after the plugin, skipping the first blocks.
fn render_rms(loaded: &mut Loaded, events: &[Event]) -> f64 {
    let (mut energy, mut samples) = (0.0, 0);
    for block in 0..64 {
        let mut left = [0.0; 512];
        let mut right = [0.0; 512];
        for frame in 0..512 {
            let t = (block * 512 + frame) as f32 / 48_000.0;
            left[frame] = 0.1 * (std::f32::consts::TAU * 220.0 * t).sin();
            right[frame] = 0.05 * (std::f32::consts::TAU * 440.0 * t).sin();
        }
        let frames = (block * 512) as i64;
        let transport = TransportInfo {
            sample_rate: 48_000.0,
            bpm: 120.0,
            beats: frames as f64 / 24_000.0,
            frames,
            playing: true,
            numerator: 4,
            denominator: 4,
            bar_start: 0.0,
        };
        loaded.processor.process(&transport, if block == 0 { events } else { &[] }, &mut left, &mut right);
        if block >= 8 {
            energy += left.iter().chain(&right).map(|s| f64::from(*s).powi(2)).sum::<f64>();
            samples += 1024;
        }
    }
    (energy / f64::from(samples)).sqrt()
}

#[test]
#[ignore = "needs the 3BandEQ plugin"]
fn processes_parameters_and_restores_state() {
    let info = eq();
    assert_eq!(info.kind, PluginKind::Effect);
    let mut loaded = daw_plugins::load(&info.plugin, &[], 48_000.0, 512).expect("load");
    let master = loaded.controller.params().into_iter().find(|p| p.name == "Master").expect("master gain").id;
    let baseline = render_rms(&mut loaded, &[]);
    assert!(baseline > 0.01);

    // Master spans -24..24 dB, so 0.25 is -12 dB.
    loaded.controller.set_param(master, 0.25);
    let attenuated = render_rms(&mut loaded, &[Event { offset: 0, kind: EventKind::Param { id: master, value: 0.25 } }]);
    let ratio = attenuated / baseline;
    assert!((ratio - 10f64.powf(-12.0 / 20.0)).abs() < 0.005, "gain ratio {ratio}");
    let state = loaded.controller.save_state().expect("save state");
    drop(loaded);

    let mut restored = daw_plugins::load(&info.plugin, &state, 48_000.0, 512).expect("restore");
    assert!((restored.controller.param_value(master) - 0.25).abs() < 1e-5);
    assert!((render_rms(&mut restored, &[]) - attenuated).abs() < 1e-5);
}

#[test]
#[ignore = "needs the 3BandEQ plugin and an X11 display"]
fn editor_hides_and_reopens() {
    let info = eq();
    let mut loaded = daw_plugins::load(&info.plugin, &[], 48_000.0, 512).expect("load");
    for _ in 0..3 {
        loaded.controller.open_editor("3 Band EQ").unwrap();
        for _ in 0..10 {
            loaded.controller.take_touches();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(loaded.controller.editor_open());
        loaded.controller.hide_editor();
        assert!(!loaded.controller.editor_open());
    }
}
