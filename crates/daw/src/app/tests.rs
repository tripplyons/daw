//! Drives `App::update` the way the UI does and checks the project.

use daw_model::automation::{Shape, TimeSnap, ValueSnap};
use daw_model::layout::{Axis, Panel};
use daw_model::time::Grid;
use daw_model::{ClipSource, Target};
use iced::keyboard::key::{Code, Physical};
use iced::keyboard::{self, Key, Location, Modifiers};

use super::*;
use crate::panels::{automation as auto, piano_roll as roll, playlist as list};

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

    // Cmd bypasses snapping.
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
    let _ = app.update(press(Code::KeyA, char_key("a"), Modifiers::LOGO));
    assert_eq!(app.automation.selected.len(), 2);
    let _ = app.update(press(Code::Digit5, char_key("5"), Modifiers::empty()));
    assert!(points(&app).iter().all(|p| p.2 == Shape::Stairs(4)));

    // Duplicate the selection after itself.
    let _ = app.update(press(Code::KeyD, char_key("d"), Modifiers::LOGO));
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
    let _ = app.update(press(Code::KeyD, char_key("d"), Modifiers::LOGO));
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
    assert!(!app.dirty);
    let saved = app.project.clone();

    let mut other = self::app();
    other.open(path);
    assert_eq!(other.project.patterns, saved.patterns);
    assert_eq!(other.project.automation, saved.automation);
    assert_eq!(other.selected_pattern, pattern);

    let wav = dir.join("song.wav");
    let _ = other.update(Message::Exported(Some(wav.clone())));
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
fn play_starts_from_pattern_start_or_song_marker_and_pause_returns_there() {
    let mut app = app();
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
}

#[test]
fn closing_prompts_only_with_unsaved_changes_and_saves_before_quitting() {
    let dir = std::env::temp_dir().join(format!("daw-close-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    // Clean project: no prompt.
    let mut app = app();
    let _ = app.update(Message::CloseRequested);
    assert!(!app.closing);

    // Unsaved changes: the prompt opens once, and Cancel keeps everything.
    let _ = app.update(Message::NewPattern);
    assert!(app.dirty);
    let _ = app.update(Message::CloseRequested);
    assert!(app.closing);
    let _ = app.update(Message::CloseChoice(CloseChoice::Cancel));
    assert!(!app.closing && app.dirty);

    // Save with a known path writes the file before quitting.
    let path = dir.join("close.dawproj");
    app.path = Some(path.clone());
    let _ = app.update(Message::CloseRequested);
    let _ = app.update(Message::CloseChoice(CloseChoice::Save));
    assert!(!app.dirty);
    assert_eq!(Project::from_ron(&std::fs::read_to_string(&path).unwrap()).unwrap().patterns.len(), 2);

    // A failed save keeps the app open with the changes.
    let _ = app.update(Message::NewPattern);
    app.path = Some(dir.join("missing-dir").join("x.dawproj"));
    let _ = app.update(Message::CloseRequested);
    let _ = app.update(Message::CloseChoice(CloseChoice::Save));
    assert!(app.dirty && !app.closing);
    assert!(app.status.contains("save failed"), "{}", app.status);

    // Untitled project: cancelling the save dialog keeps the app open.
    app.path = None;
    let _ = app.update(Message::CloseRequested);
    let _ = app.update(Message::SavedAsThenClose(None));
    assert!(app.dirty && !app.closing);

    // Plugin parameter edits count as unsaved changes.
    let mut clean = self::app();
    clean.touched(Target::Tempo, 0.5);
    assert!(clean.dirty);
    std::fs::remove_dir_all(&dir).unwrap();
}
