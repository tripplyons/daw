//! Load a VST3 bundle, play a note or noise through it, and print the output peak.

use daw_engine::{Event, EventKind, TransportInfo};

fn main() {
    let path = std::env::args().nth(1).expect("usage: vst3_smoke <bundle>");
    let infos = daw_plugins::vst3::scan_bundle(std::path::Path::new(&path)).expect("scan");
    for info in infos {
        println!("{:?} {} / {} [{}]", info.kind, info.plugin.name, info.plugin.vendor, info.category);
        let loaded = match daw_plugins::load(&info.plugin, &[], 48000.0, 512) {
            Ok(loaded) => loaded,
            Err(error) => {
                println!("  load failed: {error}");
                continue;
            }
        };
        let mut processor = loaded.processor;
        let controller = loaded.controller;
        println!("  params: {}", controller.params().len());
        let transport = TransportInfo {
            sample_rate: 48000.0, bpm: 120.0, beats: 0.0, frames: 0, playing: true,
            numerator: 4, denominator: 4, bar_start: 0.0,
        };
        let mut peak = 0.0f32;
        for block in 0..200 {
            let mut left: Vec<f32> = (0..512).map(|i| ((i * 7919 + block) % 97) as f32 / 97.0 - 0.5).collect();
            let mut right = left.clone();
            let events = if block == 0 { vec![Event { offset: 0, kind: EventKind::NoteOn { key: 60, velocity: 0.9 } }] } else { vec![] };
            processor.process(&transport, &events, &mut left, &mut right);
            peak = left.iter().chain(&right).fold(peak, |m, s| m.max(s.abs()));
        }
        let state = controller.save_state().map(|s| s.len());
        println!("  output peak {peak:.3}, state {state:?}");
    }
}
