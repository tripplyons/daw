//! Drives `App::update` the way the UI does and checks the project.

use daw_engine::output::BitDepth;
use daw_model::automation::{Shape, TimeSnap, ValueSnap};
use daw_model::layout::{Axis, Panel};
use daw_model::time::Grid;
use daw_model::{ClipSource, Target};
use iced::keyboard::key::{Code, Physical};
use iced::keyboard::{self, Key, Location, Modifiers};

use super::*;
use crate::panels::{automation as auto, piano_roll as roll, playlist as list, timeline};

/// A fresh app that ignores the user's config and writes its own to a temp file.
fn app() -> App {
    let mut app = App::boot().0;
    app.keymap = crate::keys::Keymap::default();
    app.config_error = None;
    let id = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("daw-app-config-{}-{:p}", std::process::id(), &id));
    app.config_path = dir.join("config.json");
    app
}

fn wait_saves(app: &mut App) -> usize {
    let mut count = 0;
    while let Some(finished) = app.saving.wait() { let _ = app.save_finished(finished); count += 1; }
    count
}

fn wait_audio(app: &mut App) {
    app.session.wait_preparation(&mut app.project, app.mode).unwrap();
}

fn wait_renders(app: &mut App) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.rendering.busy() {
        app.poll_renders();
        assert!(std::time::Instant::now() < deadline, "audio render timed out");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

struct TestPlugin(f32);

impl daw_plugins::Controller for TestPlugin {
    fn params(&self) -> Vec<daw_plugins::ParamInfo> {
        vec![daw_plugins::ParamInfo { id: 0, name: "value".into(), units: String::new(), steps: 0, default: 0.0, automatable: true }]
    }
    fn param_value(&self, _: u32) -> f32 { self.0 }
    fn set_param(&mut self, _: u32, value: f32) { self.0 = value; }
    fn param_text(&self, _: u32, value: f32) -> String { value.to_string() }
    fn save_state(&self) -> Result<Vec<u8>, daw_plugins::PluginError> { Ok(self.0.to_le_bytes().to_vec()) }
    fn restore_state(&mut self, state: &[u8]) -> Result<(), daw_plugins::PluginError> {
        self.0 = f32::from_le_bytes(state.try_into().map_err(|_| daw_plugins::PluginError::Load("invalid test state".into()))?);
        Ok(())
    }
    fn has_editor(&self) -> bool { false }
    fn open_editor(&mut self, _: &str) -> Result<(), daw_plugins::PluginError> { Err(daw_plugins::PluginError::Load("no editor".into())) }
    fn hide_editor(&mut self) {}
    fn editor_open(&self) -> bool { false }
    fn take_touches(&mut self) -> Vec<daw_plugins::Touch> { Vec::new() }
}

fn press(code: Code, key: Key, modifiers: Modifiers) -> Message {
    Message::Key(
        keyboard::Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key: Physical::Code(code),
            location: Location::Standard,
            modifiers,
            text: None,
            repeat: false,
        },
        false,
    )
}

fn focus(app: &mut App, panel: Panel) {
    let layout = app.layout_mut();
    let tile = layout.focused;
    layout.set_panel(tile, panel);
}

fn char_key(c: &str) -> Key {
    Key::Character(c.into())
}

fn points(app: &App) -> Vec<(u64, f32, Shape)> {
    let id = app.automation.clip.expect("clip open");
    app.project.automation_clip(id).unwrap().envelope.points.iter().map(|p| (p.time, p.value, p.shape)).collect()
}

#[test]
fn bind_mode_creates_clip_on_first_touch_only() {
    let mut app = app();
    let _ = app.update(Message::Action(Action::ToggleBind));
    let channel = app.project.channels[0].id;
    app.touched(Target::ChannelVolume(channel), 0.6);
    app.touched(Target::ChannelVolume(channel), 0.7);
    assert_eq!(app.project.automation.len(), 1);
    let clip = &app.project.automation[0];
    assert_eq!(clip.target, Target::ChannelVolume(channel));
    assert!((clip.envelope.points[0].value - 0.6).abs() < 1e-6);
    assert!(app.project.playlist.clips.iter().any(|c| c.source == ClipSource::Automation(clip.id)));
    assert_eq!(app.automation.clip, Some(clip.id));
    assert!(app.layout().find_panel(Panel::Automation).is_some());
    assert_eq!(app.last_touched.front().map(|t| t.0), Some(Target::ChannelVolume(channel)));
}

#[test]
fn automation_points_snap_to_time_and_value_and_undo() {
    let mut app = app();
    let _ = app.update(Message::Automate(Target::Tempo));
    app.automation.time_snap = TimeSnap::Grid(Grid::Division(16));
    app.automation.value_snap = ValueSnap::Step(0.05);
    let before = points(&app);

    let _ = app.update(auto::Message::Add { time: 1000.0, value: 0.52, bypass: false }.into());
    let _ = app.update(auto::Message::End.into());
    assert!(points(&app).contains(&(960, 0.5, Shape::Linear)), "{:?}", points(&app));
    let index = points(&app).iter().position(|p| p.0 == 960).unwrap();

    // Drag right by a bit more than a sixteenth; the point lands on the grid.
    let _ = app.update(auto::Message::Begin { index, additive: false }.into());
    let _ = app.update(auto::Message::Drag { ticks: 250.0, value: 0.0, bypass: false }.into());
    let _ = app.update(auto::Message::End.into());
    assert!(points(&app).iter().any(|p| p.0 == 1200 && (p.1 - 0.5).abs() < 1e-6), "{:?}", points(&app));

    // Cmd (Ctrl on Linux) bypasses snapping.
    let index = points(&app).iter().position(|p| p.0 == 1200).unwrap();
    let _ = app.update(auto::Message::Begin { index, additive: false }.into());
    let _ = app.update(auto::Message::Drag { ticks: 7.0, value: 0.013, bypass: true }.into());
    let _ = app.update(auto::Message::End.into());
    assert!(points(&app).iter().any(|p| p.0 == 1207 && (p.1 - 0.513).abs() < 1e-4), "{:?}", points(&app));

    for _ in 0..3 {
        let _ = app.update(Message::Action(Action::Undo));
    }
    assert_eq!(points(&app), before);
    let _ = app.update(Message::Action(Action::Redo));
    assert!(points(&app).contains(&(960, 0.5, Shape::Linear)));
}

#[test]
fn automation_shapes_tension_keys_and_delete() {
    let mut app = app();
    let _ = app.update(Message::Automate(Target::Tempo));
    let _ = app.update(auto::Message::CycleShape(0).into());
    assert_eq!(points(&app)[0].2, Shape::Curve);
    let _ = app.update(auto::Message::BeginTension(0).into());
    let _ = app.update(auto::Message::Tension(0, 3.0).into());
    let id = app.automation.clip.unwrap();
    assert_eq!(app.project.automation_clip(id).unwrap().envelope.points[0].tension, 1.0);

    focus(&mut app, Panel::Automation);
    let _ = app.update(press(Code::KeyA, char_key("a"), Modifiers::COMMAND));
    assert_eq!(app.automation.selected.len(), 2);
    let _ = app.update(press(Code::Digit5, char_key("5"), Modifiers::empty()));
    assert!(points(&app).iter().all(|p| p.2 == Shape::Stairs(4)));

    // Duplicate the selection after itself.
    let _ = app.update(press(Code::KeyD, char_key("d"), Modifiers::COMMAND));
    assert_eq!(points(&app).len(), 4);

    let _ = app.update(press(Code::Delete, Key::Named(keyboard::key::Named::Delete), Modifiers::empty()));
    assert_eq!(points(&app).len(), 2);
}

#[test]
fn box_select_line_and_lfo_replace_ranges() {
    let mut app = app();
    let _ = app.update(Message::Automate(Target::Tempo));
    let length = app.project.automation_clip(app.automation.clip.unwrap()).unwrap().length;

    let line = vec![
        daw_model::automation::Point::new(960, 0.0),
        daw_model::automation::Point::new(1920, 1.0),
    ];
    let _ = app.update(auto::Message::Replace { start: 960, end: 1920, points: line }.into());
    assert_eq!(points(&app).len(), 4);

    let _ = app.update(auto::Message::BoxSelect { t0: 900.0, t1: 2000.0, v0: 0.0, v1: 1.0, additive: false }.into());
    assert_eq!(app.automation.selected.len(), 2);

    app.automation.lfo_rate = auto::LfoRate(4);
    let _ = app.update(auto::Message::Lfo.into());
    let pts = points(&app);
    assert!(pts.len() > 4, "{pts:?}");
    assert!(pts.iter().all(|p| p.1 >= -1e-6 && p.1 <= 1.0 + 1e-6));
    assert!(pts.windows(2).all(|w| w[0].0 <= w[1].0));
    assert_eq!(pts.last().unwrap().0, length);
}

#[test]
fn recording_overwrites_between_touches() {
    let mut app = app();
    let target = Target::Tempo;
    app.record = true;
    app.playing = true;
    app.position = 0.0;
    app.touched(target, 0.2);
    let id = app.project.automation_for(target).unwrap();
    for (position, value) in [(480.0, 0.4), (960.0, 0.6), (1440.0, 0.8)] {
        app.position = position;
        app.touched(target, value);
    }
    let pts: Vec<(u64, f32)> = app.project.automation_clip(id).unwrap().envelope.points.iter().map(|p| (p.time, p.value)).collect();
    assert!(pts.contains(&(480, 0.4)) && pts.contains(&(1440, 0.8)), "{pts:?}");

    // Second pass over the same range replaces what was there.
    auto::stopped(&mut app);
    app.position = 400.0;
    app.touched(target, 0.9);
    app.position = 1500.0;
    app.touched(target, 0.1);
    let pts: Vec<(u64, f32)> = app.project.automation_clip(id).unwrap().envelope.points.iter().map(|p| (p.time, p.value)).collect();
    assert!(!pts.iter().any(|p| p.0 > 400 && p.0 < 1500), "{pts:?}");
}

