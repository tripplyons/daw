//! Headless UI snapshots for design review, driven by a small script.
//!
//! ```sh
//! DAW_SNAPSHOT_SCRIPT=shots.txt DAW_SNAPSHOT_DIR=out \
//!     cargo test --release -p daw ui_snapshots -- --ignored --nocapture
//! ```
//!
//! Each script line is one step; `#` starts a comment:
//!
//! - `reset`: start over with a fresh app
//! - `open PATH`: open a project
//! - `size W H`: set the window size in logical pixels
//! - `catalog`: load the plugin scan cache into the browser
//! - `click TEXT`: click the widget showing TEXT
//! - `click X Y`, `rclick X Y`, `hover X Y`, `drag X1 Y1 X2 Y2`
//! - `press X Y`, `release X Y`: the halves of a left click, for shots mid-drag
//! - `scroll X Y DY`: scroll DY lines at a point
//! - `key KEY [cmd|shift|alt|ctrl]...`: KEY is a character or Escape, Enter, Tab, Delete, Space, F10
//! - `type TEXT`: type characters
//! - `shot NAME`: save `NAME.png` in the output directory
//! - `begin` ... `end`: run the steps between in one simulator, so widget
//!   state such as an open pick list lasts until `end`
//!
//! The simulator runs no subscriptions or tasks, so the harness forwards the
//! events the app's subscription listens to, and async work such as plugin
//! scans stays pending.

use std::path::{Path, PathBuf};

use iced::keyboard::key::Named;
use iced::{Point, Size, mouse};
use iced_test::Simulator;

use super::*;

struct Harness {
    app: App,
    size: Size,
    cursor: Option<Point>,
    out: PathBuf,
}

/// One input or capture inside a simulator run.
enum Act {
    Events(Vec<iced::Event>),
    ClickText(String, mouse::Button),
    Shot(String),
}

impl Harness {
    fn new(out: PathBuf) -> Self {
        let mut app = app();
        app.scan = None;
        let size = Size::new(1400.0, 860.0);
        app.window = size;
        Harness { app, size, cursor: None, out }
    }

    /// Run acts in one simulator, so widget state such as a pressed button
    /// or an open pick list lasts for the whole run, then update the app with
    /// the subscription's messages and the widgets' messages.
    fn run(&mut self, acts: Vec<Act>) {
        let settings = iced::Settings { default_text_size: crate::theme::TEXT_SIZE.into(), ..iced::Settings::default() };
        let mut ui = Simulator::with_size(settings, self.size, self.app.view());
        if let Some(cursor) = self.cursor {
            ui.point_at(cursor);
        }
        // Cursor moves and raw presses reach the app before the widgets' messages.
        let (mut before, mut after) = (Vec::new(), Vec::new());
        for act in acts {
            let events = match act {
                Act::Events(events) => events,
                Act::ClickText(text, button) => {
                    let target = ui.find(text.as_str()).unwrap_or_else(|e| panic!("no widget shows {text:?}: {e}"));
                    let bounds = iced_test::selector::Bounded::visible_bounds(&target).unwrap_or_else(|| panic!("{text:?} is not visible"));
                    click(bounds.center(), button)
                }
                Act::Shot(name) => {
                    let snapshot = ui.snapshot(&crate::theme::theme()).expect("snapshot");
                    save(&self.out, &name, &snapshot);
                    continue;
                }
            };
            for event in events {
                if let iced::Event::Mouse(mouse::Event::CursorMoved { position }) = event {
                    self.cursor = Some(position);
                    ui.point_at(position);
                }
                let status = ui.simulate([event.clone()]);
                let captured = status.first().is_some_and(|s| *s == iced::event::Status::Captured);
                match event {
                    iced::Event::Keyboard(key) => after.push(Message::Key(key, captured)),
                    iced::Event::Mouse(mouse::Event::CursorMoved { position }) => before.push(Message::MouseMoved(position)),
                    iced::Event::Mouse(mouse::Event::ButtonPressed(_)) => before.push(Message::MousePressed),
                    iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => after.push(Message::MouseReleased),
                    iced::Event::Mouse(mouse::Event::WheelScrolled { .. }) => after.push(Message::EndEdit),
                    _ => {}
                }
            }
        }
        let produced: Vec<Message> = ui.into_messages().collect();
        for message in before.into_iter().chain(produced).chain(after) {
            let _ = self.app.update(message);
        }
    }

