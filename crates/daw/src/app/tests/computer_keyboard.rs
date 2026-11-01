use super::*;

fn recording_app() -> App {
    let mut app = app();
    app.mode = PlayMode::Pattern(app.selected_pattern);
    let _ = app.update(Message::Action(Action::ToggleComputerKeyboard));
    let _ = app.update(Message::Action(Action::ToggleMidiRecord));
    let _ = app.update(Message::Action(Action::PlayPause));
    app.session.test_render(512);
    assert!(app.session.shared().playing());
    app
}

fn down(code: Code) -> Message { press(code, Key::Unidentified, Modifiers::empty()) }

fn up(code: Code, captured: bool) -> Message {
    Message::Key(keyboard::Event::KeyReleased {
        key: Key::Unidentified, modified_key: Key::Unidentified,
        physical_key: Physical::Code(code), location: Location::Standard,
        modifiers: Modifiers::COMMAND,
    }, captured)
}

fn notes(app: &App, channel: daw_model::ChannelId) -> &[daw_model::Note] {
    app.project.pattern(app.selected_pattern).unwrap().notes(channel)
}

#[test]
fn computer_keyboard_plays_while_stopped_and_releases_the_live_voice() {
    for release in [up(Code::KeyA, false), Message::WindowUnfocused, Message::Action(Action::ToggleComputerKeyboard)] {
        let mut app = app();
        let _ = app.update(Message::Action(Action::ToggleComputerKeyboard));
        assert_eq!(app.session.test_render(512), 0.0);
        let _ = app.update(down(Code::KeyA));
        assert!(app.session.test_render(4800) > 0.01);
        let _ = app.update(release);
        app.session.test_render(48_000); // Let the synth's release envelope finish.
        assert!(app.session.test_render(512) < 0.000001);
        assert!(!app.playing);
    }
}

#[test]
fn computer_keyboard_records_chords_and_ignores_repeats() {
    let mut app = recording_app();
    let channel = app.selected_channel.unwrap();
    let keys = [Code::KeyA, Code::KeyW, Code::KeyS, Code::KeyE, Code::KeyD,
        Code::KeyF, Code::KeyT, Code::KeyG, Code::KeyY, Code::KeyH,
        Code::KeyU, Code::KeyJ, Code::KeyK];
    for code in keys {
        let _ = app.update(down(code));
        let Message::Key(mut event, _) = down(code) else { unreachable!() };
        if let keyboard::Event::KeyPressed { repeat, .. } = &mut event { *repeat = true; }
        let _ = app.update(Message::Key(event, false));
    }
    app.session.test_render(4800);
    for code in keys { let _ = app.update(up(code, false)); }
    let recorded = notes(&app, channel);
    assert_eq!(recorded.len(), keys.len());
    for (note, pitch) in recorded.iter().zip(60..=72) {
        assert_eq!(note.key, pitch);
        assert!(note.length > 1);
        assert_eq!(note.velocity, 0.8);
    }
}

#[test]
fn computer_keyboard_keeps_original_instrument_and_octave_until_release() {
    let mut app = recording_app();
    let original = app.selected_channel.unwrap();
    let _ = app.update(down(Code::KeyA));
    let other = app.project.add_channel("other", daw_model::Source::Synth(Default::default()));
    app.selected_channel = Some(other);
    let _ = app.update(down(Code::KeyX));
    let _ = app.update(down(Code::KeyW));
    app.session.test_render(4800);
    let _ = app.update(up(Code::KeyA, true));
    let _ = app.update(up(Code::KeyW, true));
    assert_eq!(notes(&app, original)[0].key, 60);
    assert_eq!(notes(&app, other)[0].key, 73);
    assert_eq!(notes(&app, original).len(), 1);
    assert_eq!(notes(&app, other).len(), 1);
}

#[test]
fn computer_keyboard_releases_notes_on_focus_loss_and_disable() {
    for release in [Message::WindowUnfocused, Message::Action(Action::ToggleComputerKeyboard)] {
        let mut app = recording_app();
        let channel = app.selected_channel.unwrap();
        let _ = app.update(down(Code::KeyA));
        let _ = app.update(down(Code::KeyD));
        app.session.test_render(4800);
        let _ = app.update(release);
        assert_eq!(notes(&app, channel).len(), 2);
        assert!(notes(&app, channel).iter().all(|n| n.length > 1));
        // A later key-up must not finish either note a second time.
        let _ = app.update(up(Code::KeyA, true));
        let _ = app.update(up(Code::KeyD, true));
        assert_eq!(notes(&app, channel).len(), 2);
    }
}

#[test]
fn computer_keyboard_respects_text_entry_capture_and_modified_shortcuts() {
    let mut app = recording_app();
    let channel = app.selected_channel.unwrap();
    let Message::Key(event, _) = down(Code::KeyA) else { unreachable!() };
    let _ = app.update(Message::Key(event, true));
    let _ = app.update(up(Code::KeyA, true));
    let _ = app.update(press(Code::KeyA, Key::Unidentified, Modifiers::COMMAND));
    let _ = app.update(up(Code::KeyA, false));
    assert!(notes(&app, channel).is_empty());
    // A mapped letter still works as a shortcut when it has a modifier.
    let before = app.record;
    let _ = app.update(press(Code::KeyR, Key::Unidentified, Modifiers::COMMAND));
    assert_eq!(app.record, !before);
    app.settings.capturing = Some("toggle-mode");
    let _ = app.update(down(Code::KeyA));
    let _ = app.update(up(Code::KeyA, false));
    assert!(app.settings.capturing.is_none());
    assert!(notes(&app, channel).is_empty());
    // Entering capture while a note is held must not swallow its release.
    let _ = app.update(down(Code::KeyD));
    app.settings.capturing = Some("toggle-mode");
    app.session.test_render(4800);
    let _ = app.update(up(Code::KeyD, true));
    assert_eq!(notes(&app, channel).len(), 1);
}

