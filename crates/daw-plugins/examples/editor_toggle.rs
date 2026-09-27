//! Show and hide a plugin editor several times, checking that each show
//! works. Pass a VST3 bundle path, or part of an Audio Unit's name.

use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use objc2::MainThreadMarker;
#[cfg(target_os = "macos")]
use objc2_app_kit::NSApplication;

fn pump(controller: &mut dyn daw_plugins::Controller, seconds: f64) {
    let end = Instant::now() + Duration::from_secs_f64(seconds);
    while Instant::now() < end {
        #[cfg(target_os = "macos")]
        unsafe { objc2_core_foundation::CFRunLoop::run_in_mode(objc2_core_foundation::kCFRunLoopDefaultMode, 0.02, true) };
        controller.take_touches();
        #[cfg(not(target_os = "macos"))]
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn main() {
    let arg = std::env::args().nth(1).expect("usage: editor_toggle <vst3 bundle | au name>");
    #[cfg(target_os = "macos")]
    let _app = NSApplication::sharedApplication(MainThreadMarker::new().expect("main thread"));
    let info = if arg.ends_with(".vst3") {
        daw_plugins::vst3::scan_bundle(std::path::Path::new(&arg)).expect("scan").remove(0).plugin
    } else {
        #[cfg(target_os = "macos")]
        { daw_plugins::au::list().into_iter().find(|(i, _)| i.plugin.name == arg).expect("no such audio unit").0.plugin }
        #[cfg(not(target_os = "macos"))]
        { panic!("pass a .vst3 bundle path; Audio Units require macOS"); }
    };
    let mut controller = daw_plugins::load(&info, &[], 48000.0, 512).expect("load").controller;
    for round in 1..=5 {
        controller.open_editor(&info.name).expect("open editor");
        pump(&mut *controller, 0.5);
        assert!(controller.editor_open());
        println!("round {round}: open, visible {}", controller.editor_open());
        controller.hide_editor();
        pump(&mut *controller, 0.3);
        assert!(!controller.editor_open());
        println!("round {round}: hidden, visible {}", controller.editor_open());
    }
}
