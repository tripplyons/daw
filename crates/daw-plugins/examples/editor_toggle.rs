//! Show and hide a plugin editor several times, checking that each show
//! works. Pass a VST3 bundle path, or part of an Audio Unit's name.

use std::time::{Duration, Instant};

use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;

fn pump(seconds: f64) {
    let end = Instant::now() + Duration::from_secs_f64(seconds);
    while Instant::now() < end {
        unsafe { objc2_core_foundation::CFRunLoop::run_in_mode(objc2_core_foundation::kCFRunLoopDefaultMode, 0.02, true) };
    }
}

fn main() {
    let arg = std::env::args().nth(1).expect("usage: editor_toggle <vst3 bundle | au name>");
    let _app = NSApplication::sharedApplication(MainThreadMarker::new().expect("main thread"));
    let info = if arg.ends_with(".vst3") {
        daw_plugins::vst3::scan_bundle(std::path::Path::new(&arg)).expect("scan").remove(0).plugin
    } else {
        daw_plugins::au::list().into_iter().find(|(i, _)| i.plugin.name == arg).expect("no such audio unit").0.plugin
    };
    let mut controller = daw_plugins::load(&info, &[], 48000.0, 512).expect("load").controller;
    for round in 1..=5 {
        let opened = controller.open_editor(&info.name);
        pump(0.5);
        println!("round {round}: open {opened:?}, visible {}", controller.editor_open());
        controller.hide_editor();
        pump(0.3);
        println!("round {round}: hidden, visible {}", controller.editor_open());
    }
}
