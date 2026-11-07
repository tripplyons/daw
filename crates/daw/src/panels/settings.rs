//! Settings: MIDI input, autosave, render tails, and key bindings. Every
//! change except the tails saves to the config file; the tails belong to the
//! project.

use daw_model::RenderSettings;
use rune::{El, col, row, text};

use super::{label, pick_sized, tool};
use crate::app::{App, Message as AppMessage};
use crate::input::Event;
use crate::keys::{self, BINDINGS, Chord};
use crate::theme::{self, C};
use crate::ui::widgets::{self, Btn, Look};

#[derive(Debug, Default)]
pub struct State {
    /// Settings is showing over the workspace.
    pub open: bool,
    /// Action waiting for its new key chord.
    pub capturing: Option<&'static str>,
    /// Tail length being typed, before it is checked.
    pub tail_text: Option<(Tail, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tail {
    Export,
    Consolidation,
}

impl Tail {
    fn name(self) -> &'static str {
        match self {
            Tail::Export => "export tail",
            Tail::Consolidation => "consolidation tail",
        }
    }

    fn seconds(self, settings: &mut RenderSettings) -> &mut f64 {
        match self {
            Tail::Export => &mut settings.export_tail_seconds,
            Tail::Consolidation => &mut settings.consolidation_tail_seconds,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    MidiPort(crate::midi::Port),
    MidiRescan,
    Autosave(u64),
    UiScale(u16),
    TailText(Tail, String),
    TailDone(Tail),
    Capture(&'static str),
    Cancel,
    Remove(&'static str, Chord),
    Reset(&'static str),
    ResetAll,
}

impl From<Message> for AppMessage {
    fn from(message: Message) -> Self {
        AppMessage::Settings(message)
    }
}

pub fn update(app: &mut App, message: Message) {
    match message {
        Message::MidiRescan => {
            if let Err(error) = app.midi.rescan() { app.set_status(format!("MIDI scan failed: {error}")); }
        }
        Message::MidiPort(port) => {
            app.poll_midi();
            app.finish_midi();
            let name = port.index.map(|_| port.name.clone());
            if app.connect_midi(port) {
                app.config.midi_input = name;
                app.save_config("MIDI input changed".into());
            }
        }
        Message::Autosave(minutes) => {
            app.config.autosave_minutes = minutes;
            app.save_config(if minutes == 0 { "autosave off".into() } else { format!("autosave every {minutes} minutes") });
        }
        Message::UiScale(scale) => {
            if !crate::config::UI_SCALES.contains(&scale) { return; }
            // Window and cursor coordinates are in the scaled UI's logical pixels.
            let ratio = f32::from(app.config.ui_scale) / f32::from(scale);
            app.window.width *= ratio;
            app.window.height *= ratio;
            app.cursor.x *= ratio;
            app.cursor.y *= ratio;
            app.menu = None;
            app.config.ui_scale = scale;
            app.save_config(format!("UI scale {scale}%"));
        }
        Message::TailText(tail, value) => app.settings.tail_text = Some((tail, value)),
        Message::TailDone(tail) => {
            let Some((typed, text)) = app.settings.tail_text.as_ref() else { return };
            if *typed != tail { return; }
            let Ok(value) = text.trim().parse::<f64>() else { app.set_status(format!("{} must be a number", tail.name())); return };
            let value = match RenderSettings::check_tail(tail.name(), value) {
                Ok(value) => value,
                Err(error) => { app.set_status(error); return; }
            };
            if *tail.seconds(&mut app.project.render) != value {
                app.checkpoint();
                *tail.seconds(&mut app.project.render) = value;
                app.mark_edited();
            }
            app.settings.tail_text = None;
        }
        Message::Capture(id) => app.settings.capturing = Some(id),
        Message::Cancel => app.settings.capturing = None,
        Message::Remove(id, chord) => {
            app.keymap.remove(id, chord);
            app.save_config(format!("removed {chord} from {}", keys::label(id)));
        }
        Message::Reset(id) => {
            app.keymap.reset(id);
            app.save_config(format!("reset {}", keys::label(id)));
        }
        Message::ResetAll => {
            app.keymap = keys::Keymap::default();
            app.settings.capturing = None;
            app.save_config("reset all key bindings".into());
        }
    }
}

/// Handle a key press while capturing. Escape alone cancels; modifier keys
/// on their own wait for the rest of the chord.
pub fn key(app: &mut App, event: &Event) {
    let Some(id) = app.settings.capturing else { return };
    let Some(chord) = Chord::from_event(event) else { return };
    if chord == Chord::parse("escape").expect("valid chord") {
        app.settings.capturing = None;
        return;
    }
    app.settings.capturing = None;
    let status = match app.keymap.assign(id, chord) {
        Some(from) => format!("{chord} moved from {} to {}", keys::label(from), keys::label(id)),
        None => format!("{chord} bound to {}", keys::label(id)),
    };
    app.save_config(status);
}

/// The shared wheel scheme in `timeline::Wheel`, for reference.
const SCROLLING: &[(&str, &str)] = &[
    ("Scroll", "Scroll up and down"),
    ("Shift+Scroll, or swipe sideways", "Scroll sideways"),
    (if cfg!(target_os = "macos") { "Cmd+Scroll" } else { "Ctrl+Scroll" }, "Zoom time"),
    ("Alt+Scroll", "Zoom height (keys, tracks, values)"),
    ("Alt+Shift+Scroll over a note", "Change note velocity"),
];

pub fn toolbar(app: &App) -> El {
    super::bar(vec![tool("Reset all keys", Message::ResetAll.into()), label(app.config_path.display())])
}

/// Width of the name column in the settings form.
const NAME_WIDTH: f32 = 190.0;

/// A form row: a name on the left, its controls on the right.
fn form_row(name: &str, control: El) -> El {
    row().w_full().min_h(34.0).gap(12.0).center_y()
        .child(text(name.to_string()).font(theme::BODY).color(C.text_dim).w(NAME_WIDTH).single_line())
        .child(control)
}

fn heading(name: &str) -> El {
    col().w_full().pt(18.0).pb(6.0).gap(6.0)
        .child(widgets::section(name))
        .child(widgets::rule())
}

fn tail_field(app: &App, tail: Tail, value: f64) -> El {
    let shown = app.settings.tail_text.as_ref().filter(|(typed, _)| *typed == tail).map_or_else(|| value.to_string(), |(_, text)| text.clone());
    let field = match tail {
        Tail::Export => widgets::field(app, "tail-export", &shown, "Seconds", |s| Message::TailText(Tail::Export, s).into(), Some(Message::TailDone(Tail::Export).into())),
        Tail::Consolidation => widgets::field(app, "tail-consolidation", &shown, "Seconds", |s| Message::TailText(Tail::Consolidation, s).into(), Some(Message::TailDone(Tail::Consolidation).into())),
    };
    row().gap(8.0).center_y()
        .child(field.w(80.0))
        .child(label(format!("seconds, up to {}", RenderSettings::MAX_TAIL_SECONDS)))
}

fn binding_row(app: &App, id: &'static str, name: &'static str) -> El {
    let mut chords = row().gap(4.0).center_y().flex_wrap();
    for &chord in app.keymap.chords(id) {
        chords = chords.child(Btn::new(keys::pretty(&chord.to_string())).trailing("×").height(22.0)
            .help(format!("Remove {chord}")).build(Message::Remove(id, chord)));
    }
    let capturing = app.settings.capturing == Some(id);
    chords = if capturing {
        chords.child(Btn::new("Press keys…").look(Look::On).height(22.0).help("Press the new key combination. Escape cancels.").build(Message::Cancel))
    } else {
        chords.child(Btn::new("").icon(&crate::ui::icons::PLUS).ghost().height(22.0).help("Add a key").build(Message::Capture(id)))
    };
    let mut line = row().w_full().min_h(30.0).gap(12.0).center_y().px(6.0).radius(theme::RADIUS).hover(C.hover)
        .child(text(super::sentence(name)).font(theme::BODY).color(C.text).w(NAME_WIDTH - 6.0).single_line().truncate())
        .child(chords.grow(1.0).shrink().min_w(0.0));
    if !app.keymap.is_default(id) {
        line = line.child(Btn::new("Reset").ghost().height(22.0).help("Restore the default keys").build(Message::Reset(id)));
    }
    line
}

pub fn view(app: &App) -> El {
    let mut list = col().w_full().px(20.0).pb(20.0);
    list = list.child(heading("General"))
        .child(form_row("Interface size", row().gap(8.0).center_y()
            .child(pick_sized("ui-scale", crate::config::UI_SCALES.iter().map(|s| format!("{s}%")).collect(), Some(format!("{}%", app.config.ui_scale)), "Scale", 80.0,
                |s: String| Message::UiScale(s.trim_end_matches('%').parse().unwrap_or(100)).into()))))
        .child(form_row("Autosave", pick_sized("autosave", vec!["Off".to_string(), "Every minute".into(), "Every 2 minutes".into(), "Every 5 minutes".into(), "Every 10 minutes".into()],
            Some(match app.config.autosave_minutes { 0 => "Off".to_string(), 1 => "Every minute".into(), m => format!("Every {m} minutes") }), "Autosave", 150.0,
            |s: String| Message::Autosave(match s.as_str() { "Off" => 0, "Every minute" => 1, s => s.split(' ').nth(1).and_then(|n| n.parse().ok()).unwrap_or(5) }).into())));
    list = list.child(heading("MIDI"))
        .child(form_row("Input device", row().gap(8.0).center_y()
            .child(pick_sized("midi-port", app.midi.ports.clone(), app.midi.selected.clone(), "No device", 220.0, |p| Message::MidiPort(p).into()))
            .child(Btn::new("Rescan").ghost().help("Look for MIDI devices again").build(Message::MidiRescan))));
    list = list.child(heading("Rendering (saved with the project)"))
        .child(form_row("Export tail", tail_field(app, Tail::Export, app.project.render.export_tail_seconds)))
        .child(form_row("Consolidation tail", tail_field(app, Tail::Consolidation, app.project.render.consolidation_tail_seconds)));
    if let Some(error) = &app.config_error {
        list = list.child(row().pt(10.0).child(text(error.clone()).font(theme::BODY).color(C.danger).wrap()));
    }
    list = list.child(row().w_full().center_y().child(heading("Key bindings").grow(1.0))
        .child(Btn::new("Reset all").ghost().height(22.0).help("Restore every default key binding").build(Message::ResetAll).mt(12.0)));
    let mut group = "";
    for binding in BINDINGS {
        if binding.group != group {
            group = binding.group;
            list = list.child(row().pt(10.0).pb(2.0).px(6.0).child(text(super::sentence(group)).font(theme::LABEL).color(C.text_faint)));
        }
        list = list.child(binding_row(app, binding.id, binding.label));
    }
    // Scrolling is fixed; list it next to the editable keys.
    list = list.child(heading("Scrolling"));
    for (combo, action) in SCROLLING {
        list = list.child(row().w_full().min_h(26.0).gap(12.0).center_y().px(6.0)
            .child(text(*action).font(theme::BODY).color(C.text).w(NAME_WIDTH - 6.0))
            .child(text(*combo).font(theme::LABEL).color(C.text_dim)));
    }
    let command = if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" };
    let note = format!("Keys match by physical position on any layout. Panel keys (Delete, arrows, \
                {command}+A/C/V/D, 1-6 in automation, Q in the piano roll) apply when no binding matches.");
    list = list.child(row().pt(14.0).child(text(note).font(theme::LABEL).color(C.text_faint).wrap()));
    super::scroll(list, true, false)
}