    /// The acts a script line stands for, or `None` after a step that changes
    /// the app directly.
    fn parse(&mut self, line: &str) -> Option<Vec<Act>> {
        let words: Vec<&str> = line.split_whitespace().collect();
        let number = |i: usize| -> f32 { words[i].parse().unwrap_or_else(|_| panic!("bad number in {line:?}")) };
        let point = |i: usize| Point::new(number(i), number(i + 1));
        let acts = match words[0] {
            "reset" => { *self = Harness::new(self.out.clone()); return None; }
            "open" => { self.app.open(PathBuf::from(words[1])); return None; }
            "size" => {
                self.size = Size::new(number(1), number(2));
                let _ = self.app.update(Message::WindowResized(self.size));
                return None;
            }
            "catalog" => {
                let catalog = daw_plugins::scan::cached(&daw_plugins::scan::cache_path());
                let _ = self.app.update(Message::ScanDone(catalog));
                return None;
            }
            "click" | "rclick" => {
                let button = if words[0] == "click" { mouse::Button::Left } else { mouse::Button::Right };
                if words.len() == 3 && words[1].parse::<f32>().is_ok() {
                    vec![Act::Events(click(point(1), button))]
                } else {
                    vec![Act::ClickText(line[words[0].len()..].trim().to_string(), button)]
                }
            }
            "hover" => vec![Act::Events(vec![moved(point(1))])],
            "press" => vec![Act::Events(vec![moved(point(1)), iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))])],
            "release" => vec![Act::Events(vec![moved(point(1)), iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))])],
            "drag" => {
                let (from, to) = (point(1), point(3));
                let mut events = vec![moved(from), iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))];
                for i in 1..=8 {
                    let t = i as f32 / 8.0;
                    events.push(moved(Point::new(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t)));
                }
                events.push(iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)));
                vec![Act::Events(events)]
            }
            "scroll" => vec![Act::Events(vec![
                moved(point(1)),
                iced::Event::Mouse(mouse::Event::WheelScrolled { delta: mouse::ScrollDelta::Lines { x: 0.0, y: number(3) } }),
            ])],
            "key" => {
                let mut modifiers = Modifiers::empty();
                for word in &words[2..] {
                    modifiers |= match *word {
                        "cmd" => Modifiers::COMMAND,
                        "shift" => Modifiers::SHIFT,
                        "alt" => Modifiers::ALT,
                        "ctrl" => Modifiers::CTRL,
                        other => panic!("unknown modifier {other}"),
                    };
                }
                vec![Act::Events(key(words[1], modifiers))]
            }
            "type" => vec![Act::Events(line["type".len()..].trim().chars().flat_map(|c| key(&c.to_string(), Modifiers::empty())).collect())],
            "shot" => vec![Act::Shot(words[1].to_string())],
            other => panic!("unknown step {other:?}"),
        };
        Some(acts)
    }
}

fn moved(position: Point) -> iced::Event {
    iced::Event::Mouse(mouse::Event::CursorMoved { position })
}

fn click(at: Point, button: mouse::Button) -> Vec<iced::Event> {
    vec![moved(at), iced::Event::Mouse(mouse::Event::ButtonPressed(button)), iced::Event::Mouse(mouse::Event::ButtonReleased(button))]
}

