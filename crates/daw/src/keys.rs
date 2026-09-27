//! Configurable key bindings. Chords use physical keys because Alt changes the
//! typed character on macOS. Alt is the default tiling modifier so Cmd stays
//! free for app commands.

use std::collections::BTreeMap;

use daw_model::layout::{Axis, Direction};
use iced::keyboard::key::{Code, Physical};
use iced::keyboard::{Event, Modifiers};
#[cfg(target_os = "macos")]
use iced::keyboard::{Key, Location};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    Focus(Direction),
    Swap(Direction),
    Resize(Direction),
    SplitAxis(Axis),
    Split,
    Close,
    CyclePanel,
    Zoom,
    Workspace(usize),
    ToggleBind,
    ToggleRecord,
    ToggleMode,
    PlayPause,
    Stop,
    Undo,
    Redo,
    New,
    Open,
    Save,
    SaveAs,
    Export,
    Settings,
    Delete,
}

/// A bindable action: stable id for the config file, label, and group.
pub struct Binding {
    pub id: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub action: Action,
    defaults: &'static [&'static str],
}

const fn bind(id: &'static str, label: &'static str, group: &'static str, action: Action, defaults: &'static [&'static str]) -> Binding {
    Binding { id, label, group, action, defaults }
}

use Direction::{Down, Left, Right, Up};

pub const BINDINGS: &[Binding] = &[
    bind("focus-left", "focus left", "tiling", Action::Focus(Left), &["alt+h", "alt+left"]),
    bind("focus-down", "focus down", "tiling", Action::Focus(Down), &["alt+j", "alt+down"]),
    bind("focus-up", "focus up", "tiling", Action::Focus(Up), &["alt+k", "alt+up"]),
    bind("focus-right", "focus right", "tiling", Action::Focus(Right), &["alt+l", "alt+right"]),
    bind("swap-left", "move tile left", "tiling", Action::Swap(Left), &["alt+shift+h", "alt+shift+left"]),
    bind("swap-down", "move tile down", "tiling", Action::Swap(Down), &["alt+shift+j", "alt+shift+down"]),
    bind("swap-up", "move tile up", "tiling", Action::Swap(Up), &["alt+shift+k", "alt+shift+up"]),
    bind("swap-right", "move tile right", "tiling", Action::Swap(Right), &["alt+shift+l", "alt+shift+right"]),
    bind("resize-left", "resize left", "tiling", Action::Resize(Left), &["ctrl+alt+h", "ctrl+alt+left"]),
    bind("resize-down", "resize down", "tiling", Action::Resize(Down), &["ctrl+alt+j", "ctrl+alt+down"]),
    bind("resize-up", "resize up", "tiling", Action::Resize(Up), &["ctrl+alt+k", "ctrl+alt+up"]),
    bind("resize-right", "resize right", "tiling", Action::Resize(Right), &["ctrl+alt+l", "ctrl+alt+right"]),
    bind("split", "split tile", "tiling", Action::Split, &["alt+enter"]),
    bind("split-stacked", "next split stacked", "tiling", Action::SplitAxis(Axis::Vertical), &["alt+v"]),
    bind("split-side", "next split side by side", "tiling", Action::SplitAxis(Axis::Horizontal), &["alt+b"]),
    bind("close", "close tile", "tiling", Action::Close, &["alt+q"]),
    bind("cycle-panel", "change panel", "tiling", Action::CyclePanel, &["alt+space"]),
    bind("zoom", "focus mode", "tiling", Action::Zoom, &["alt+f"]),
    bind("workspace-1", "workspace 1", "workspaces", Action::Workspace(0), &["alt+1"]),
    bind("workspace-2", "workspace 2", "workspaces", Action::Workspace(1), &["alt+2"]),
    bind("workspace-3", "workspace 3", "workspaces", Action::Workspace(2), &["alt+3"]),
    bind("workspace-4", "workspace 4", "workspaces", Action::Workspace(3), &["alt+4"]),
    bind("workspace-5", "workspace 5", "workspaces", Action::Workspace(4), &["alt+5"]),
    bind("workspace-6", "workspace 6", "workspaces", Action::Workspace(5), &["alt+6"]),
    bind("workspace-7", "workspace 7", "workspaces", Action::Workspace(6), &["alt+7"]),
    bind("workspace-8", "workspace 8", "workspaces", Action::Workspace(7), &["alt+8"]),
    bind("workspace-9", "workspace 9", "workspaces", Action::Workspace(8), &["alt+9"]),
    bind("play-pause", "play or stop", "transport", Action::PlayPause, &["space"]),
    bind("stop", "stop and rewind", "transport", Action::Stop, &["escape"]),
    bind("toggle-mode", "pattern or song mode", "transport", Action::ToggleMode, &["alt+s"]),
    bind("toggle-bind", "bind mode", "automation", Action::ToggleBind, &["alt+a"]),
    bind("toggle-record", "record automation", "automation", Action::ToggleRecord, &["alt+r"]),
    bind("delete", "delete selected", "edit", Action::Delete, &["delete", "forwarddelete"]),
    bind("undo", "undo", "edit", Action::Undo, &["cmd+z"]),
    bind("redo", "redo", "edit", Action::Redo, &["cmd+shift+z"]),
    bind("new", "new project", "file", Action::New, &["cmd+n"]),
    bind("open", "open project", "file", Action::Open, &["cmd+o"]),
    bind("save", "save", "file", Action::Save, &["cmd+s"]),
    bind("save-as", "save as", "file", Action::SaveAs, &["cmd+shift+s"]),
    bind("export", "export wav", "file", Action::Export, &["cmd+e"]),
    bind("settings", "settings", "file", Action::Settings, &["cmd+comma"]),
];

const KEY_NAMES: &[(Code, &str)] = &[
    (Code::KeyA, "a"), (Code::KeyB, "b"), (Code::KeyC, "c"), (Code::KeyD, "d"), (Code::KeyE, "e"),
    (Code::KeyF, "f"), (Code::KeyG, "g"), (Code::KeyH, "h"), (Code::KeyI, "i"), (Code::KeyJ, "j"),
    (Code::KeyK, "k"), (Code::KeyL, "l"), (Code::KeyM, "m"), (Code::KeyN, "n"), (Code::KeyO, "o"),
    (Code::KeyP, "p"), (Code::KeyQ, "q"), (Code::KeyR, "r"), (Code::KeyS, "s"), (Code::KeyT, "t"),
    (Code::KeyU, "u"), (Code::KeyV, "v"), (Code::KeyW, "w"), (Code::KeyX, "x"), (Code::KeyY, "y"),
    (Code::KeyZ, "z"),
    (Code::Digit0, "0"), (Code::Digit1, "1"), (Code::Digit2, "2"), (Code::Digit3, "3"), (Code::Digit4, "4"),
    (Code::Digit5, "5"), (Code::Digit6, "6"), (Code::Digit7, "7"), (Code::Digit8, "8"), (Code::Digit9, "9"),
    (Code::ArrowLeft, "left"), (Code::ArrowRight, "right"), (Code::ArrowUp, "up"), (Code::ArrowDown, "down"),
    (Code::Enter, "enter"), (Code::Space, "space"), (Code::Escape, "escape"), (Code::Tab, "tab"),
    // Named after the Mac keys: "delete" is the key above Return, which reports
    // Backspace; fn+delete reports Delete.
    (Code::Backspace, "delete"), (Code::Delete, "forwarddelete"), (Code::Home, "home"), (Code::End, "end"),
    (Code::PageUp, "pageup"), (Code::PageDown, "pagedown"),
    (Code::Minus, "minus"), (Code::Equal, "equal"), (Code::BracketLeft, "bracketleft"),
    (Code::BracketRight, "bracketright"), (Code::Backslash, "backslash"), (Code::Semicolon, "semicolon"),
    (Code::Quote, "quote"), (Code::Backquote, "backquote"), (Code::Comma, "comma"), (Code::Period, "period"),
    (Code::Slash, "slash"),
    (Code::F1, "f1"), (Code::F2, "f2"), (Code::F3, "f3"), (Code::F4, "f4"), (Code::F5, "f5"), (Code::F6, "f6"),
    (Code::F7, "f7"), (Code::F8, "f8"), (Code::F9, "f9"), (Code::F10, "f10"), (Code::F11, "f11"), (Code::F12, "f12"),
];

/// macOS virtual key codes (`kVK_*`) for the keys in `KEY_NAMES`.
#[cfg(target_os = "macos")]
#[rustfmt::skip]
const MAC_KEY_CODES: &[(u16, Code)] = &[
    (0x00, Code::KeyA), (0x0B, Code::KeyB), (0x08, Code::KeyC), (0x02, Code::KeyD), (0x0E, Code::KeyE),
    (0x03, Code::KeyF), (0x05, Code::KeyG), (0x04, Code::KeyH), (0x22, Code::KeyI), (0x26, Code::KeyJ),
    (0x28, Code::KeyK), (0x25, Code::KeyL), (0x2E, Code::KeyM), (0x2D, Code::KeyN), (0x1F, Code::KeyO),
    (0x23, Code::KeyP), (0x0C, Code::KeyQ), (0x0F, Code::KeyR), (0x01, Code::KeyS), (0x11, Code::KeyT),
    (0x20, Code::KeyU), (0x09, Code::KeyV), (0x0D, Code::KeyW), (0x07, Code::KeyX), (0x10, Code::KeyY),
    (0x06, Code::KeyZ),
    (0x1D, Code::Digit0), (0x12, Code::Digit1), (0x13, Code::Digit2), (0x14, Code::Digit3), (0x15, Code::Digit4),
    (0x17, Code::Digit5), (0x16, Code::Digit6), (0x1A, Code::Digit7), (0x1C, Code::Digit8), (0x19, Code::Digit9),
    (0x7B, Code::ArrowLeft), (0x7C, Code::ArrowRight), (0x7E, Code::ArrowUp), (0x7D, Code::ArrowDown),
    (0x24, Code::Enter), (0x4C, Code::Enter), (0x31, Code::Space), (0x35, Code::Escape), (0x30, Code::Tab),
    (0x33, Code::Backspace), (0x75, Code::Delete), (0x73, Code::Home), (0x77, Code::End),
    (0x74, Code::PageUp), (0x79, Code::PageDown),
    (0x1B, Code::Minus), (0x18, Code::Equal), (0x21, Code::BracketLeft), (0x1E, Code::BracketRight),
    (0x2A, Code::Backslash), (0x29, Code::Semicolon), (0x27, Code::Quote), (0x32, Code::Backquote),
    (0x2B, Code::Comma), (0x2F, Code::Period), (0x2C, Code::Slash),
    (0x7A, Code::F1), (0x78, Code::F2), (0x63, Code::F3), (0x76, Code::F4), (0x60, Code::F5), (0x61, Code::F6),
    (0x62, Code::F7), (0x64, Code::F8), (0x65, Code::F9), (0x6D, Code::F10), (0x67, Code::F11), (0x6F, Code::F12),
];

/// A key press from a plugin editor window as a key event for the app. Only
/// the physical key is known, so panel keys that read the typed character
/// do not apply.
#[cfg(target_os = "macos")]
pub fn plugin_key_event(press: daw_plugins::KeyPress) -> Option<Event> {
    let code = MAC_KEY_CODES.iter().find(|(k, _)| *k == press.key_code)?.1;
    let mut modifiers = Modifiers::empty();
    for (on, modifier) in [(press.cmd, Modifiers::LOGO), (press.ctrl, Modifiers::CTRL), (press.alt, Modifiers::ALT), (press.shift, Modifiers::SHIFT)] {
        modifiers.set(modifier, on);
    }
    Some(Event::KeyPressed {
        key: Key::Unidentified,
        modified_key: Key::Unidentified,
        physical_key: Physical::Code(code),
        location: Location::Standard,
        modifiers,
        text: None,
        repeat: press.repeat,
    })
}

/// A physical key with exact modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub code: Code,
    pub cmd: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Chord {
    pub fn from_event(event: &Event) -> Option<Chord> {
        let Event::KeyPressed { physical_key: Physical::Code(code), modifiers, .. } = event else { return None };
        KEY_NAMES.iter().any(|(c, _)| c == code).then(|| Chord::new(*code, *modifiers))
    }

    fn new(code: Code, modifiers: Modifiers) -> Chord {
        Chord { code, cmd: modifiers.logo(), ctrl: modifiers.control(), alt: modifiers.alt(), shift: modifiers.shift() }
    }

    /// Parse `cmd+ctrl+alt+shift+key`; modifiers in any order.
    pub fn parse(text: &str) -> Option<Chord> {
        let mut chord = Chord { code: Code::Space, cmd: false, ctrl: false, alt: false, shift: false };
        let mut key = None;
        for part in text.split('+').map(|p| p.trim().to_lowercase()) {
            match part.as_str() {
                "cmd" | "super" | "logo" => chord.cmd = true,
                "ctrl" | "control" => chord.ctrl = true,
                "alt" | "option" => chord.alt = true,
                "shift" => chord.shift = true,
                "backspace" if key.is_none() => key = Some(Code::Backspace),
                name if key.is_none() => key = Some(KEY_NAMES.iter().find(|(_, n)| *n == name)?.0),
                _ => return None,
            }
        }
        chord.code = key?;
        Some(chord)
    }

    /// Only Shift or nothing held: the key would type text.
    fn types_text(self) -> bool {
        !self.cmd && !self.ctrl && !self.alt
    }
}

impl std::fmt::Display for Chord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (on, name) in [(self.cmd, "cmd"), (self.ctrl, "ctrl"), (self.alt, "alt"), (self.shift, "shift")] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        let key = KEY_NAMES.iter().find(|(c, _)| *c == self.code).map(|(_, n)| *n).unwrap_or("?");
        f.write_str(key)
    }
}

