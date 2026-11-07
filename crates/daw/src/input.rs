//! Keyboard events in the shape the key bindings, musical typing, and tests
//! use: a pressed or released key, its physical position, and modifiers.
//! The UI layer translates Rune's key events into these.

pub mod key {
    /// A key by its physical position on a US layout. Tests and saved
    /// bindings name modifier keys the UI layer never reports alone.
    #[allow(dead_code)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
    pub enum Code {
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM,
        KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
        Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9,
        ArrowLeft, ArrowRight, ArrowUp, ArrowDown,
        Enter, Space, Escape, Tab, Backspace, Delete, Home, End, PageUp, PageDown,
        Minus, Equal, BracketLeft, BracketRight, Backslash, Semicolon, Quote, Backquote,
        Comma, Period, Slash,
        F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
        ShiftLeft, ShiftRight, ControlLeft, ControlRight, AltLeft, AltRight, SuperLeft, SuperRight,
    }

    /// Where a key event came from.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum Physical {
        Code(Code),
        Unidentified(NativeCode),
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum NativeCode {
        Unidentified,
    }

    /// Keys that do not type a character.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum Named {
        ArrowLeft, ArrowRight, ArrowUp, ArrowDown,
        Enter, Space, Escape, Tab, Backspace, Delete, Home, End, PageUp, PageDown,
        F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
    }
}

use key::{Code, Named, Physical};

/// The logical key: what the key means under the current layout.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    Named(Named),
    Character(String),
    Unidentified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Location {
    Standard,
}

/// Held modifier keys. `COMMAND` is Cmd on macOS and Ctrl elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const SHIFT: Modifiers = Modifiers(1);
    pub const CTRL: Modifiers = Modifiers(2);
    pub const ALT: Modifiers = Modifiers(4);
    pub const LOGO: Modifiers = Modifiers(8);
    pub const COMMAND: Modifiers = if cfg!(target_os = "macos") { Self::LOGO } else { Self::CTRL };

    pub const fn empty() -> Self {
        Modifiers(0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(self, other: Modifiers) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn shift(self) -> bool {
        self.contains(Self::SHIFT)
    }

    pub const fn control(self) -> bool {
        self.contains(Self::CTRL)
    }

    pub const fn alt(self) -> bool {
        self.contains(Self::ALT)
    }

    pub const fn logo(self) -> bool {
        self.contains(Self::LOGO)
    }

    pub const fn command(self) -> bool {
        self.contains(Self::COMMAND)
    }

    pub fn insert(&mut self, other: Modifiers) {
        self.0 |= other.0;
    }

    pub fn set(&mut self, other: Modifiers, on: bool) {
        if on { self.0 |= other.0 } else { self.0 &= !other.0 }
    }
}

impl std::ops::BitOr for Modifiers {
    type Output = Modifiers;
    fn bitor(self, rhs: Modifiers) -> Modifiers {
        Modifiers(self.0 | rhs.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    KeyPressed {
        key: Key,
        modified_key: Key,
        physical_key: Physical,
        location: Location,
        modifiers: Modifiers,
        text: Option<String>,
        repeat: bool,
    },
    KeyReleased {
        key: Key,
        modified_key: Key,
        physical_key: Physical,
        location: Location,
        modifiers: Modifiers,
    },
    ModifiersChanged(Modifiers),
}

impl Event {
    pub fn pressed(code: Option<Code>, key: Key, modifiers: Modifiers, text: Option<String>) -> Event {
        Event::KeyPressed {
            modified_key: key.clone(),
            key,
            physical_key: code.map_or(Physical::Unidentified(key::NativeCode::Unidentified), Physical::Code),
            location: Location::Standard,
            modifiers,
            text,
            repeat: false,
        }
    }

    pub fn released(code: Option<Code>, key: Key, modifiers: Modifiers) -> Event {
        Event::KeyReleased {
            modified_key: key.clone(),
            key,
            physical_key: code.map_or(Physical::Unidentified(key::NativeCode::Unidentified), Physical::Code),
            location: Location::Standard,
            modifiers,
        }
    }
}

/// Mouse wheel movement over a timeline, in lines or pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScrollDelta {
    Lines { x: f32, y: f32 },
    Pixels { x: f32, y: f32 },
}

/// A point in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const ORIGIN: Point = Point { x: 0.0, y: 0.0 };

    pub const fn new(x: f32, y: f32) -> Self {
        Point { x, y }
    }

    pub fn distance(self, other: Point) -> f32 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }
}