fn key(name: &str, modifiers: Modifiers) -> Vec<iced::Event> {
    let (key, text) = match name {
        "Escape" => (Key::Named(Named::Escape), None),
        "Enter" => (Key::Named(Named::Enter), None),
        "Tab" => (Key::Named(Named::Tab), None),
        "Delete" => (Key::Named(Named::Delete), None),
        "Space" => (Key::Named(Named::Space), Some(" ".into())),
        "F10" => (Key::Named(Named::F10), None),
        c => (Key::Character(c.to_lowercase().into()), if modifiers.command() { None } else { Some(c.into()) }),
    };
    let physical = code(name).map_or(Physical::Unidentified(keyboard::key::NativeCode::Unidentified), Physical::Code);
    vec![
        iced::Event::Keyboard(keyboard::Event::KeyPressed {
            key: key.clone(),
            modified_key: key.clone(),
            physical_key: physical,
            location: Location::Standard,
            modifiers,
            text,
            repeat: false,
        }),
        iced::Event::Keyboard(keyboard::Event::KeyReleased { key: key.clone(), modified_key: key, physical_key: physical, location: Location::Standard, modifiers }),
    ]
}

/// The US-layout key code for a step's key name; bindings match codes.
fn code(name: &str) -> Option<Code> {
    use Code::*;
    Some(match name {
        "Escape" => Escape,
        "Enter" => Enter,
        "Tab" => Tab,
        "Delete" => Delete,
        "Space" => Space,
        "F10" => F10,
        "," => Comma,
        "." => Period,
        "/" => Slash,
        "-" => Minus,
        "=" => Equal,
        _ => {
            let c = name.chars().next().filter(|_| name.chars().count() == 1)?.to_ascii_lowercase();
            const LETTERS: [Code; 26] = [KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ];
            const DIGITS: [Code; 10] = [Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9];
            match c {
                'a'..='z' => LETTERS[(c as u8 - b'a') as usize],
                '0'..='9' => DIGITS[(c as u8 - b'0') as usize],
                _ => return None,
            }
        }
    })
}

/// Save `NAME.png`; `matches_image` writes `NAME-RENDERER.png`, and only
/// when that file is missing.
fn save(out: &Path, name: &str, snapshot: &iced_test::simulator::Snapshot) {
    let prefix = format!("{name}-");
    let rendered = || std::fs::read_dir(out).unwrap().flatten().filter(|e| {
        let file = e.file_name().to_string_lossy().into_owned();
        file.starts_with(&prefix) && file.ends_with(".png")
    });
    for entry in rendered() {
        std::fs::remove_file(entry.path()).unwrap();
    }
    assert!(snapshot.matches_image(out.join(name)).expect("write snapshot"));
    let target = out.join(format!("{name}.png"));
    std::fs::rename(rendered().next().unwrap().path(), &target).unwrap();
    println!("wrote {}", target.display());
}

#[test]
#[ignore = "writes PNGs for design review; set DAW_SNAPSHOT_SCRIPT and DAW_SNAPSHOT_DIR"]
fn ui_snapshots() {
    let script = std::env::var("DAW_SNAPSHOT_SCRIPT").expect("set DAW_SNAPSHOT_SCRIPT");
    let out = PathBuf::from(std::env::var("DAW_SNAPSHOT_DIR").expect("set DAW_SNAPSHOT_DIR"));
    std::fs::create_dir_all(&out).unwrap();
    let mut harness = Harness::new(out);
    let mut block: Option<Vec<Act>> = None;
    for line in std::fs::read_to_string(Path::new(&script)).unwrap().lines() {
        let line = line.trim();
        match line {
            "" => {}
            _ if line.starts_with('#') => {}
            "begin" => block = Some(Vec::new()),
            "end" => harness.run(block.take().expect("end without begin")),
            _ => match (harness.parse(line), &mut block) {
                (Some(acts), Some(block)) => block.extend(acts),
                (Some(acts), None) => harness.run(acts),
                (None, Some(_)) => panic!("{line:?} cannot run inside begin/end"),
                (None, None) => {}
            },
        }
    }
}
