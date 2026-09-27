//! Writes a small demo project: `cargo run -p daw-model --example demo_project -- out.dawproj`.
//! Workspace 1 is the default layout, 2 the automation editor, 3 mixer and
//! parameters, 4 settings.

use daw_model::automation::{Point, Shape};
use daw_model::layout::{Axis, Layout, Panel};
use daw_model::{ClipSource, Note, PluginFormat, PluginRef, Project, STEP_TICKS, Source, Target};

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "demo.dawproj".into());
    // `--plugins-only` leaves the built-in synth silent, to hear only Vital through OTT.
    let mut project = Project::new();
    let bar = project.signature.ticks_per_bar();
    let synth = project.channels[0].id;
    let pattern = project.patterns[0].id;

    let vital = project.add_plugin(PluginRef {
        format: PluginFormat::Vst3,
        id: "56535456697461766974616C00000000".into(),
        path: "/Library/Audio/Plug-Ins/VST3/Vital.vst3".into(),
        name: "Vital".into(),
        vendor: "Vital Audio".into(),
    });
    let lead = project.add_channel("Vital", Source::Plugin(vital));
    let ott = project.add_plugin(PluginRef {
        format: PluginFormat::AudioUnit,
        id: "61756678-58665454-58464552".into(),
        path: String::new(),
        name: "OTT".into(),
        vendor: "Xfer Records".into(),
    });
    project.mixer.inserts[2].effects.push(ott);

    let plugins_only = std::env::args().any(|a| a == "--plugins-only");
    let p = project.pattern_mut(pattern).unwrap();
    p.length = bar * 2;
    for step in [0, 4, 8, 12, 16, 20, 24, 28, 30] {
        if !plugins_only {
            p.toggle_step(synth, step);
        }
    }
    let melody = [(0, 64, 2), (2, 67, 2), (4, 71, 4), (8, 69, 2), (10, 67, 2), (12, 64, 4), (16, 62, 6), (24, 60, 8)];
    for (step, key, length) in melody {
        p.notes_mut(lead).push(Note { start: step * STEP_TICKS, length: length * STEP_TICKS, key, velocity: 0.8 });
    }

    let cutoff = project.add_automation("synth cutoff", Target::SynthCutoff(synth), 0.3);
    let clip = project.automation_clip_mut(cutoff).unwrap();
    clip.envelope.points = vec![
        Point { shape: Shape::Curve, tension: 0.6, ..Point::new(0, 0.2) },
        Point { shape: Shape::SCurve, tension: 0.0, ..Point::new(bar, 0.9) },
        Point { shape: Shape::Hold, ..Point::new(bar * 2, 0.4) },
        Point { shape: Shape::Stairs(4), ..Point::new(bar * 2 + bar / 2, 0.7) },
        Point { shape: Shape::Linear, ..Point::new(bar * 3, 0.1) },
        Point::new(bar * 4, 0.5),
    ];
    let volume = project.add_automation("insert 1 volume", Target::InsertVolume(project.mixer.inserts[1].id), 0.4);
    for start in [0, bar * 2, bar * 4, bar * 6] {
        project.add_clip(0, start, ClipSource::Pattern(pattern));
    }
    project.add_clip(1, 0, ClipSource::Automation(cutoff));
    project.add_clip(1, bar * 4, ClipSource::Automation(cutoff));
    project.add_clip(2, bar * 2, ClipSource::Automation(volume));
    project.playlist.loop_range = Some((0, bar * 8));

    project.workspaces.layouts[1] = Layout::single(Panel::Automation);
    let mut mix = Layout::single(Panel::Mixer);
    mix.split(Axis::Horizontal, Panel::Parameters);
    project.workspaces.layouts[2] = mix;
    project.workspaces.layouts[3] = Layout::single(Panel::Settings);

    std::fs::write(&path, project.to_ron().unwrap()).unwrap();
    println!("wrote {path}");
}
