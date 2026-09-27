//! List Audio Units, then load the ones whose name contains the argument and
//! run a note or noise through them.

#[cfg(target_os = "macos")]
use daw_engine::{Event, EventKind, TransportInfo};

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("Audio Units are only supported on macOS.");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
fn main() {
    let filter = std::env::args().nth(1).unwrap_or_default();
    let all = daw_plugins::au::list();
    println!("{} audio units", all.len());
    for (info, _) in all.iter().filter(|(i, _)| i.plugin.name.contains(&filter)).take(5) {
        println!("{:?} {} / {} [{}] {}", info.kind, info.plugin.name, info.plugin.vendor, info.category, info.plugin.id);
        let loaded = match daw_plugins::load(&info.plugin, &[], 48000.0, 512) {
            Ok(loaded) => loaded,
            Err(error) => {
                println!("  load failed: {error}");
                continue;
            }
        };
        let mut processor = loaded.processor;
        let controller = loaded.controller;
        let params = controller.params();
        println!("  params: {} editor: {}", params.len(), controller.has_editor());
        if let Some(p) = params.first() {
            println!("  first: {} = {}", p.name, controller.param_text(p.id, controller.param_value(p.id)));
        }
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
        if let Ok(state) = controller.save_state() {
            match daw_plugins::load(&info.plugin, &state, 48000.0, 512) {
                Ok(_) => println!("  reload with state ok"),
                Err(e) => println!("  reload failed: {e}"),
            }
        }
    }
}