#[test]
fn piano_roll_add_move_resize_and_keys() {
    let mut app = app();
    app.project.grid = Grid::Division(16);
    let _ = app.update(roll::Message::Add { start: 240, key: 60 }.into());
    let _ = app.update(roll::Message::Drag { ticks: 250.0, keys: 2, bypass: false }.into());
    let _ = app.update(roll::Message::End.into());
    let notes = app.project.pattern(app.selected_pattern).unwrap().notes(app.selected_channel.unwrap()).to_vec();
    assert_eq!((notes[0].start, notes[0].key), (480, 62));

    let _ = app.update(roll::Message::Begin { index: 0, resize: true, additive: false }.into());
    let _ = app.update(roll::Message::Drag { ticks: 480.0, keys: 0, bypass: false }.into());
    let _ = app.update(roll::Message::End.into());
    let notes = app.project.pattern(app.selected_pattern).unwrap().notes(app.selected_channel.unwrap()).to_vec();
    assert_eq!(notes[0].length, 240 + 480);
    assert_eq!(app.piano_roll.length, 720);

    focus(&mut app, Panel::PianoRoll);
    let _ = app.update(press(Code::ArrowUp, Key::Named(keyboard::key::Named::ArrowUp), Modifiers::SHIFT));
    let notes = app.project.pattern(app.selected_pattern).unwrap().notes(app.selected_channel.unwrap()).to_vec();
    assert_eq!(notes[0].key, 74);
    let _ = app.update(roll::Message::Velocity(0, -0.3).into());
    let notes = app.project.pattern(app.selected_pattern).unwrap().notes(app.selected_channel.unwrap()).to_vec();
    assert!((notes[0].velocity - 0.5).abs() < 1e-6);
    let _ = app.update(roll::Message::Drag { ticks: 0.0, keys: 0, bypass: false }.into());
    if let Some(note) = app.project.pattern_mut(app.selected_pattern).unwrap().notes_mut(app.selected_channel.unwrap()).first_mut() {
        note.start = 500;
    }
    let _ = app.update(press(Code::KeyQ, char_key("q"), Modifiers::empty()));
    let notes = app.project.pattern(app.selected_pattern).unwrap().notes(app.selected_channel.unwrap()).to_vec();
    assert_eq!(notes[0].start, 480);
    let _ = app.update(press(Code::Backspace, Key::Named(keyboard::key::Named::Backspace), Modifiers::empty()));
    assert!(app.project.pattern(app.selected_pattern).unwrap().notes(app.selected_channel.unwrap()).is_empty());
}

#[test]
fn each_pattern_sets_its_own_length_and_can_shrink() {
    let mut app = app();
    let bar = app.project.signature.ticks_per_bar();
    let first = app.selected_pattern;
    let _ = app.update(Message::PatternBars(3));
    let _ = app.update(Message::NewPattern);
    let second = app.selected_pattern;
    assert_eq!(app.project.pattern(first).unwrap().length, bar * 3);
    assert_eq!(app.project.pattern(second).unwrap().length, bar);

    // Placing a note past the end grows the pattern; the ruler drag shrinks it
    // again as one undo step.
    let _ = app.update(roll::Message::Add { start: bar * 5, key: 60 }.into());
    let _ = app.update(roll::Message::End.into());
    assert_eq!(app.project.pattern(second).unwrap().length, bar * 6);
    let _ = app.update(roll::Message::PatternEnd(bar as f64 * 2.4).into());
    let _ = app.update(roll::Message::PatternEnd(bar as f64 * 1.8).into());
    let _ = app.update(Message::EndEdit);
    assert_eq!(app.project.pattern(second).unwrap().length, bar * 2);
    let _ = app.update(Message::Action(Action::Undo));
    assert_eq!(app.project.pattern(second).unwrap().length, bar * 6);

    // Editing a note inside a shortened pattern does not grow it back.
    let _ = app.update(roll::Message::PatternEnd(bar as f64 * 2.0).into());
    let _ = app.update(Message::EndEdit);
    let _ = app.update(roll::Message::Add { start: 0, key: 64 }.into());
    let _ = app.update(roll::Message::End.into());
    assert_eq!(app.project.pattern(second).unwrap().length, bar * 2);
}

#[test]
fn piano_roll_command_selects_and_duplicates_notes() {
    let mut app = app();
    let _ = app.update(roll::Message::Add { start: 0, key: 64 }.into());
    let _ = app.update(roll::Message::End.into());
    focus(&mut app, Panel::PianoRoll);
    app.piano_roll.selected.clear();
    let _ = app.update(press(Code::KeyA, char_key("a"), Modifiers::COMMAND));
    assert_eq!(app.piano_roll.selected, vec![0]);
    let _ = app.update(press(Code::KeyD, char_key("d"), Modifiers::COMMAND));
    let _ = app.update(press(Code::KeyA, char_key("a"), Modifiers::COMMAND));
    assert_eq!(app.piano_roll.selected.len(), 2);
}

#[test]
fn bpm_field_applies_only_on_submit_or_focus_loss() {
    let mut app = app();
    let before = app.project.bpm;
    let (revision, undo) = (app.revision, app.undo.len());
    for text in ["", "2", "25", "250"] {
        let _ = app.update(Message::SetBpm(text.into()));
        assert_eq!(app.bpm_text.as_deref(), Some(text));
        assert_eq!(app.project.bpm, before);
        assert_eq!((app.revision, app.undo.len(), app.dirty), (revision, undo, false));
    }
    let _ = app.update(Message::MousePressed);
    let _ = app.update(Message::BpmFocused(true));
    assert_eq!(app.project.bpm, before);
    let _ = app.update(Message::BpmFocused(false));
    assert_eq!(app.project.bpm, 250.0);
    assert_eq!(app.undo.len(), undo + 1);
    let _ = app.update(Message::Action(Action::Undo));
    let _ = app.update(Message::SetBpm("95.5".into()));
    let _ = app.update(Message::BpmDone);
    assert_eq!((app.project.bpm, app.bpm_text.clone()), (95.5, None));
    let _ = app.update(Message::Action(Action::Undo));
    assert_eq!(app.project.bpm, before);
    let (revision, undo) = (app.revision, app.undo.len());
    for text in ["", "NaN", "0.5", "1000"] {
        let _ = app.update(Message::SetBpm(text.into()));
        let _ = app.update(Message::BpmDone);
        assert_eq!(app.project.bpm, before);
        assert_eq!((app.revision, app.undo.len()), (revision, undo));
    }
    for bpm in [1.0, 999.0] {
        let _ = app.update(Message::SetBpm(bpm.to_string()));
        let _ = app.update(Message::BpmDone);
        assert_eq!(app.project.bpm, bpm);
    }
}

#[test]
fn mixer_routes_insert_outputs_without_loops() {
    let mut app = app();
    let [a, b] = [1, 2].map(|i| app.project.mixer.inserts[i].id);
    let _ = app.update(crate::panels::mixer::Message::Output(a, b).into());
    assert_eq!(app.project.mixer.output(a), Some(b));
    let _ = app.update(crate::panels::mixer::Message::Output(b, a).into());
    assert_eq!(app.project.mixer.output(b), Some(daw_model::MASTER));
    assert!(app.status.contains("back into itself"), "{}", app.status);
    let _ = app.update(Message::Action(Action::Undo));
    assert_eq!(app.project.mixer.output(a), Some(daw_model::MASTER));
}

#[test]
fn alt_scroll_zooms_key_and_track_height_around_the_cursor() {
    let mut app = app();
    let (top, height) = (app.piano_roll.top_key, app.piano_roll.key_height);
    let y = timeline::RULER_HEIGHT + height * 10.0; // ten keys below the top
    let _ = app.update(roll::Message::ZoomKeys { steps: 3.0, y }.into());
    assert!(app.piano_roll.key_height > height);
    let key_under = app.piano_roll.top_key as f32 - (y - timeline::RULER_HEIGHT) / app.piano_roll.key_height;
    assert!((key_under - (top as f32 - 10.0)).abs() <= 1.0, "{key_under}");
    let _ = app.update(roll::Message::ZoomKeys { steps: -100.0, y }.into());
    assert_eq!(app.piano_roll.key_height, 6.0);

    let height = app.playlist.track_height;
    let _ = app.update(list::Message::ZoomTracks { steps: 2.0, y: 100.0 }.into());
    assert!(app.playlist.track_height > height);
    let _ = app.update(list::Message::ZoomTracks { steps: 100.0, y: 100.0 }.into());
    assert_eq!(app.playlist.track_height, 120.0);
}

#[test]
fn automation_clips_size_like_patterns() {
    let mut app = app();
    let bar = app.project.signature.ticks_per_bar();
    let _ = app.update(Message::Automate(Target::Tempo));
    let id = app.automation.clip.unwrap();
    let length = |app: &App| app.project.automation_clip(id).unwrap().length;
    let _ = app.update(auto::Message::Bars(3).into());
    assert_eq!(length(&app), bar * 3);

    let _ = app.update(auto::Message::Add { time: bar as f64 * 4.5, value: 0.5, bypass: true }.into());
    let _ = app.update(auto::Message::End.into());
    assert_eq!(length(&app), bar * 5);
    let _ = app.update(auto::Message::ClipEnd(bar as f64 * 1.2).into());
    let _ = app.update(Message::EndEdit);
    assert_eq!(length(&app), bar);
    let _ = app.update(auto::Message::Add { time: 10.0, value: 0.5, bypass: true }.into());
    let _ = app.update(auto::Message::End.into());
    assert_eq!(length(&app), bar);
}

#[test]
fn playlist_place_move_duplicate_and_loop() {
    let mut app = app();
    let bar = app.project.signature.ticks_per_bar();
    let _ = app.update(list::Message::Add { start: 0, track: 2 }.into());
    let _ = app.update(list::Message::Drag { ticks: bar as f64 + 30.0, tracks: 1, bypass: false }.into());
    let _ = app.update(list::Message::End.into());
    let clip = app.project.playlist.clips[0].clone();
    assert_eq!((clip.start, clip.track), (bar, 3));

    focus(&mut app, Panel::Playlist);
    let _ = app.update(press(Code::KeyD, char_key("d"), Modifiers::COMMAND));
    assert_eq!(app.project.playlist.clips.len(), 2);
    assert_eq!(app.project.playlist.clips[1].start, bar + clip.length);

    let id = app.project.playlist.clips[0].id;
    let _ = app.update(list::Message::Split(id, bar as f64 + 500.0).into());
    let halves: Vec<_> = app.project.playlist.clips.iter().filter(|c| c.track == 3 && c.start < bar * 2).map(|c| (c.start, c.length, c.offset)).collect();
    assert!(halves.contains(&(bar, 480, 0)) && halves.contains(&(bar + 480, clip.length - 480, 480)), "{halves:?}");

    let _ = app.update(list::Message::Loop(Some((10.0, bar as f64 * 2.0 + 5.0))).into());
    assert_eq!(app.project.playlist.loop_range, Some((0, bar * 2)));
    let _ = app.update(list::Message::Loop(None).into());
    assert_eq!(app.project.playlist.loop_range, None);
}

#[test]
fn tiling_actions_through_keys() {
    let mut app = app();
    let tiles = app.layout().leaves().len();
    let alt = Modifiers::ALT;
    let _ = app.update(press(Code::KeyV, char_key("√"), alt));
    let _ = app.update(press(Code::Enter, Key::Named(keyboard::key::Named::Enter), alt));
    assert_eq!(app.layout().leaves().len(), tiles + 1);
    assert_eq!(app.focused_panel(), Panel::Mixer, "first panel not on screen");
    let _ = app.update(press(Code::KeyF, char_key("ƒ"), alt));
    assert!(app.layout().zoomed);
    let _ = app.update(press(Code::KeyF, char_key("ƒ"), alt));
    let _ = app.update(press(Code::KeyQ, char_key("œ"), alt));
    assert_eq!(app.layout().leaves().len(), tiles);
    let _ = app.update(press(Code::Digit3, char_key("£"), alt));
    assert_eq!(app.project.workspaces.active, 2);
    assert_eq!(app.split_axis, Axis::Vertical);
}