impl std::ops::Sub for Point {
    type Output = Point;
    fn sub(self, other: Point) -> Point {
        Point::new(self.x - other.x, self.y - other.y)
    }
}

/// A size in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub const fn new(width: f32, height: f32) -> Self {
        Size { width, height }
    }
}

/// A rectangle in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rectangle {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rectangle {
    pub const fn with_size(size: Size) -> Self {
        Rectangle { x: 0.0, y: 0.0, width: size.width, height: size.height }
    }

    pub const fn position(&self) -> Point {
        Point { x: self.x, y: self.y }
    }

    pub const fn size(&self) -> Size {
        Size { width: self.width, height: self.height }
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.x + self.width && p.y >= self.y && p.y < self.y + self.height
    }
}

/// Map a Rune physical key to the position the key bindings name.
pub fn code_from_rune(code: rune::PhysicalKeyCode) -> Option<Code> {
    use rune::PhysicalKeyCode as P;
    Some(match code {
        P::KeyA => Code::KeyA, P::KeyB => Code::KeyB, P::KeyC => Code::KeyC, P::KeyD => Code::KeyD,
        P::KeyE => Code::KeyE, P::KeyF => Code::KeyF, P::KeyG => Code::KeyG, P::KeyH => Code::KeyH,
        P::KeyI => Code::KeyI, P::KeyJ => Code::KeyJ, P::KeyK => Code::KeyK, P::KeyL => Code::KeyL,
        P::KeyM => Code::KeyM, P::KeyN => Code::KeyN, P::KeyO => Code::KeyO, P::KeyP => Code::KeyP,
        P::KeyQ => Code::KeyQ, P::KeyR => Code::KeyR, P::KeyS => Code::KeyS, P::KeyT => Code::KeyT,
        P::KeyU => Code::KeyU, P::KeyV => Code::KeyV, P::KeyW => Code::KeyW, P::KeyX => Code::KeyX,
        P::KeyY => Code::KeyY, P::KeyZ => Code::KeyZ,
        P::Digit0 => Code::Digit0, P::Digit1 => Code::Digit1, P::Digit2 => Code::Digit2,
        P::Digit3 => Code::Digit3, P::Digit4 => Code::Digit4, P::Digit5 => Code::Digit5,
        P::Digit6 => Code::Digit6, P::Digit7 => Code::Digit7, P::Digit8 => Code::Digit8,
        P::Digit9 => Code::Digit9,
        P::ArrowLeft => Code::ArrowLeft, P::ArrowRight => Code::ArrowRight,
        P::ArrowUp => Code::ArrowUp, P::ArrowDown => Code::ArrowDown,
        P::Enter => Code::Enter, P::Space => Code::Space, P::Escape => Code::Escape, P::Tab => Code::Tab,
        P::Backspace => Code::Backspace, P::Delete => Code::Delete, P::Home => Code::Home, P::End => Code::End,
        P::PageUp => Code::PageUp, P::PageDown => Code::PageDown,
        P::Minus => Code::Minus, P::Equal => Code::Equal, P::BracketLeft => Code::BracketLeft,
        P::BracketRight => Code::BracketRight, P::Backslash => Code::Backslash, P::Semicolon => Code::Semicolon,
        P::Quote => Code::Quote, P::Backquote => Code::Backquote, P::Comma => Code::Comma,
        P::Period => Code::Period, P::Slash => Code::Slash,
        P::F1 => Code::F1, P::F2 => Code::F2, P::F3 => Code::F3, P::F4 => Code::F4, P::F5 => Code::F5,
        P::F6 => Code::F6, P::F7 => Code::F7, P::F8 => Code::F8, P::F9 => Code::F9, P::F10 => Code::F10,
        P::F11 => Code::F11, P::F12 => Code::F12,
        _ => return None,
    })
}
