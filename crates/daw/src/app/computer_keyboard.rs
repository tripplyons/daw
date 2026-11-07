//! Musical typing in the main window and native plugin editors. Keep the note-on target until release.

use std::collections::HashMap;

use daw_engine::song::channel_node;
use crate::input::{Event, Modifiers, key::{Code, Physical}};

use super::App;
use crate::midi::Packet;

pub(super) const PIANO: &[Code] = &[
    Code::KeyA, Code::KeyW, Code::KeyS, Code::KeyE, Code::KeyD, Code::KeyF,
    Code::KeyT, Code::KeyG, Code::KeyY, Code::KeyH, Code::KeyU, Code::KeyJ, Code::KeyK,
];
// Hardware MIDI channels occupy 0..16. Keep computer notes in their own lane.
const COMPUTER_CHANNEL: u8 = 16;

pub struct State {
    pub enabled: bool,
    pub base: u8,
    held: HashMap<Code, Packet>,
}

impl Default for State {
    fn default() -> Self { Self { enabled: false, base: 60, held: HashMap::new() } }
}

enum Input {
    Ignored,
    Consumed,
    Note(Packet),
}

impl State {
    fn key(&mut self, event: &Event, blocked: bool, target: Option<Packet>) -> Input {
        if let Event::KeyReleased { physical_key: Physical::Code(code), .. } = event {
            return self.held.remove(code).map_or(Input::Ignored, |packet| Input::Note(Packet { velocity: 0.0, ..packet }));
        }
        let Event::KeyPressed { physical_key: Physical::Code(code), modifiers, repeat, .. } = event else { return Input::Ignored };
        if !self.enabled || blocked || *modifiers != Modifiers::empty() { return Input::Ignored; }
        if matches!(code, Code::KeyZ | Code::KeyX) {
            if !repeat {
                self.base = if *code == Code::KeyZ { self.base.saturating_sub(12) } else { (self.base + 12).min(108) };
            }
            return Input::Consumed;
        }
        let Some(offset) = PIANO.iter().position(|key| key == code) else { return Input::Ignored };
        if *repeat || self.held.contains_key(code) { return Input::Consumed; }
        let Some(target) = target else { return Input::Consumed };
        let packet = Packet { midi_channel: COMPUTER_CHANNEL, key: self.base + offset as u8, ..target };
        self.held.insert(*code, packet);
        Input::Note(packet)
    }
}

impl App {
    /// Returns true only when musical typing consumed the event. Releases are
    /// processed even if a text field or shortcut capture now owns the keyboard.
    pub(super) fn computer_key(&mut self, event: &Event, typing: bool) -> bool {
        let shared = self.session.shared();
        let elapsed = shared.record_position();
        let target = self.selected_channel.and_then(|id| self.project.channel(id)).map(|channel| Packet {
            midi_channel: COMPUTER_CHANNEL, channel: channel.id, node: channel_node(&channel.source, channel.id),
            key: 0, velocity: 0.8, tick: shared.position(), elapsed,
            recording: self.midi.recording && shared.playing(),
        }).filter(|packet| packet.node != 0);
        let blocked = typing || self.settings.capturing.is_some() || self.menu.is_some();
        match self.computer_keyboard.key(event, blocked, target) {
            Input::Ignored => false,
            Input::Consumed => true,
            Input::Note(packet) => {
                self.session.note(packet.node, packet.key, packet.velocity);
                self.midi_packet(Packet { elapsed, ..packet });
                true
            }
        }
    }

    pub(crate) fn release_computer_keys(&mut self) {
        let elapsed = self.session.shared().record_position();
        let held = std::mem::take(&mut self.computer_keyboard.held);
        for packet in held.into_values() {
            self.session.note(packet.node, packet.key, 0.0);
            self.midi_packet(Packet { velocity: 0.0, elapsed, ..packet });
        }
    }

    pub(super) fn toggle_computer_keyboard(&mut self) {
        self.release_computer_keys();
        self.computer_keyboard.enabled = !self.computer_keyboard.enabled;
        #[cfg(target_os = "macos")]
        super::PLUGIN_KEYBOARD_ENABLED.store(self.computer_keyboard.enabled, std::sync::atomic::Ordering::Relaxed);
        self.set_status(if self.computer_keyboard.enabled {
            "Computer keyboard on: A W S E D F T G Y H U J K play notes; Z/X change octave"
        } else { "Computer keyboard off" });
    }
}