#[test]
fn computer_keyboard_octaves_are_bounded_and_repeat_safe() {
    let mut app = app();
    assert!(!app.computer_keyboard.enabled);
    assert_eq!(app.computer_keyboard.base, 60);
    let _ = app.update(down(Code::KeyX));
    assert_eq!(app.computer_keyboard.base, 60);
    let _ = app.update(Message::Action(Action::ToggleComputerKeyboard));
    let _ = app.update(down(Code::KeyX));
    assert_eq!(app.computer_keyboard.base, 72);
    let Message::Key(mut event, _) = down(Code::KeyX) else { unreachable!() };
    if let keyboard::Event::KeyPressed { repeat, .. } = &mut event { *repeat = true; }
    let _ = app.update(Message::Key(event, false));
    assert_eq!(app.computer_keyboard.base, 72);
    for _ in 0..20 { let _ = app.update(down(Code::KeyX)); }
    assert_eq!(app.computer_keyboard.base, 108);
    for _ in 0..20 { let _ = app.update(down(Code::KeyZ)); }
    assert_eq!(app.computer_keyboard.base, 0);
}

#[test]
fn computer_keyboard_uses_configurable_toggle_and_not_plugin_editor_presses() {
    let mut app = app();
    let _ = app.update(press(Code::KeyM, Key::Unidentified, Modifiers::COMMAND | Modifiers::SHIFT));
    assert!(app.computer_keyboard.enabled);
    let mut config = app.keymap.to_config();
    config.insert("computer-keyboard".into(), vec!["a".into()]);
    app.keymap = crate::keys::Keymap::from_config(&config).0;
    let _ = app.update(down(Code::KeyA));
    assert!(!app.computer_keyboard.enabled);
    let _ = app.update(down(Code::KeyA));
    assert!(app.computer_keyboard.enabled);

    let mut app = recording_app();
    let channel = app.selected_channel.unwrap();
    let Message::Key(event, _) = down(Code::KeyA) else { unreachable!() };
    let _ = app.update(Message::PluginKey(event));
    let _ = app.update(up(Code::KeyA, false));
    assert!(notes(&app, channel).is_empty());
}

#[test]
fn computer_keyboard_records_song_takes_through_existing_midi_controls() {
    let mut app = app();
    app.mode = PlayMode::Song;
    let channel = app.selected_channel.unwrap();
    let count = app.project.patterns.len();
    let _ = app.update(Message::Action(Action::ToggleComputerKeyboard));
    assert!(!app.midi.recording);
    let _ = app.update(Message::Action(Action::ToggleMidiRecord));
    let _ = app.update(Message::Action(Action::PlayPause));
    app.session.test_render(512);
    let _ = app.update(down(Code::KeyA));
    app.session.test_render(4800);
    let _ = app.update(up(Code::KeyA, false));
    assert_eq!(app.project.patterns.len(), count + 1);
    let take = app.project.patterns.last().unwrap();
    assert_eq!(take.name, "MIDI take");
    assert_eq!(take.notes(channel).len(), 1);
    assert_eq!(take.notes(channel)[0].key, 60);
    assert!(app.project.playlist.clips.iter().any(|clip| clip.source == ClipSource::Pattern(take.id)));
}

#[test]
fn computer_keyboard_does_not_record_when_stopped_or_unarmed() {
    for armed in [false, true] {
        let mut app = app();
        let channel = app.selected_channel.unwrap();
        let _ = app.update(Message::Action(Action::ToggleComputerKeyboard));
        if armed { let _ = app.update(Message::Action(Action::ToggleMidiRecord)); }
        let _ = app.update(down(Code::KeyA));
        app.session.test_render(4800);
        let _ = app.update(up(Code::KeyA, false));
        assert!(notes(&app, channel).is_empty());
        assert!(!app.playing);
        if !armed {
            let _ = app.update(Message::Action(Action::PlayPause));
            app.session.test_render(512);
            let _ = app.update(down(Code::KeyA));
            app.session.test_render(4800);
            let _ = app.update(up(Code::KeyA, false));
            assert!(notes(&app, channel).is_empty());
        }
    }
}

#[test]
fn computer_keyboard_recording_is_separate_from_hardware_midi() {
    let mut app = recording_app();
    let channel = app.selected_channel.unwrap();
    let _ = app.update(down(Code::KeyD)); // MIDI 64, like midi_packet below.
    let packet = midi_packet(&app, 0.0, 0.6);
    app.midi_packet(packet);
    app.session.test_render(4800);
    let _ = app.update(up(Code::KeyD, false));
    assert_eq!(notes(&app, channel).len(), 1);
    app.midi_packet(crate::midi::Packet { velocity: 0.0, elapsed: 240.0, ..packet });
    assert_eq!(notes(&app, channel).len(), 2);
}

#[test]
fn computer_keyboard_finishes_on_transport_stop_without_retriggering_repeats() {
    let mut app = recording_app();
    let channel = app.selected_channel.unwrap();
    let _ = app.update(down(Code::KeyA));
    app.session.test_render(4800);
    let _ = app.update(Message::Action(Action::Stop));
    assert_eq!(notes(&app, channel).len(), 1);
    let Message::Key(mut event, _) = down(Code::KeyA) else { unreachable!() };
    if let keyboard::Event::KeyPressed { repeat, .. } = &mut event { *repeat = true; }
    let _ = app.update(Message::Key(event, false));
    let _ = app.update(up(Code::KeyA, false));
    assert_eq!(notes(&app, channel).len(), 1);
}