fn binding(id: &str) -> Option<&'static Binding> {
    BINDINGS.iter().find(|b| b.id == id)
}

/// Parse a default chord. Defaults are written with Cmd, which is Ctrl on
/// Linux; saved chords keep whatever the user picked.
fn default_chord(text: &str) -> Option<Chord> {
    let mut chord = Chord::parse(text)?;
    if cfg!(not(target_os = "macos")) && chord.cmd {
        chord.cmd = false;
        chord.ctrl = true;
    }
    Some(chord)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Keymap {
    /// Chords per binding id, for every entry in `BINDINGS`.
    chords: BTreeMap<&'static str, Vec<Chord>>,
}

impl Default for Keymap {
    fn default() -> Self {
        let chords = BINDINGS.iter().map(|b| (b.id, b.defaults.iter().filter_map(|d| default_chord(d)).collect())).collect();
        Keymap { chords }
    }
}

impl Keymap {
    /// Build from saved config. Actions missing from the config keep their
    /// defaults; unknown actions and unreadable chords come back as warnings.
    pub fn from_config(config: &BTreeMap<String, Vec<String>>) -> (Keymap, Vec<String>) {
        let mut keymap = Keymap::default();
        let mut warnings = Vec::new();
        for (id, chords) in config {
            let Some(binding) = binding(id) else {
                warnings.push(format!("unknown action \"{id}\""));
                continue;
            };
            let mut parsed = Vec::new();
            for text in chords {
                match Chord::parse(text) {
                    Some(chord) => parsed.push(chord),
                    None => warnings.push(format!("unreadable key \"{text}\" for {id}")),
                }
            }
            keymap.chords.insert(binding.id, parsed);
        }
        (keymap, warnings)
    }

    pub fn to_config(&self) -> BTreeMap<String, Vec<String>> {
        self.chords.iter().map(|(id, chords)| (id.to_string(), chords.iter().map(Chord::to_string).collect())).collect()
    }

    pub fn chords(&self, id: &str) -> &[Chord] {
        self.chords.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn is_default(&self, id: &str) -> bool {
        let defaults: Vec<Chord> = binding(id).map(|b| b.defaults.iter().filter_map(|d| default_chord(d)).collect()).unwrap_or_default();
        self.chords(id) == defaults.as_slice()
    }

    /// Bind `chord` to `id`, taking it from any other action. Returns the
    /// action it was taken from.
    pub fn assign(&mut self, id: &str, chord: Chord) -> Option<&'static str> {
        let binding = binding(id)?;
        let mut taken = None;
        for (other, chords) in &mut self.chords {
            if *other != binding.id && chords.contains(&chord) {
                chords.retain(|c| *c != chord);
                taken = Some(*other);
            }
        }
        let chords = self.chords.entry(binding.id).or_default();
        if !chords.contains(&chord) {
            chords.push(chord);
        }
        taken
    }

    pub fn remove(&mut self, id: &str, chord: Chord) {
        if let Some(chords) = self.chords.get_mut(id) {
            chords.retain(|c| *c != chord);
        }
    }

    /// Restore one action's defaults, taking them from other actions.
    pub fn reset(&mut self, id: &str) {
        let Some(binding) = binding(id) else { return };
        self.chords.insert(binding.id, Vec::new());
        for chord in binding.defaults.iter().filter_map(|d| default_chord(d)) {
            self.assign(binding.id, chord);
        }
    }

    /// Map a key press to an action. `typing` is true when a text field has
    /// the event; chords that would type text are then left to the field.
    pub fn action(&self, event: &Event, typing: bool) -> Option<Action> {
        let chord = Chord::from_event(event)?;
        if typing && chord.types_text() {
            return None;
        }
        let id = self.chords.iter().find(|(_, chords)| chords.contains(&chord)).map(|(id, _)| *id)?;
        binding(id).map(|b| b.action)
    }
}

pub fn label(id: &str) -> &'static str {
    binding(id).map(|b| b.label).unwrap_or("unknown")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_defaults_use_control_for_app_commands() {
        let mut keymap = Keymap::default();
        assert_eq!(keymap.chords("undo"), &[Chord::parse("ctrl+z").unwrap()]);
        keymap.remove("undo", Chord::parse("ctrl+z").unwrap());
        keymap.reset("undo");
        assert_eq!(keymap.chords("undo"), &[Chord::parse("ctrl+z").unwrap()]);
        assert!(keymap.is_default("undo"));
    }

    #[test]
    fn chords_round_trip_and_defaults_parse() {
        for binding in BINDINGS {
            for text in binding.defaults {
                let chord = Chord::parse(text).unwrap_or_else(|| panic!("{text}"));
                assert_eq!(chord.to_string(), *text);
            }
        }
        assert_eq!(Chord::parse("Shift+Option+H").unwrap().to_string(), "alt+shift+h");
        assert!(Chord::parse("alt+nope").is_none());
        assert_eq!(Chord::parse("backspace"), Chord::parse("delete"));
        assert!(Chord::parse("alt+h+j").is_none());
    }

    #[test]
    fn assign_moves_chord_and_config_round_trips() {
        let mut keymap = Keymap::default();
        let chord = Chord::parse("alt+h").unwrap();
        assert_eq!(keymap.assign("zoom", chord), Some("focus-left"));
        assert_eq!(keymap.chords("focus-left"), &[Chord::parse("alt+left").unwrap()]);
        assert!(!keymap.is_default("zoom"));
        let (loaded, warnings) = Keymap::from_config(&keymap.to_config());
        assert!(warnings.is_empty());
        assert_eq!(loaded, keymap);
        keymap.reset("focus-left");
        assert!(keymap.is_default("focus-left"));
        assert_eq!(keymap.chords("zoom"), &[Chord::parse("alt+f").unwrap()]);
    }

    #[test]
    fn missing_actions_keep_defaults_and_bad_entries_warn() {
        let mut config = BTreeMap::new();
        config.insert("undo".to_string(), vec!["ctrl+z".to_string(), "bogus".to_string()]);
        config.insert("fly".to_string(), vec!["alt+x".to_string()]);
        config.insert("stop".to_string(), vec![]);
        let (keymap, warnings) = Keymap::from_config(&config);
        assert_eq!(warnings.len(), 2);
        assert_eq!(keymap.chords("undo"), &[Chord::parse("ctrl+z").unwrap()]);
        assert!(keymap.chords("stop").is_empty());
        assert!(keymap.is_default("redo"));
    }
}