#[test]
fn save_open_and_export() {
    let dir = std::env::temp_dir().join(format!("daw-app-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = app();
    let channel = app.project.channels[0].id;
    let pattern = app.selected_pattern;
    let _ = app.update(crate::panels::channel_rack::Message::Step(channel, 0).into());
    let _ = app.update(list::Message::Add { start: 0, track: 0 }.into());
    let _ = app.update(list::Message::End.into());
    let _ = app.update(Message::Automate(Target::ChannelVolume(channel)));

    let path = dir.join("song.dawproj");
    let _ = app.update(Message::SavedAs(Some(path.clone())));
    wait_saves(&mut app);
    assert!(!app.dirty);
    let saved = app.project.clone();

    let mut other = self::app();
    other.open(path);
    assert_eq!(other.project.patterns, saved.patterns);
    assert_eq!(other.project.automation, saved.automation);
    assert_eq!(other.selected_pattern, pattern);

    let wav = dir.join("song.wav");
    let _ = other.update(Message::Exported(Some(wav.clone())));
    wait_renders(&mut other);
    let reader = hound::WavReader::open(&wav).unwrap();
    assert!(reader.duration() > 0, "{}", other.status);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn settings_capture_rebinds_and_saves() {
    let mut app = app();
    let _ = app.update(Message::Action(Action::Settings));
    assert_eq!(app.focused_panel(), Panel::Settings);

    let _ = app.update(crate::panels::settings::Message::Capture("zoom").into());
    // Modifier keys alone wait for the rest of the chord.
    let _ = app.update(press(Code::AltLeft, Key::Named(keyboard::key::Named::Alt), Modifiers::ALT));
    assert_eq!(app.settings.capturing, Some("zoom"));
    let _ = app.update(press(Code::KeyH, char_key("˙"), Modifiers::ALT));
    assert_eq!(app.settings.capturing, None);
    assert!(app.status.contains("moved from focus left"), "{}", app.status);

    // The new chord now zooms instead of moving focus.
    let _ = app.update(press(Code::KeyH, char_key("˙"), Modifiers::ALT));
    assert!(app.layout().zoomed);

    let saved = std::fs::read_to_string(&app.config_path).unwrap();
    let config: crate::config::Config = serde_json::from_str(&saved).unwrap();
    assert_eq!(config.keys["zoom"], vec!["alt+f", "alt+h"]);
    assert_eq!(config.keys["focus-left"], vec!["alt+left"]);

    // Escape cancels a capture without binding anything.
    let _ = app.update(crate::panels::settings::Message::Capture("undo").into());
    let _ = app.update(press(Code::Escape, Key::Named(keyboard::key::Named::Escape), Modifiers::empty()));
    assert_eq!(app.keymap.chords("undo").len(), 1);

    let _ = app.update(crate::panels::settings::Message::ResetAll.into());
    assert_eq!(app.keymap, crate::keys::Keymap::default());
    std::fs::remove_dir_all(app.config_path.parent().unwrap()).unwrap();
}

#[test]
fn delete_key_removes_the_selection_in_each_panel() {
    let mut app = app();
    let delete = || press(Code::Backspace, Key::Named(keyboard::key::Named::Backspace), Modifiers::empty());
    let forward_delete = || press(Code::Delete, Key::Named(keyboard::key::Named::Delete), Modifiers::empty());

    // Channel rack: the selected channel and its notes go.
    let _ = app.update(crate::panels::channel_rack::Message::AddSynth.into());
    let second = app.selected_channel.unwrap();
    focus(&mut app, Panel::ChannelRack);
    let _ = app.update(delete());
    assert!(app.project.channel(second).is_none());
    assert_eq!(app.project.channels.len(), 1);
    assert_eq!(app.selected_channel, Some(app.project.channels[0].id));

    // Mixer: the selected insert goes and its channel falls back to the master.
    let insert = app.project.channels[0].insert;
    app.selected_insert = insert;
    focus(&mut app, Panel::Mixer);
    let _ = app.update(forward_delete());
    assert!(app.project.mixer.insert(insert).is_none());
    assert_eq!(app.project.channels[0].insert, daw_model::MASTER);
    // The master stays.
    app.selected_insert = daw_model::MASTER;
    let _ = app.update(delete());
    assert!(app.project.mixer.insert(daw_model::MASTER).is_some());
    assert!(app.status.contains("nothing selected"), "{}", app.status);

    // Playlist: selected clips first, then the selected track.
    let tracks = app.project.playlist.tracks.len();
    let _ = app.update(list::Message::Add { start: 0, track: 1 }.into());
    let _ = app.update(list::Message::End.into());
    let _ = app.update(list::Message::Add { start: 0, track: 2 }.into());
    let _ = app.update(list::Message::End.into());
    focus(&mut app, Panel::Playlist);
    let _ = app.update(delete());
    assert_eq!(app.project.playlist.clips.len(), 1);
    let _ = app.update(list::Message::SelectTrack(0).into());
    let _ = app.update(delete());
    assert_eq!(app.project.playlist.tracks.len(), tracks - 1);
    assert_eq!(app.project.playlist.clips[0].track, 0);

    // Undo brings the track back.
    let _ = app.update(Message::Action(Action::Undo));
    assert_eq!(app.project.playlist.tracks.len(), tracks);

    // Delete is a normal binding and can be changed.
    assert_eq!(app.keymap.chords("delete").iter().map(|c| c.to_string()).collect::<Vec<_>>(), vec!["delete", "forwarddelete"]);
}

#[test]
fn play_starts_from_pattern_or_song_marker_and_pause_returns_there() {
    let mut app = app();
    assert_eq!(app.mode, PlayMode::Song, "projects start in song mode");
    let _ = app.update(Message::Action(Action::ToggleMode));
    assert!(matches!(app.mode, PlayMode::Pattern(_)));
    let _ = app.update(Message::Action(Action::PlayPause));
    app.position = 1234.0;
    let _ = app.update(Message::Action(Action::PlayPause));
    assert_eq!(app.position, 0.0);
    let _ = app.update(Message::Action(Action::PlayPause));
    assert_eq!(app.position, 0.0);
    let _ = app.update(Message::Action(Action::PlayPause));

    // Clicking the ruler switches to song mode and sets a snapped marker.
    app.project.grid = Grid::Beat;
    let _ = app.update(list::Message::Seek(2000.0).into());
    assert_eq!(app.mode, PlayMode::Song);
    assert_eq!((app.song_start, app.position), (1920.0, 1920.0));

    let _ = app.update(Message::Action(Action::PlayPause));
    assert!(app.playing);
    app.position = 5000.0; // the playhead moved while playing
    let _ = app.update(Message::Action(Action::PlayPause));
    assert!(!app.playing);
    assert_eq!((app.song_start, app.position), (1920.0, 1920.0));

    // Play again starts from the same marker.
    app.position = 7000.0;
    let _ = app.update(Message::Action(Action::PlayPause));
    assert_eq!(app.position, 1920.0);
    let _ = app.update(Message::Action(Action::PlayPause));

    // Escape stops and moves the marker back to the start.
    let _ = app.update(Message::Action(Action::Stop));
    assert_eq!((app.song_start, app.position), (0.0, 0.0));

    // The piano roll ruler does the same for the pattern and switches back.
    let _ = app.update(roll::Message::Seek(1000.0).into());
    assert_eq!(app.mode, PlayMode::Pattern(app.selected_pattern));
    assert_eq!((app.pattern_start, app.position), (960.0, 960.0));
    let _ = app.update(Message::Action(Action::PlayPause));
    app.position = 1500.0;
    let _ = app.update(Message::Action(Action::PlayPause));
    assert_eq!((app.pattern_start, app.position), (960.0, 960.0));

    // A marker past a shortened pattern's end plays from the top.
    let _ = app.update(roll::Message::Seek(bar_ticks(&app) * 1.5).into());
    let _ = app.update(Message::PatternBars(1));
    let _ = app.update(Message::Action(Action::PlayPause));
    assert_eq!(app.position, 0.0);
    let _ = app.update(Message::Action(Action::PlayPause));
    let _ = app.update(Message::Action(Action::Stop));
    assert_eq!(app.pattern_start, 0.0);
}

fn bar_ticks(app: &App) -> f64 {
    app.project.signature.ticks_per_bar() as f64
}

#[test]
fn new_and_open_ask_to_save_unsaved_changes_first() {
    let dir = std::env::temp_dir().join(format!("daw-replace-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let other = dir.join("other.dawproj");
    std::fs::write(&other, Project::new().to_ron().unwrap()).unwrap();

    // Clean project: New and Open act at once.
    let mut app = app();
    let _ = app.update(Message::Opened(Some(other.clone())));
    assert_eq!(app.path.as_deref(), Some(other.as_path()));
    let _ = app.update(Message::Action(Action::New));
    assert!(app.path.is_none() && app.pending.is_none());

    // Unsaved changes: Cancel keeps the project, Don't Save goes ahead.
    let _ = app.update(Message::NewPattern);
    let _ = app.update(Message::Opened(Some(other.clone())));
    assert_eq!(app.pending, Some(Pending::Open(other.clone())));
    let _ = app.update(Message::SaveChoice(SaveChoice::Cancel));
    assert!(app.pending.is_none() && app.dirty && app.path.is_none());
    let _ = app.update(Message::Action(Action::New));
    assert_eq!(app.pending, Some(Pending::New));
    let _ = app.update(Message::SaveChoice(SaveChoice::Discard));
    assert!(!app.dirty && app.project.patterns.len() == 1);

    // Save writes the current project, then opens the other one.
    let saved = dir.join("saved.dawproj");
    app.path = Some(saved.clone());
    let _ = app.update(Message::NewPattern);
    let _ = app.update(Message::Opened(Some(other.clone())));
    let _ = app.update(Message::SaveChoice(SaveChoice::Save));
    wait_saves(&mut app);
    assert_eq!(crate::project_files::load(&saved).unwrap().patterns.len(), 2);
    assert_eq!(app.path.as_deref(), Some(other.as_path()));
    assert!(!app.dirty && app.pending.is_none());
}

#[test]
fn closing_prompts_only_with_unsaved_changes_and_saves_before_quitting() {
    let dir = std::env::temp_dir().join(format!("daw-close-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    // Clean project: no prompt.
    let mut app = app();
    let _ = app.update(Message::CloseRequested);
    assert!(app.pending.is_none());

    // Unsaved changes: the prompt opens once, and Cancel keeps everything.
    let _ = app.update(Message::NewPattern);
    assert!(app.dirty);
    let _ = app.update(Message::CloseRequested);
    assert_eq!(app.pending, Some(Pending::Close));
    let _ = app.update(Message::SaveChoice(SaveChoice::Cancel));
    assert!(app.pending.is_none() && app.dirty);

    // Save with a known path writes the file before quitting.
    let path = dir.join("close.dawproj");
    app.path = Some(path.clone());
    let _ = app.update(Message::CloseRequested);
    let _ = app.update(Message::SaveChoice(SaveChoice::Save));
    wait_saves(&mut app);
    assert!(!app.dirty);
    assert_eq!(crate::project_files::load(&path).unwrap().patterns.len(), 2);

    // A failed save keeps the app open with the changes.
    let _ = app.update(Message::NewPattern);
    app.path = Some(dir.join("missing-dir").join("x.dawproj"));
    let _ = app.update(Message::CloseRequested);
    let _ = app.update(Message::SaveChoice(SaveChoice::Save));
    wait_saves(&mut app);
    assert!(app.dirty && app.pending.is_none());
    assert!(app.status.contains("save failed"), "{}", app.status);

    // Untitled project: cancelling the save dialog keeps the app open.
    app.path = None;
    let _ = app.update(Message::CloseRequested);
    let _ = app.update(Message::SavedAsThenContinue(None));
    assert!(app.dirty && app.pending.is_none());

    // Plugin parameter edits count as unsaved changes.
    let mut clean = self::app();
    clean.touched(Target::Tempo, 0.5);
    assert!(clean.dirty);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn right_click_menu_renames_and_deletes() {
    use crate::menu::{Item, Message as Menu};
    let mut app = app();
    let rename = |app: &mut App, item: Item, name: &str| {
        let _ = app.update(Menu::Open(item).into());
        let _ = app.update(Menu::Rename.into());
        let _ = app.update(Menu::Input(name.into()).into());
        let _ = app.update(Menu::Close.into());
    };

    let channel = app.project.channels[0].id;
    rename(&mut app, Item::Channel(channel), "  bass ");
    assert_eq!(app.project.channel(channel).unwrap().name, "bass");
    assert!(app.menu.is_none());

    let pattern = app.project.patterns[0].id;
    rename(&mut app, Item::Pattern(pattern), "verse");
    assert_eq!(app.project.pattern(pattern).unwrap().name, "verse");

    let insert = app.project.mixer.inserts[2].id;
    rename(&mut app, Item::Insert(insert), "drums bus");
    assert_eq!(app.project.mixer.insert(insert).unwrap().name, "drums bus");
    assert_eq!(app.selected_insert, insert, "right click selects the insert");

    rename(&mut app, Item::Track(3), "lead");
    assert_eq!(app.project.playlist.tracks[3].name, "lead");

    // Blank names and Escape leave the name alone.
    rename(&mut app, Item::Track(3), "   ");
    assert_eq!(app.project.playlist.tracks[3].name, "lead");
    let _ = app.update(Menu::Open(Item::Track(3)).into());
    let _ = app.update(Menu::Rename.into());
    let _ = app.update(Menu::Input("nope".into()).into());
    let _ = app.update(press(Code::Escape, Key::Named(keyboard::key::Named::Escape), Modifiers::empty()));
    assert!(app.menu.is_none());
    assert_eq!(app.project.playlist.tracks[3].name, "lead");

    // Each rename is one undo step.
    let _ = app.update(Message::Action(Action::Undo));
    assert_eq!(app.project.playlist.tracks[3].name, "track 4");

    // Menu entries act on the item that was right-clicked.
    let tracks = app.project.playlist.tracks.len();
    let _ = app.update(Menu::Open(Item::Track(5)).into());
    let _ = app.update(Menu::Choose(Box::new(list::Message::DeleteTrack.into())).into());
    assert_eq!(app.project.playlist.tracks.len(), tracks - 1);
    assert!(app.menu.is_none());
}

/// Write a mono WAV of `frames` frames at 48 kHz.
fn write_wav(path: &std::path::Path, frames: usize) {
    let spec = hound::WavSpec { channels: 1, sample_rate: 48_000, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for i in 0..frames {
        writer.write_sample((i as f32 * 0.01).sin() * 0.5).unwrap();
    }
    writer.finalize().unwrap();
}

#[test]
fn recorded_takes_become_clips_at_their_start_and_undo() {
    let dir = std::env::temp_dir().join(format!("daw-take-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = app();
    app.path = Some(dir.join("song.dawproj"));
    app.project.bpm = 120.0;
    let channels = app.project.channels.len();
    // One second of stereo input.
    let take = daw_engine::input::Take { start: 1920.0, channels: 2, sample_rate: 48_000, samples: vec![0.25; 96_000] };
    let path = app.place_take(take).unwrap().expect("a take");
    assert_eq!(path, dir.join("recordings").join("take 1.wav"));

    let reader = hound::WavReader::open(&path).unwrap();
    assert_eq!((reader.spec().channels, reader.duration()), (2, 48_000));
    let channel = app.project.channels.last().unwrap();
    assert_eq!(channel.source, daw_model::Source::Audio { path: path.to_string_lossy().into_owned() });
    let clip = app.project.playlist.clips.last().unwrap();
    assert_eq!((clip.source, clip.start, clip.length, clip.offset), (ClipSource::Audio(channel.id), 1920, 1920, 0));

    let _ = app.update(Message::Action(Action::Undo));
    assert_eq!(app.project.channels.len(), channels);
    assert!(app.project.playlist.clips.is_empty());

    let empty = daw_engine::input::Take { start: 0.0, channels: 1, sample_rate: 48_000, samples: Vec::new() };
    assert_eq!(app.place_take(empty), Ok(None));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn dropped_audio_places_a_clip_that_trims_from_both_edges() {
    let dir = std::env::temp_dir().join(format!("daw-import-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let wav = dir.join("vocal.wav");
    write_wav(&wav, 48_000);
    let mut app = app();
    app.project.bpm = 120.0;
    let _ = app.update(Message::Dropped(wav.clone()));
    let channel = app.project.channels.last().unwrap().clone();
    assert_eq!(channel.name, "vocal");
    let clip = app.project.playlist.clips[0].clone();
    // One second at 120 bpm is two beats.
    assert_eq!((clip.start, clip.length, clip.source), (0, 1920, ClipSource::Audio(channel.id)));

    let drag = |app: &mut App, edge, ticks| {
        let _ = app.update(list::Message::Begin { id: clip.id, edge, additive: false }.into());
        let _ = app.update(list::Message::Drag { ticks, tracks: 0, bypass: true }.into());
        let _ = app.update(list::Message::End.into());
        let clip = &app.project.playlist.clips[0];
        (clip.start, clip.length, clip.offset)
    };
    assert_eq!(drag(&mut app, Some(list::Edge::Start), 480.0), (480, 1440, 480));
    assert_eq!(drag(&mut app, Some(list::Edge::End), 5000.0), (480, 1440, 480), "stops at the file end");
    assert_eq!(drag(&mut app, Some(list::Edge::Start), -2000.0), (0, 1920, 0), "stops at the file start");
    assert_eq!(drag(&mut app, None, 960.0), (960, 1920, 0));

    // The brush is now the audio channel, so a click places the whole file.
    let _ = app.update(list::Message::Add { start: 7680, track: 0 }.into());
    let _ = app.update(list::Message::End.into());
    let added = app.project.playlist.clips.last().unwrap();
    assert_eq!((added.start, added.length, added.source), (7680, 1920, ClipSource::Audio(channel.id)));
    std::fs::remove_dir_all(&dir).unwrap();
}

fn midi_packet(app: &App, tick: f64, velocity: f32) -> crate::midi::Packet {
    let channel = app.project.channels[0].id;
    crate::midi::Packet { midi_channel: 0, channel, node: channel.0, key: 64, velocity, tick, elapsed: tick, recording: true }
}

#[test]
fn midi_recording_splits_looped_notes_and_keeps_original_channel() {
    let mut app = app();
    let pattern = app.selected_pattern;
    app.mode = PlayMode::Pattern(pattern);
    app.midi.recording = true;
    let bar = app.project.signature.ticks_per_bar();
    app.midi_packet(midi_packet(&app, (bar - 240) as f64, 0.7));
    app.selected_channel = Some(app.project.add_channel("other", daw_model::Source::Synth(Default::default())));
    let mut off = midi_packet(&app, 120.0, 0.0);
    off.elapsed = (bar + 120) as f64;
    app.midi_packet(off);
    let notes = app.project.pattern(pattern).unwrap().notes(app.project.channels[0].id);
    assert_eq!(notes.len(), 2);
    assert_eq!((notes[0].start, notes[0].length, notes[0].key, notes[0].velocity), (0, 120, 64, 0.7));
    assert_eq!((notes[1].start, notes[1].length), (bar - 240, 240));
    let _ = app.update(Message::Action(Action::Undo));
    assert!(app.project.pattern(pattern).unwrap().notes(app.project.channels[0].id).is_empty());
}

#[test]
fn song_midi_recording_creates_and_grows_one_take() {
    let mut app = app();
    app.mode = PlayMode::Song;
    app.song_start = 960.0;
    app.midi.recording = true;
    let count = app.project.patterns.len();
    app.midi_packet(midi_packet(&app, 1020.0, 1.0));
    app.midi_packet(midi_packet(&app, 1500.0, 0.0));
    app.midi_packet(midi_packet(&app, 6000.0, 0.5));
    app.midi_packet(midi_packet(&app, 10000.0, 0.0));
    assert_eq!(app.project.patterns.len(), count + 1);
    let pattern = app.project.patterns.last().unwrap();
    let notes = pattern.notes(app.project.channels[0].id);
    assert_eq!((notes[0].start, notes[0].length), (60, 480));
    assert_eq!((notes[1].start, notes[1].length), (5040, 4000));
    let clip = app.project.playlist.clips.last().unwrap();
    assert_eq!(clip.start, 960);
    assert_eq!(clip.source, ClipSource::Pattern(pattern.id));
    assert_eq!(clip.length, pattern.length);
    assert!(pattern.length >= notes[1].end());
}

#[test]
fn audio_pitch_prepares_only_the_released_value_and_undoes_once() {
    let dir = std::env::temp_dir().join(format!("daw-pitch-drag-{}", crate::project_files::stamp()));
    std::fs::create_dir(&dir).unwrap();
    let wav = dir.join("clip.wav");
    write_wav(&wav, 48_000);
    let mut app = app();
    app.import_audio(&wav, 0).unwrap();
    let first = app.project.playlist.clips[0].id;
    let ClipSource::Audio(channel) = app.project.playlist.clips[0].source else { panic!("audio clip") };
    let second = app.project.add_audio_clip(0, 1920, channel, 1920);
    let third = app.project.add_audio_clip(0, 3840, channel, 1920);
    app.playlist.selected = vec![first, second];
    app.dirty = false;
    let (revision, undo) = (app.revision, app.undo.len());
    let daw_model::Source::Audio { path } = &app.project.channel(channel).unwrap().source else { panic!("audio source") };
    let path = path.clone();
    let shifted = |pitch| daw_engine::audio::cache_key(&path, daw_model::AudioEdit { semitones: pitch, ..Default::default() });
    for pitch in [1.0, 4.0, 7.0] {
        let _ = app.update(list::Message::AudioPitch(pitch).into());
        assert_eq!(app.playlist.pitch_edit, Some((vec![first, second], pitch)));
        assert!(app.project.playlist.clips.iter().all(|c| c.audio.semitones == 0.0));
        assert!(app.session.waveform(&shifted(pitch)).is_none());
        assert_eq!((app.revision, app.undo.len(), app.dirty), (revision, undo, false));
    }
    // An unrelated refresh during the drag must still use the committed pitch.
    app.refresh();
    assert!(app.session.waveform(&shifted(7.0)).is_none());
    app.playlist.selected = vec![third];
    let _ = app.update(list::Message::AudioPitchDone.into());
    assert!(app.playlist.pitch_edit.is_none());
    assert_eq!(app.project.playlist.clips.iter().map(|c| c.audio.semitones).collect::<Vec<_>>(), vec![7.0, 7.0, 0.0]);
    assert!(app.session.audio_progress().is_some());
    wait_audio(&mut app);
    assert!(app.session.waveform(&shifted(7.0)).is_some());
    assert!(app.session.waveform(&shifted(1.0)).is_none());
    assert!(app.session.waveform(&shifted(4.0)).is_none());
    assert_eq!((app.revision, app.undo.len(), app.dirty), (revision + 1, undo + 1, true));

    app.playlist.selected = vec![first, second];
    let _ = app.update(list::Message::AudioPitch(2.0).into());
    let _ = app.update(list::Message::AudioPitch(7.0).into());
    let _ = app.update(list::Message::AudioPitchDone.into());
    let _ = app.update(list::Message::AudioPitchDone.into());
    assert_eq!((app.revision, app.undo.len()), (revision + 1, undo + 1));

    let _ = app.update(list::Message::AudioPitch(12.0).into());
    let _ = app.update(Message::Action(Action::Undo));
    assert!(app.playlist.pitch_edit.is_none());
    assert!(app.project.playlist.clips.iter().all(|c| c.audio.semitones == 0.0));
    let _ = app.update(list::Message::AudioPitchDone.into());
    assert_eq!((app.revision, app.undo.len()), (revision + 2, undo));
    let _ = app.update(list::Message::AudioPitch(2.0).into());
    let _ = app.update(Message::Key(keyboard::Event::KeyReleased {
        key: Key::Named(keyboard::key::Named::ArrowUp),
        modified_key: Key::Named(keyboard::key::Named::ArrowUp),
        physical_key: Physical::Code(Code::ArrowUp),
        location: Location::Standard,
        modifiers: Modifiers::default(),
    }, true));
    assert!(app.playlist.pitch_edit.is_none());
    assert_eq!(app.project.playlist.clips[0].audio.semitones, 2.0);
    wait_audio(&mut app);
    assert!(app.session.waveform(&shifted(2.0)).is_some());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn selecting_notes_and_clips_preserves_dirty_state_undo_and_redo() {
    let mut app = app();
    let _ = app.update(roll::Message::Add { start: 0, key: 60 }.into());
    let _ = app.update(roll::Message::End.into());
    let clip = app.project.add_clip(0, 0, ClipSource::Pattern(app.selected_pattern));
    let _ = app.update(Message::NewPattern);
    let _ = app.update(Message::Action(Action::Undo));
    app.dirty = false;
    let before = app.project.clone();
    let (revision, undo, redo, updates) = (app.revision, app.undo.len(), app.redo.len(), app.session.song_updates);
    for additive in [false, true, true] {
        let _ = app.update(roll::Message::Begin { index: 0, resize: false, additive }.into());
        let _ = app.update(roll::Message::Drag { ticks: 1.0, keys: 0, bypass: false }.into());
        let _ = app.update(roll::Message::End.into());
        let _ = app.update(list::Message::Begin { id: clip, edge: None, additive }.into());
        let _ = app.update(list::Message::Drag { ticks: 1.0, tracks: 0, bypass: false }.into());
        let _ = app.update(list::Message::End.into());
    }
    assert_eq!(app.project, before);
    assert!(!app.dirty);
    assert_eq!((app.revision, app.undo.len(), app.redo.len(), app.session.song_updates), (revision, undo, redo, updates));
    let _ = app.update(list::Message::Begin { id: clip, edge: None, additive: false }.into());
    let _ = app.update(list::Message::Drag { ticks: 960.0, tracks: 0, bypass: false }.into());
    let _ = app.update(list::Message::Drag { ticks: 0.0, tracks: 0, bypass: false }.into());
    let _ = app.update(list::Message::End.into());
    assert_eq!((app.revision, app.undo.len(), app.redo.len()), (revision, undo, redo));

    let _ = app.update(roll::Message::Begin { index: 0, resize: false, additive: false }.into());
    let _ = app.update(roll::Message::Drag { ticks: 240.0, keys: 1, bypass: true }.into());
    let _ = app.update(roll::Message::Drag { ticks: 480.0, keys: 2, bypass: true }.into());
    let _ = app.update(roll::Message::End.into());
    assert_eq!(app.undo.len(), undo + 1);
    let _ = app.update(Message::Action(Action::Undo));
    assert_eq!(app.project.patterns, before.patterns);
}

#[test]
fn selecting_automation_points_and_handles_preserves_undo_and_dirty_state() {
    let mut app = app();
    let channel = app.selected_channel.unwrap();
    let _ = app.update(Message::Automate(Target::ChannelVolume(channel)));
    let _ = app.update(auto::Message::Add { time: 240.0, value: 0.25, bypass: true }.into());
    let _ = app.update(auto::Message::End.into());
    let _ = app.update(Message::NewPattern);
    let _ = app.update(Message::Action(Action::Undo));
    app.dirty = false;
    let before = app.project.clone();
    let (undo, redo, revision, updates) = (app.undo.len(), app.redo.len(), app.revision, app.session.song_updates);
    for additive in [false, true, true] {
        let _ = app.update(auto::Message::Begin { index: 0, additive }.into());
        let _ = app.update(auto::Message::Drag { ticks: 0.0, value: 0.0, bypass: false }.into());
        let _ = app.update(auto::Message::End.into());
    }
    let _ = app.update(auto::Message::BeginTension(0).into());
    let _ = app.update(auto::Message::Tension(0, 0.0).into());
    let _ = app.update(auto::Message::End.into());
    assert_eq!(app.project, before);
    assert!(!app.dirty);
    assert_eq!((app.undo.len(), app.redo.len(), app.revision, app.session.song_updates), (undo, redo, revision, updates));
    let _ = app.update(auto::Message::Begin { index: 0, additive: false }.into());
    for ticks in [240.0, 480.0] { let _ = app.update(auto::Message::Drag { ticks, value: 0.1, bypass: true }.into()); }
    let _ = app.update(auto::Message::End.into());
    assert_eq!(app.undo.len(), undo + 1);
    let _ = app.update(Message::Action(Action::Undo));
    let _ = app.update(auto::Message::End.into());
    assert_eq!(app.project, before);
    let _ = app.update(auto::Message::BeginTension(0).into());
    for tension in [0.2, 0.4] { let _ = app.update(auto::Message::Tension(0, tension).into()); }
    let _ = app.update(auto::Message::End.into());
    assert_eq!(app.undo.len(), undo + 1);
    let _ = app.update(Message::Action(Action::Undo));
    assert_eq!(app.project, before);
}

#[test]
fn keyboard_note_nudges_create_one_undo_step_and_skip_clamped_moves() {
    let mut app = app();
    focus(&mut app, Panel::PianoRoll);
    let _ = app.update(roll::Message::Add { start: 0, key: 60 }.into());
    let _ = app.update(roll::Message::End.into());
    app.dirty = false;
    let (undo, revision, updates) = (app.undo.len(), app.revision, app.session.song_updates);
    let _ = app.update(press(Code::ArrowLeft, Key::Named(keyboard::key::Named::ArrowLeft), Modifiers::empty()));
    assert!(!app.dirty);
    assert_eq!((app.undo.len(), app.revision, app.session.song_updates), (undo, revision, updates));
    let _ = app.update(press(Code::ArrowRight, Key::Named(keyboard::key::Named::ArrowRight), Modifiers::empty()));
    assert_eq!(app.undo.len(), undo + 1);
    let _ = app.update(Message::Action(Action::Undo));
    let channel = app.selected_channel.unwrap();
    assert_eq!(app.project.pattern(app.selected_pattern).unwrap().notes(channel)[0].start, 0);
}

#[test]
fn live_controls_keep_one_undo_step_without_recompiling_the_song() {
    use crate::panels::{channel_rack as rack, mixer, parameters};
    let mut app = app();
    let channel = app.project.channels[0].id;
    let [from, to] = [1, 2].map(|i| app.project.mixer.inserts[i].id);
    let _ = app.update(mixer::Message::Send(from, to, false).into());
    let updates = app.session.song_updates;
    let undo = app.undo.len();
    for value in [0.2, 0.4, 0.7] { let _ = app.update(mixer::Message::Volume(from, value).into()); }
    let _ = app.update(Message::EndEdit);
    for value in [-0.5, 0.0, 0.5] { let _ = app.update(mixer::Message::Pan(from, value).into()); }
    let _ = app.update(Message::EndEdit);
    for value in [0.2, 0.4, 0.7] { let _ = app.update(mixer::Message::SendLevel(from, to, value).into()); }
    let _ = app.update(Message::EndEdit);
    for value in [0.2, 0.4, 0.7] { let _ = app.update(rack::Message::Volume(channel, value).into()); }
    let _ = app.update(Message::EndEdit);
    let daw_model::Source::Synth(mut synth) = app.project.channel(channel).unwrap().source else { panic!("synth") };
    for cutoff in [0.2, 0.4, 0.6] {
        synth.cutoff = cutoff;
        let _ = app.update(parameters::Message::Synth(channel, synth).into());
    }
    let _ = app.update(Message::EndEdit);
    assert_eq!(app.session.song_updates, updates);
    assert_eq!(app.undo.len(), undo + 5);
    assert_eq!(app.project.mixer.insert(from).unwrap().sends[0].level, 0.7);
    assert_eq!(app.project.channel(channel).unwrap().volume, 0.7);
    let _ = app.update(Message::Action(Action::Undo));
    assert_ne!(app.project.channel(channel).unwrap().source, daw_model::Source::Synth(synth));
    assert_eq!(app.project.channel(channel).unwrap().volume, 0.7);
}

#[test]
fn audio_stretch_drag_prepares_only_on_release_and_cancels_on_undo() {
    let dir = std::env::temp_dir().join(format!("daw-stretch-preview-{}", crate::project_files::stamp()));
    std::fs::create_dir(&dir).unwrap();
    let wav = dir.join("clip.wav");
    write_wav(&wav, 48_000);
    let mut app = app();
    app.project.bpm = 120.0;
    app.import_audio(&wav, 0).unwrap();
    let clip = app.project.playlist.clips[0].clone();
    let daw_model::Source::Audio { path } = &app.project.channels.last().unwrap().source else { panic!("audio") };
    let path = path.clone();
    app.playlist.stretch_mode = true;
    app.dirty = false;
    let (revision, undo, updates) = (app.revision, app.undo.len(), app.session.song_updates);
    let _ = app.update(list::Message::Begin { id: clip.id, edge: Some(list::Edge::End), additive: false }.into());
    for ticks in [480.0, 960.0, 1920.0] {
        let _ = app.update(list::Message::Drag { ticks, tracks: 0, bypass: true }.into());
        assert_eq!(app.project.playlist.clips[0], clip);
        let preview = &app.playlist.drag_preview[0];
        assert_eq!(preview.length, clip.length + ticks as u64);
        assert!(app.session.waveform(&daw_engine::audio::cache_key(&path, preview.audio)).is_none());
        assert_eq!((app.revision, app.undo.len(), app.session.song_updates), (revision, undo, updates));
        assert!(!app.dirty);
    }
    app.refresh();
    let _ = app.update(list::Message::End.into());
    wait_audio(&mut app);
    let stretched = &app.project.playlist.clips[0];
    assert_eq!((stretched.length, stretched.audio.stretch), (3840, 2.0));
    assert!(app.session.waveform(&daw_engine::audio::cache_key(&path, stretched.audio)).is_some());
    assert_eq!((app.revision, app.undo.len(), app.session.song_updates), (revision + 1, undo + 1, updates + 2));
    let _ = app.update(list::Message::Begin { id: clip.id, edge: Some(list::Edge::Start), additive: false }.into());
    let _ = app.update(list::Message::Drag { ticks: 960.0, tracks: 0, bypass: true }.into());
    let _ = app.update(Message::Action(Action::Undo));
    let _ = app.update(list::Message::End.into());
    assert_eq!(app.project.playlist.clips[0], clip);
    assert!(app.playlist.drag_preview.is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn audio_edits_scale_trim_offsets_and_undo_together() {
    let dir = std::env::temp_dir().join(format!("daw-edits-{}", crate::project_files::stamp()));
    std::fs::create_dir(&dir).unwrap();
    let wav = dir.join("clip.wav");
    write_wav(&wav, 4800);
    let mut app = app();
    app.import_audio(&wav, 960).unwrap();
    let clip = app.project.playlist.clips[0].id;
    app.project.playlist.clips[0].offset = 20;
    app.project.playlist.clips[0].length = 100;
    app.playlist.selected = vec![clip];
    let _ = app.update(list::Message::AudioStretch(2.0).into());
    let stretched = &app.project.playlist.clips[0];
    assert_eq!((stretched.start, stretched.length, stretched.offset, stretched.audio.stretch), (960, 200, 40, 2.0));
    let _ = app.update(list::Message::Reverse.into());
    assert!(app.project.playlist.clips[0].audio.reverse);
    let _ = app.update(list::Message::AudioPitch(12.0).into());
    let _ = app.update(list::Message::AudioPitchDone.into());
    for _ in 0..3 { let _ = app.update(Message::Action(Action::Undo)); }
    let restored = &app.project.playlist.clips[0];
    assert_eq!((restored.length, restored.offset, restored.audio), (100, 20, daw_model::AudioEdit::default()));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn stretch_mode_resizes_audio_and_split_keeps_the_edits() {
    let dir = std::env::temp_dir().join(format!("daw-stretch-drag-{}", crate::project_files::stamp()));
    std::fs::create_dir(&dir).unwrap();
    let wav = dir.join("clip.wav");
    write_wav(&wav, 48_000);
    let mut app = app();
    app.project.bpm = 120.0;
    app.import_audio(&wav, 0).unwrap();
    let id = app.project.playlist.clips[0].id;
    let _ = app.update(list::Message::StretchMode.into());
    let _ = app.update(list::Message::Begin { id, edge: Some(list::Edge::End), additive: false }.into());
    let _ = app.update(list::Message::Drag { ticks: 1920.0, tracks: 0, bypass: true }.into());
    let _ = app.update(list::Message::End.into());
    assert_eq!((app.project.playlist.clips[0].length, app.project.playlist.clips[0].audio.stretch), (3840, 2.0));
    let _ = app.update(list::Message::Reverse.into());
    let _ = app.update(list::Message::AudioPitch(7.0).into());
    let _ = app.update(list::Message::AudioPitchDone.into());
    let _ = app.update(list::Message::Split(id, 1920.0).into());
    let [left, right] = &app.project.playlist.clips[..] else { panic!("split clips") };
    assert_eq!(left.audio, right.audio);
    assert_eq!((left.length, right.offset, right.length), (1920, 1920, 1920));
    let _ = app.update(Message::Action(Action::Undo));
    assert_eq!(app.project.playlist.clips.len(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cloning_and_making_unique_are_independent_undo_steps() {
    let mut app = app();
    focus(&mut app, Panel::Playlist);
    let pattern = app.selected_pattern;
    let channel = app.project.channels[0].id;
    app.project.pattern_mut(pattern).unwrap().toggle_step(channel, 0);
    let first = app.project.add_clip(0, 0, ClipSource::Pattern(pattern));
    app.project.add_clip(1, 3840, ClipSource::Pattern(pattern));
    app.playlist.selected = vec![first];
    let _ = app.update(press(Code::KeyU, char_key("u"), Modifiers::COMMAND));
    let ClipSource::Pattern(unique) = app.project.playlist.clips[0].source else { panic!("unique pattern") };
    assert_ne!(unique, pattern);
    assert_eq!(app.project.playlist.clips[1].source, ClipSource::Pattern(pattern));
    let _ = app.update(Message::ClonePattern(unique));
    let clone = app.selected_pattern;
    assert_ne!(clone, unique);
    app.project.pattern_mut(clone).unwrap().notes_mut(channel)[0].key = 72;
    assert_eq!(app.project.pattern(unique).unwrap().notes(channel)[0].key, 60);
    let _ = app.update(Message::Action(Action::Undo));
    assert_eq!(app.project.patterns.len(), 2);
    let _ = app.update(Message::Action(Action::Undo));
    assert_eq!(app.project.patterns.len(), 1);
    assert_eq!(app.project.playlist.clips[0].source, ClipSource::Pattern(pattern));
}

#[test]
fn consolidation_bakes_insert_gain_and_leaves_master_processing_live() {
    let dir = std::env::temp_dir().join(format!("daw-consolidate-{}", crate::project_files::stamp()));
    std::fs::create_dir(&dir).unwrap();
    let mut app = app();
    app.path = Some(dir.join("song.dawproj"));
    app.project.bpm = 120.0;
    let channel = app.project.channels[0].id;
    let pattern = app.selected_pattern;
    app.project.pattern_mut(pattern).unwrap().toggle_step(channel, 0);
    let source = app.project.add_clip(0, 960, ClipSource::Pattern(pattern));
    app.playlist.selected = vec![source];
    app.project.mixer.inserts[0].volume = 0.5;
    app.project.mixer.inserts[1].volume = 0.4;
    let master_volume = app.project.add_automation("master volume", Target::InsertVolume(daw_model::MASTER), 0.3);
    app.project.add_clip(1, 0, ClipSource::Automation(master_volume));
    app.refresh();
    let end = app.project.playlist.clips[0].end();
    let before = dir.join("before.wav");
    app.session.export(&app.project, &before, crate::render::RenderOptions { depth: BitDepth::Float32, range: Some((960, end)), tail_seconds: 0.0 }, &[]).unwrap();
    app.consolidate_selection().unwrap();
    wait_renders(&mut app);
    assert!(app.project.playlist.clips[0].muted);
    let audio = app.project.channels.last().unwrap();
    assert_eq!((audio.volume, audio.pan, audio.insert), (1.0, 0.0, daw_model::MASTER));
    assert_eq!(app.project.mixer.inserts[0].volume, 0.5);
    let after = dir.join("after.wav");
    app.session.export(&app.project, &after, crate::render::RenderOptions { depth: BitDepth::Float32, range: Some((960, end)), tail_seconds: 0.0 }, &[]).unwrap();
    let before = daw_engine::synth::Sample::load(before.to_str().unwrap()).unwrap();
    let after = daw_engine::synth::Sample::load(after.to_str().unwrap()).unwrap();
    assert_eq!(before.left.len(), after.left.len());
    assert!(before.left.iter().any(|s| s.abs() > 0.001));
    let error = before.left[128..before.left.len() - 128].iter().zip(&after.left[128..after.left.len() - 128])
        .map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(error < 0.0001, "consolidated output error: {error}");
    let _ = app.update(Message::Action(Action::Undo));
    assert!(!app.project.playlist.clips[0].muted);
    assert_eq!(app.project.playlist.clips.len(), 2);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn plugin_only_edits_trigger_autosave_with_current_state_during_playback() {
    let mut app = app();
    app.backup_key = format!("plugin-test-{}", crate::project_files::stamp());
    let folder = crate::project_files::backups_dir().join(&app.backup_key);
    let instance = app.project.add_plugin(daw_model::PluginRef {
        format: daw_model::PluginFormat::Vst3, id: "test".into(), path: String::new(), name: "test".into(), vendor: String::new(),
    });
    app.session.test_controller(instance, Box::new(TestPlugin(0.0)));
    app.playing = true;
    let revision = app.revision;
    let _ = app.update(crate::panels::parameters::Message::Set(instance, 0, 0.25).into());
    assert_eq!(app.revision, revision + 1);
    app.autosave();
    assert!(app.saving.autosaving());
    wait_saves(&mut app);
    assert_eq!(app.autosaved_revision, revision + 1);
    assert!(app.dirty);
    let backup = std::fs::read_dir(&folder).unwrap().next().unwrap().unwrap().path();
    assert_eq!(crate::project_files::load(&backup).unwrap().plugin(instance).unwrap().state, 0.25_f32.to_le_bytes());

    let _ = app.update(crate::panels::parameters::Message::Set(instance, 0, 0.75).into());
    app.config.autosave_minutes = 1;
    app.last_autosave = Instant::now() - Duration::from_secs(61);
    app.tick();
    assert!(app.saving.autosaving());
    assert_eq!(app.autosaved_revision, revision + 1);
    wait_saves(&mut app);
    assert_eq!(app.autosaved_revision, revision + 2);
    let mut backups: Vec<_> = std::fs::read_dir(&folder).unwrap().map(|p| p.unwrap().path()).collect();
    backups.sort();
    assert_eq!(backups.len(), 2);
    assert_eq!(crate::project_files::load(backups.last().unwrap()).unwrap().plugin(instance).unwrap().state, 0.75_f32.to_le_bytes());
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn background_saves_keep_newer_edits_dirty_and_ignore_results_for_replaced_projects() {
    let dir = std::env::temp_dir().join(format!("daw-save-races-{}", crate::project_files::stamp()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("song.dawproj");
    let mut app = app();
    let _ = app.update(Message::NewPattern);
    let _ = app.update(Message::SavedAs(Some(path.clone())));
    assert!(app.dirty);
    let _ = app.update(Message::NewPattern);
    wait_saves(&mut app);
    assert!(app.dirty);
    assert_eq!(app.project.patterns.len(), 3);
    assert_eq!(crate::project_files::load(&path).unwrap().patterns.len(), 2);
    app.save();
    let _ = app.update(Message::NewPattern);
    app.save();
    app.save();
    assert_eq!(wait_saves(&mut app), 2);
    assert!(!app.dirty);
    assert_eq!(crate::project_files::load(&path).unwrap().patterns.len(), 4);

    let _ = app.update(Message::NewPattern);
    app.save();
    app.new_project();
    let current = app.project.clone();
    wait_saves(&mut app);
    assert_eq!(app.project, current);
    assert!(app.path.is_none() && !app.dirty);
    assert_eq!(crate::project_files::load(&path).unwrap().patterns.len(), 5);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn closing_a_clean_project_waits_for_an_in_progress_save_as() {
    let dir = std::env::temp_dir().join(format!("daw-clean-save-close-{}", crate::project_files::stamp()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("clean.dawproj");
    let mut app = app();
    let _ = app.update(Message::SavedAs(Some(path.clone())));
    assert!(!app.dirty);
    let _ = app.update(Message::CloseRequested);
    assert_eq!(app.pending, Some(Pending::Close));
    wait_saves(&mut app);
    assert!(app.pending.is_none() && !app.dirty);
    assert_eq!(crate::project_files::load(&path).unwrap().name, "clean");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn older_save_results_do_not_interrupt_a_newer_save_before_close() {
    for missing_audio in [false, true] {
        let dir = std::env::temp_dir().join(format!("daw-save-order-{}", crate::project_files::stamp()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("song.dawproj");
        let mut app = app();
        app.path = Some(path.clone());
        let _ = app.update(Message::NewPattern);
        if missing_audio {
            app.project.add_channel("missing", daw_model::Source::Audio { path: dir.join("missing.wav").to_string_lossy().into_owned() });
            app.mark_edited();
        }
        app.save();
        if missing_audio { app.project.channels.pop(); }
        let _ = app.update(Message::NewPattern);
        app.save();
        let _ = app.update(Message::CloseRequested);
        let first = app.saving.wait().unwrap();
        let _ = app.save_finished(first);
        assert_eq!(app.pending, Some(Pending::Close));
        assert!(app.dirty);
        assert_eq!(wait_saves(&mut app), 1);
        assert!(app.pending.is_none() && !app.dirty);
        assert_eq!(crate::project_files::load(&path).unwrap().patterns.len(), 3);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn an_older_save_does_not_dismiss_the_unsaved_changes_prompt() {
    let dir = std::env::temp_dir().join(format!("daw-save-prompt-{}", crate::project_files::stamp()));
    std::fs::create_dir(&dir).unwrap();
    let mut app = app();
    app.path = Some(dir.join("song.dawproj"));
    let _ = app.update(Message::NewPattern);
    app.save();
    let _ = app.update(Message::NewPattern);
    let _ = app.update(Message::CloseRequested);
    wait_saves(&mut app);
    assert_eq!(app.pending, Some(Pending::Close));
    assert!(app.dirty);
    let _ = app.update(Message::SaveChoice(SaveChoice::Cancel));
    assert!(app.pending.is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn background_packaging_embeds_audio_and_keeps_unsaved_changes() {
    let dir = std::env::temp_dir().join(format!("daw-background-pack-{}", crate::project_files::stamp()));
    std::fs::create_dir(&dir).unwrap();
    let wav = dir.join("clip.wav");
    write_wav(&wav, 4800);
    let mut app = app();
    app.import_audio(&wav, 0).unwrap();
    let _ = app.update(Message::PackPicked(Some(dir.join("package"))));
    wait_saves(&mut app);
    assert!(app.dirty && app.path.is_none());
    assert!(app.status.contains("packaged"));
    std::fs::remove_file(&wav).unwrap();
    let project = crate::project_files::load(&dir.join("package.dawzip")).unwrap();
    let daw_model::Source::Audio { path } = &project.channels.last().unwrap().source else { panic!("audio") };
    assert_eq!(daw_engine::synth::Sample::load(path).unwrap().left.len(), 4800);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn newer_edits_while_saving_before_close_keep_the_project_open() {
    let dir = std::env::temp_dir().join(format!("daw-save-close-race-{}", crate::project_files::stamp()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("song.dawproj");
    let mut app = app();
    app.path = Some(path.clone());
    let _ = app.update(Message::NewPattern);
    app.pending = Some(Pending::Close);
    let _ = app.update(Message::SaveChoice(SaveChoice::Save));
    assert_eq!(app.pending, Some(Pending::Close));
    let _ = app.update(Message::NewPattern);
    wait_saves(&mut app);
    assert!(app.pending.is_none() && app.dirty);
    assert_eq!(app.project.patterns.len(), 3);
    assert_eq!(crate::project_files::load(&path).unwrap().patterns.len(), 2);
    assert!(app.status.contains("newer changes remain unsaved"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn autosave_preserves_dirty_state_and_recovery_requires_save_as() {
    let dir = std::env::temp_dir().join(format!("daw-backup-audio-{}", crate::project_files::stamp()));
    std::fs::create_dir(&dir).unwrap();
    let wav = dir.join("external.wav");
    write_wav(&wav, 4800);
    let mut app = app();
    app.import_audio(&wav, 0).unwrap();
    app.backup_key = format!("test-{}", crate::project_files::stamp());
    let folder = crate::project_files::backups_dir().join(&app.backup_key);
    app.project.name = "recovered song".into();
    app.dirty = true;
    app.playing = true;
    app.autosave();
    wait_saves(&mut app);
    assert!(app.dirty);
    assert_eq!(app.autosaved_revision, app.revision);
    let backup = std::fs::read_dir(&folder).unwrap().next().unwrap().unwrap().path();
    assert_eq!(crate::project_files::load(&backup).unwrap().name, "recovered song");
    std::fs::remove_file(wav).unwrap();
    let _ = app.proceed(Pending::Recover(backup));
    assert!(app.dirty);
    assert!(app.path.is_none());
    assert_eq!(app.project.name, "recovered song");
    assert!(app.session.preparation_error.is_none(), "{}", app.status);
    let _ = app.update(Message::SavedAs(Some(dir.join("recovered song.dawproj"))));
    wait_saves(&mut app);
    assert!(!app.dirty, "{}", app.status);
    std::fs::remove_dir_all(folder).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn midi_note_held_across_multiple_loops_fills_one_loop() {
    let mut app = app();
    let pattern = app.selected_pattern;
    let bar = app.project.pattern(pattern).unwrap().length;
    app.mode = PlayMode::Pattern(pattern);
    app.midi.recording = true;
    app.midi_packet(midi_packet(&app, 240.0, 1.0));
    let mut off = midi_packet(&app, 480.0, 0.0);
    off.elapsed = (bar * 3 + 480) as f64;
    app.midi_packet(off);
    let notes = app.project.pattern(pattern).unwrap().notes(app.project.channels[0].id);
    assert_eq!(notes.len(), 2);
    assert_eq!(notes.iter().map(|n| n.length).sum::<u64>(), bar);
    assert!(notes.iter().all(|n| n.end() <= bar));
}

#[test]
fn opening_missing_audio_keeps_the_error_visible() {
    let dir = std::env::temp_dir().join(format!("daw-missing-audio-{}", crate::project_files::stamp()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("missing.dawproj");
    let mut project = Project::new();
    let channel = project.add_channel("missing", daw_model::Source::Audio { path: dir.join("missing.wav").to_string_lossy().into_owned() });
    project.add_clip(0, 0, ClipSource::Audio(channel));
    // Legacy projects can still reference missing external files.
    std::fs::write(&path, project.to_ron().unwrap()).unwrap();
    let mut app = app();
    app.open(path);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.session.audio_progress().is_some() {
        let _ = app.update(Message::Tick);
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(app.status.contains("audio preparation failed"));
    assert!(app.status.contains("missing.wav"));
    assert!(app.session.preparation_error.is_some());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_portable_project_renders_identically_after_removing_the_source_folder() {
    let dir = std::env::temp_dir().join(format!("daw-portable-render-{}", crate::project_files::stamp()));
    let source = dir.join("source");
    std::fs::create_dir_all(&source).unwrap();
    let wav = source.join("clip.wav");
    write_wav(&wav, 24_000);
    let mut app = app();
    app.import_audio(&wav, 0).unwrap();
    app.project.playlist.clips[0].audio = daw_model::AudioEdit { stretch: 2.0, semitones: 7.0, reverse: true };
    app.project.playlist.clips[0].length *= 2;
    app.refresh();
    let end = app.project.song_length();
    let before = dir.join("before.wav");
    app.session.export(&app.project, &before, crate::render::RenderOptions { depth: BitDepth::Float32, range: Some((0, end)), tail_seconds: 0.0 }, &[]).unwrap();
    let saved = dir.join("song.dawproj");
    let _ = app.update(Message::SavedAs(Some(saved.clone())));
    wait_saves(&mut app);
    assert!(!app.dirty, "{}", app.status);
    std::fs::remove_dir_all(&source).unwrap();
    let moved = dir.join("relocated");
    std::fs::create_dir(&moved).unwrap();
    let relocated = moved.join("song.dawproj");
    std::fs::rename(&saved, &relocated).unwrap();
    app.open(relocated.clone());
    assert_eq!(app.path, Some(relocated.clone()));
    assert!(app.session.preparation_error.is_none());
    let after = dir.join("after.wav");
    app.session.export(&app.project, &after, crate::render::RenderOptions { depth: BitDepth::Float32, range: Some((0, end)), tail_seconds: 0.0 }, &[]).unwrap();
    let before = daw_engine::synth::Sample::load(before.to_str().unwrap()).unwrap();
    let after = daw_engine::synth::Sample::load(after.to_str().unwrap()).unwrap();
    assert!(before.left.iter().any(|s| s.abs() > 0.01));
    assert_eq!(before.left, after.left);
    assert_eq!(before.right, after.right);
    // Editing and saving the moved project keeps it self-contained.
    let _ = app.update(Message::NewPattern);
    let _ = app.update(Message::Action(Action::Save));
    wait_saves(&mut app);
    assert!(!app.dirty, "{}", app.status);
    let loaded = crate::project_files::load(&relocated).unwrap();
    assert_eq!(loaded.patterns.len(), 2);
    assert_eq!(loaded.playlist.clips[0].audio, app.project.playlist.clips[0].audio);
    std::fs::remove_dir_all(dir).unwrap();
}

fn test_plugin(app: &mut App) -> daw_model::InstanceId {
    let id = app.project.add_plugin(daw_model::PluginRef {
        format: daw_model::PluginFormat::Vst3, id: "test".into(), path: String::new(), name: "test".into(), vendor: String::new(),
    });
    app.session.test_controller(id, Box::new(TestPlugin(0.0)));
    id
}

#[test]
fn plugin_slider_drag_restores_settings_and_groups_recording_in_one_undo() {
    let mut app = app();
    let id = test_plugin(&mut app);
    let _ = app.update(crate::panels::parameters::Message::Set(id, 0, 0.0).into());
    assert!(!app.dirty);
    assert!(app.undo.is_empty());
    app.bind_mode = true;
    app.record = true;
    app.playing = true;
    for value in [0.2, 0.4, 0.8] {
        let _ = app.update(crate::panels::parameters::Message::Set(id, 0, value).into());
    }
    let _ = app.update(Message::EndEdit);
    assert_eq!(app.undo.len(), 1);
    assert_eq!(app.session.param_value(id, 0), 0.8);
    assert!(app.project.automation_for(Target::Plugin { instance: id, param: 0 }).is_some());
    let _ = app.update(Message::Action(Action::Undo));
    assert_eq!(app.session.param_value(id, 0), 0.0);
    assert!(app.project.automation_for(Target::Plugin { instance: id, param: 0 }).is_none());
    let _ = app.update(Message::Action(Action::Redo));
    assert_eq!(app.session.param_value(id, 0), 0.8);
    assert_eq!(app.project.plugin(id).unwrap().state, 0.8_f32.to_le_bytes());
    let _ = app.update(crate::panels::parameters::Message::Set(id, 0, 0.6).into());
    let _ = app.update(Message::EndEdit);
    assert_eq!(app.undo.len(), 2);
    let _ = app.update(Message::Action(Action::Undo));
    assert_eq!(app.session.param_value(id, 0), 0.8);
}

#[test]
fn native_plugin_gestures_capture_previous_values_before_polling() {
    use daw_plugins::Touch;
    let mut app = app();
    let id = test_plugin(&mut app);
    app.plugin_touch(id, Touch::Begin(0));
    app.plugin_touch(id, Touch::Value { param: 0, value: 0.0 });
    app.plugin_touch(id, Touch::End(0));
    assert!(!app.dirty);
    assert!(app.undo.is_empty());
    // Both gestures have already reached the plugin before the UI polls them.
    app.session.set_plugin_param(id, 0, 0.9);
    app.bind_mode = true;
    for value in [0.4, 0.9] {
        app.plugin_touch(id, Touch::Begin(0));
        app.plugin_touch(id, Touch::Value { param: 0, value });
        app.plugin_touch(id, Touch::End(0));
    }
    assert_eq!(app.undo.len(), 2);
    for value in [0.4, 0.0] {
        let _ = app.update(Message::Action(Action::Undo));
        assert_eq!(app.session.param_value(id, 0), value);
    }
    for value in [0.4, 0.9] {
        let _ = app.update(Message::Action(Action::Redo));
        assert_eq!(app.session.param_value(id, 0), value);
    }
    // Value-only editors also coalesce edits until their gesture is finished.
    app.session.set_plugin_param(id, 0, 0.3);
    app.plugin_touch(id, Touch::Value { param: 0, value: 0.5 });
    app.plugin_touch(id, Touch::Value { param: 0, value: 0.3 });
    app.finish_plugin_edits();
    assert_eq!(app.undo.len(), 3);
    let _ = app.update(Message::Action(Action::Undo));
    assert_eq!(app.session.param_value(id, 0), 0.9);
}

#[test]
fn precise_audio_values_commit_once_and_reset_without_spurious_undo() {
    use list::AudioField;
    let mut app = app();
    let dir = std::env::temp_dir().join(format!("daw-precise-{}", crate::project_files::stamp()));
    app.path = Some(dir.join("song.dawproj"));
    let path = app.place_take(daw_engine::input::Take { start: 0.0, channels: 2, sample_rate: 48_000, samples: vec![0.25; 96_000] }).unwrap().unwrap();
    let clip = app.project.playlist.clips.last().unwrap().id;
    app.playlist.selected = vec![clip];
    app.undo.clear();
    app.dirty = false;
    let updates = app.session.song_updates;
    for (field, value) in [(AudioField::Pitch, "-3"), (AudioField::Cents, "-25.5")] {
        let _ = app.update(list::Message::AudioText(field, value.into()).into());
    }
    assert!(!app.dirty);
    assert_eq!(app.session.song_updates, updates);
    let _ = app.update(list::Message::AudioApply(AudioField::Pitch).into());
    assert_eq!(app.project.playlist.clips.last().unwrap().audio.semitones, -3.255);
    assert_eq!(app.undo.len(), 1);
    let old = app.project.playlist.clips.last().unwrap().clone();
    let _ = app.update(list::Message::AudioText(AudioField::Stretch, "1.333333333".into()).into());
    let _ = app.update(list::Message::AudioApply(AudioField::Stretch).into());
    let now = app.project.playlist.clips.last().unwrap();
    assert_eq!(now.audio.stretch, 1.333333333);
    assert_eq!(now.length, (old.length as f64 * 1.333333333).round() as Ticks);
    assert_eq!(app.undo.len(), 2);
    let _ = app.update(list::Message::AudioText(AudioField::Stretch, "NaN".into()).into());
    let _ = app.update(list::Message::AudioApply(AudioField::Stretch).into());
    assert_eq!(app.undo.len(), 2);
    assert!(app.status.contains("between"));
    let _ = app.update(list::Message::AudioReset(AudioField::Cents).into());
    assert_eq!(app.project.playlist.clips.last().unwrap().audio.semitones, -3.0);
    let _ = app.update(list::Message::AudioReset(AudioField::Pitch).into());
    let _ = app.update(list::Message::AudioReset(AudioField::Stretch).into());
    assert_eq!(app.project.playlist.clips.last().unwrap().audio, daw_model::AudioEdit::default());
    let undos = app.undo.len();
    let _ = app.update(list::Message::AudioReset(AudioField::Stretch).into());
    let _ = app.update(list::Message::AudioReset(AudioField::Pitch).into());
    assert_eq!(app.undo.len(), undos);
    let _ = app.update(Message::Action(Action::Undo));
    assert_eq!(app.project.playlist.clips.last().unwrap().audio.stretch, 1.333333333);
    assert!(path.exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn render_tails_are_adjustable_portable_and_extend_consolidated_clips() {
    use crate::panels::settings::{Message, Tail};
    let mut app = app();
    let dir = std::env::temp_dir().join(format!("daw-tails-{}", crate::project_files::stamp()));
    std::fs::create_dir_all(&dir).unwrap();
    app.path = Some(dir.join("song.dawproj"));
    let channel = app.selected_channel.unwrap();
    let pattern = app.selected_pattern;
    let _ = app.update(roll::Message::Add { start: 3600, key: 60 }.into());
    let id = app.project.add_clip(0, 0, ClipSource::Pattern(pattern));
    let end = app.project.playlist.clips.last().unwrap().end();
    app.playlist.selected = vec![id];
    let _ = app.update(Message::TailText(Tail::Consolidation, "0.75".into()).into());
    let _ = app.update(Message::TailDone(Tail::Consolidation).into());
    let _ = app.update(Message::TailText(Tail::Export, "1.25".into()).into());
    let _ = app.update(Message::TailDone(Tail::Export).into());
    crate::project_files::save(app.path.as_ref().unwrap(), &app.project).unwrap();
    assert_eq!(crate::project_files::load(app.path.as_ref().unwrap()).unwrap().render, app.project.render);
    let _ = app.update(list::Message::Consolidate.into());
    wait_renders(&mut app);
    let clip = app.project.playlist.clips.last().unwrap();
    assert_eq!(clip.length, end + daw_model::time::seconds_to_ticks(0.75, app.project.bpm).round() as Ticks);
    let ClipSource::Audio(audio) = clip.source else { panic!("consolidated audio") };
    let daw_model::Source::Audio { path } = &app.project.channel(audio).unwrap().source else { panic!("audio file") };
    let sample = daw_engine::synth::Sample::load(path).unwrap();
    let nominal_frames = (daw_model::time::ticks_to_seconds(end as f64, app.project.bpm) * app.session.sample_rate()).round() as usize;
    assert_eq!(sample.left.len(), nominal_frames + (0.75 * app.session.sample_rate()).round() as usize);
    assert!(sample.left[nominal_frames..].iter().any(|s| s.abs() > 0.0001), "synth release was cut off");
    assert!(app.project.channel(channel).is_some());
    let options = crate::render::RenderOptions { depth: BitDepth::Float32, range: Some((100, 200)), tail_seconds: 1.25 };
    let (_, frames) = options.timing(&app.project, 48_000.0).unwrap();
    assert_eq!(frames, ((daw_model::time::ticks_to_seconds(100.0, app.project.bpm) + 1.25) * 48_000.0).round() as usize);
    assert!(crate::render::RenderOptions { tail_seconds: f64::NAN, ..options }.timing(&app.project, 48_000.0).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn background_export_keeps_playback_and_uses_the_requested_snapshot() {
    let dir = std::env::temp_dir().join(format!("daw-export-job-{}", crate::project_files::stamp()));
    std::fs::create_dir(&dir).unwrap();
    let mut app = app();
    let channel = app.project.channels[0].id;
    let pattern = app.selected_pattern;
    app.project.pattern_mut(pattern).unwrap().toggle_step(channel, 0);
    app.project.add_clip(0, 0, ClipSource::Pattern(pattern));
    app.playing = true;
    let path = dir.join("snapshot.wav");
    let _ = app.update(Message::Exported(Some(path.clone())));
    assert!(app.rendering.busy());
    assert!(!path.exists(), "publish only when the UI accepts the result");
    assert!(app.playing);
    let _ = app.update(crate::panels::channel_rack::Message::Volume(channel, 0.0).into());
    wait_renders(&mut app);
    assert!(app.playing);
    assert!(app.status.starts_with("exported"), "{}", app.status);
    let sample = daw_engine::synth::Sample::load(path.to_str().unwrap()).unwrap();
    assert!(sample.left.iter().any(|s| s.abs() > 0.001), "render must use the volume from the snapshot");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cancelled_failed_and_obsolete_exports_preserve_existing_files() {
    let dir = std::env::temp_dir().join(format!("daw-export-cleanup-{}", crate::project_files::stamp()));
    std::fs::create_dir(&dir).unwrap();
    let mut app = app();
    let path = dir.join("keep.wav");
    let original = b"existing file";
    std::fs::write(&path, original).unwrap();
    let _ = app.update(Message::Exported(Some(path.clone())));
    let _ = app.update(Message::CancelRender);
    wait_renders(&mut app);
    assert!(app.status.contains("cancelled"));
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);

    app.project.add_channel("missing", daw_model::Source::Audio { path: dir.join("missing.wav").to_string_lossy().into_owned() });
    let _ = app.update(Message::Exported(Some(path.clone())));
    wait_renders(&mut app);
    assert!(app.status.contains("failed"), "{}", app.status);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);

    app.project = Project::new();
    let _ = app.update(Message::Exported(Some(path.clone())));
    app.rendering.new_project();
    wait_renders(&mut app);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn pending_consolidation_creates_no_undo_and_rejects_changed_projects() {
    let dir = std::env::temp_dir().join(format!("daw-consolidate-job-{}", crate::project_files::stamp()));
    std::fs::create_dir(&dir).unwrap();
    let mut app = app();
    app.path = Some(dir.join("song.dawproj"));
    let pattern = app.selected_pattern;
    let source = app.project.add_clip(0, 0, ClipSource::Pattern(pattern));
    app.playlist.selected = vec![source];
    let undo = app.undo.len();
    app.consolidate_selection().unwrap();
    assert_eq!(app.undo.len(), undo);
    assert!(!app.project.playlist.clips[0].muted);
    let _ = app.update(Message::CancelRender);
    wait_renders(&mut app);
    assert_eq!(app.undo.len(), undo);
    assert_eq!(app.project.playlist.clips.len(), 1);
    assert_eq!(std::fs::read_dir(dir.join("recordings")).unwrap().count(), 0);

    app.consolidate_selection().unwrap();
    app.mark_edited();
    wait_renders(&mut app);
    assert!(app.status.contains("project changed"), "{}", app.status);
    assert_eq!(app.undo.len(), undo);
    assert!(!app.project.playlist.clips[0].muted);
    assert_eq!(std::fs::read_dir(dir.join("recordings")).unwrap().count(), 0);

    app.consolidate_selection().unwrap();
    wait_renders(&mut app);
    assert_eq!(app.undo.len(), undo + 1);
    assert!(app.project.playlist.clips[0].muted);
    wait_audio(&mut app);
    std::fs::remove_dir_all(dir).unwrap();
}
