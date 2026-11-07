//! Mixer inserts with meters, and the effect chain of the selected insert.

use daw_model::automation::{insert_volume_to_normalized, pan_to_normalized};
use daw_model::{Insert, InsertId, InstanceId, Send, Target};
use std::cell::Cell;

use rune::{El, Tx, col, el, row, spacer, text};

use super::browser::{self, DropZone};
use super::label;
use crate::app::{App, Message as AppMessage};
use crate::input::ScrollDelta;
use crate::menu;
use crate::task::Task;
use crate::theme::{self, C};
use crate::ui::{self, icons, widgets::{self, Btn, Choice, Look, Slider}};
use crate::units::{gain_text, pan_text};

const STRIP_WIDTH: f32 = 76.0;
const STRIPS: &str = "mixer-strips";
/// Width of the selected insert's pane beside the strips.
const PANE_WIDTH: f32 = 236.0;

thread_local! {
    /// How far the strips are scrolled sideways, for wheel steps.
    static SCROLLED: Cell<f32> = const { Cell::new(0.0) };
}

#[derive(Debug, Clone)]
pub enum Message {
    Select(InsertId),
    Volume(InsertId, f32),
    Pan(InsertId, f32),
    Mute(InsertId),
    Solo(InsertId),
    RemoveEffect(InsertId, usize),
    RaiseEffect(InsertId, usize),
    AddInsert,
    DeleteInsert,
    Output(InsertId, InsertId),
    Send(InsertId, InsertId, bool),
    SendLevel(InsertId, InsertId, f32),
    RemoveSend(InsertId, InsertId),
    /// Toggle whether an insert is heard only through its sidechain sends.
    DetectorOnly(InsertId),
    /// A wheel step the strip area did not use: plain up/down scrolling,
    /// turned into sideways scrolling since the strips only scroll sideways.
    Wheel(ScrollDelta),
}

impl From<Message> for AppMessage {
    fn from(message: Message) -> Self {
        AppMessage::Mixer(message)
    }
}

/// Delete the selected insert. Channels on it move to the master, which
/// cannot be deleted.
pub fn delete_selected(app: &mut App) -> bool {
    let id = app.selected_insert;
    if id == daw_model::MASTER || app.project.mixer.insert(id).is_none() {
        return false;
    }
    app.checkpoint();
    let name = app.project.mixer.insert(id).map(|i| i.name.clone()).unwrap_or_default();
    let index = app.project.mixer.inserts.iter().position(|i| i.id == id).unwrap_or(1);
    app.project.remove_insert(id);
    let inserts = &app.project.mixer.inserts;
    app.selected_insert = inserts.get(index).or(inserts.last()).map(|i| i.id).unwrap_or(daw_model::MASTER);
    app.set_status(format!("deleted {name}"));
    app.edited();
    true
}

/// Whether an insert's effects can take a sidechain: `Some(true)` when one
/// has a sidechain input, `Some(false)` when none has, and `None` when a
/// plugin is not loaded, so the answer is unknown.
fn sidechain_ready(app: &App, insert: &Insert) -> Option<bool> {
    let mut unknown = false;
    for &effect in &insert.effects {
        match app.session.sidechain_input(effect) {
            Some(true) => return Some(true),
            Some(false) => {}
            None => unknown = true,
        }
    }
    if unknown { None } else { Some(false) }
}

/// The first effect on an insert with a sidechain input.
fn sidechain_effect(app: &App, insert: &Insert) -> Option<InstanceId> {
    insert.effects.iter().copied().find(|&e| app.session.sidechain_input(e) == Some(true))
}

/// Tell the user what a new sidechain will drive, and what may still be
/// needed: an effect that takes a sidechain, or its sidechain switch.
fn report_sidechain(app: &mut App, from: InsertId, to: InsertId) {
    let (source, destination) = (app.project.insert_label(from), app.project.insert_label(to));
    let Some(insert) = app.project.mixer.insert(to) else { return };
    let ready = sidechain_ready(app, insert);
    match sidechain_effect(app, insert) {
        Some(instance) => {
            let name = app.project.plugin(instance).map(|p| p.plugin.name.clone()).unwrap_or_default();
            let status = format!("{source} drives the sidechain of {name} on {destination}. If {name} has an external sidechain switch, turn it on in its editor.");
            if app.session.editor_open(instance) {
                app.set_status(status);
            } else {
                app.set_status_with(status, "open editor", AppMessage::TogglePlugin(instance));
            }
        }
        None if ready == Some(false) => app.set_status(format!(
            "{source} sidechains {destination}, but no effect on {destination} has a sidechain input. Add one, such as a compressor with an external sidechain."
        )),
        None => app.set_status(format!("{source} sidechains {destination}")),
    }
}

pub fn update(app: &mut App, message: Message) -> Task {
    match message {
        Message::Send(from, to, sidechain) => {
            if !app.project.mixer.can_route(from, to) { app.set_status("that send would create feedback"); return Task::none(); }
            app.checkpoint();
            app.project.mixer.set_send(from, Send { to, level: 1.0, sidechain });
            app.edited();
            if sidechain { report_sidechain(app, from, to); }
        }
        Message::DetectorOnly(id) => {
            app.checkpoint();
            if let Some(insert) = app.project.mixer.insert_mut(id) { insert.detector_only = !insert.detector_only; }
            app.edited();
        }
        Message::SendLevel(from, to, level) => {
            if !Send::LEVEL.contains(&level) { return Task::none(); }
            if app.project.mixer.insert(from).and_then(|i| i.sends.iter().find(|s| s.to == to)).is_none_or(|s| s.level == level) { return Task::none(); }
            app.begin_edit();
            if let Some(send) = app.project.mixer.insert_mut(from).and_then(|i| i.sends.iter_mut().find(|s| s.to == to)) { send.level = level; }
            app.session.set_send_level(&app.project, from, to, level);
            app.mark_edited();
        }
        Message::RemoveSend(from, to) => {
            app.checkpoint();
            if let Some(insert) = app.project.mixer.insert_mut(from) { insert.sends.retain(|s| s.to != to); }
            app.edited();
        }
        Message::Select(id) => app.selected_insert = id,
        Message::Volume(id, volume) => {
            if app.project.mixer.insert(id).is_none_or(|i| i.volume == volume) { return Task::none(); }
            app.begin_edit();
            if let Some(insert) = app.project.mixer.insert_mut(id) {
                insert.volume = volume;
            }
            let value = insert_volume_to_normalized(volume);
            app.session.set_mix(&app.project, Target::InsertVolume(id), value);
            app.mark_edited();
            app.touched(Target::InsertVolume(id), value);
        }
        Message::Pan(id, pan) => {
            if app.project.mixer.insert(id).is_none_or(|i| i.pan == pan) { return Task::none(); }
            app.begin_edit();
            if let Some(insert) = app.project.mixer.insert_mut(id) {
                insert.pan = pan;
            }
            let value = pan_to_normalized(pan);
            app.session.set_mix(&app.project, Target::InsertPan(id), value);
            app.mark_edited();
            app.touched(Target::InsertPan(id), value);
        }
        Message::Mute(id) => {
            app.checkpoint();
            if let Some(insert) = app.project.mixer.insert_mut(id) {
                insert.mute = !insert.mute;
            }
            app.edited();
        }
        Message::Wheel(delta) => {
            let (dx, dy) = super::wheel_lines(delta);
            if dy.abs() > dx.abs() {
                let x = (SCROLLED.get() - dy * 60.0).max(0.0);
                SCROLLED.set(x);
                return Task::scroll_x(STRIPS, x);
            }
        }
        Message::Output(from, to) => {
            if !app.project.mixer.can_route(from, to) {
                app.set_status("that route would feed the insert back into itself");
                return Task::none();
            }
            app.checkpoint();
            app.project.mixer.set_output(from, to);
            app.edited();
        }
        Message::Solo(id) => {
            app.checkpoint();
            if let Some(insert) = app.project.mixer.insert_mut(id) {
                insert.solo = !insert.solo;
            }
            app.edited();
        }
        Message::RemoveEffect(id, index) => {
            let Some(instance) = app.project.mixer.insert(id).and_then(|i| i.effects.get(index).copied()) else { return Task::none() };
            app.checkpoint();
            app.project.remove_plugin(instance);
            app.edited();
        }
        Message::RaiseEffect(id, index) => {
            app.checkpoint();
            if let Some(insert) = app.project.mixer.insert_mut(id)
                && index > 0
                && index < insert.effects.len()
            {
                insert.effects.swap(index, index - 1);
            }
            app.edited();
        }
        Message::DeleteInsert => {
            delete_selected(app);
        }
        Message::AddInsert => {
            app.checkpoint();
            let id = InsertId(app.project.next_id());
            let number = app.project.mixer.inserts.len();
            app.project.mixer.inserts.push(daw_model::Insert::new(id, &format!("insert {number}")));
            app.selected_insert = id;
            app.edited();
        }
    }
    Task::none()
}

/// A label over a picker of inserts to route to or from, with a hint.
fn route_row(key: &str, name: &str, hint: &str, choices: Vec<Choice>) -> El {
    let placeholder = if choices.is_empty() { "None available" } else { "Choose insert" };
    col().gap(4.0).w_full()
        .child(label(name))
        .child(widgets::select_with(key, None, placeholder, choices, PANE_WIDTH - 28.0, Some(hint.to_string())))
}

/// A send's name with a remove button, then its level slider.
fn send_row(name: String, from: InsertId, to: InsertId, level: f32) -> El {
    col().gap(4.0).w_full().padding(8.0).radius(theme::RADIUS).fill(C.well)
        .child(row().gap(4.0).center_y()
            .child(text(name).font(theme::LABEL).color(C.text).single_line().truncate().grow(1.0).shrink())
            .child(Btn::new("").icon(&icons::CLOSE).ghost().height(20.0).help("Remove").build(Message::RemoveSend(from, to))))
        .child(row().gap(8.0).center_y()
            .child(Slider::new(level, (*Send::LEVEL.start(), *Send::LEVEL.end())).default(1.0).width(130.0).color(C.accent).help(gain_text(level))
                .build(move |v| Message::SendLevel(from, to, v).into()))
            .child(text(gain_text(level)).font(theme::NUMBER).color(C.text_dim).single_line()))
}

pub fn toolbar(app: &App) -> El {
    let mut items = vec![Btn::new("Insert").icon(&icons::PLUS).help("Add a mixer insert").build(Message::AddInsert)];
    if app.selected_insert != daw_model::MASTER {
        items.push(Btn::new("Delete insert").build(Message::DeleteInsert));
    }
    super::bar(items)
}

fn strip(app: &App, index: usize, insert: &Insert) -> El {
    let id = insert.id;
    let selected = app.selected_insert == id;
    let (left, right) = app.meters.get(index).copied().unwrap_or((0.0, 0.0));
    let color = app.project.insert_color(id).map(theme::rgb);
    let fader = widgets::fader(insert.volume, (0.0, 2.0), 1.0, 150.0, move |v| Message::Volume(id, v).into())
        .help(format!("Volume: {}\nDouble-click for 0 dB; right-click to automate", gain_text(insert.volume)))
        .on_secondary_press(move |app: &mut App, _, tx: &mut Tx| ui::send(app, tx, AppMessage::Automate(Target::InsertVolume(id))));
    let pan = Slider::new(insert.pan, (-1.0, 1.0)).bipolar().default(0.0).width(STRIP_WIDTH - 14.0).color(C.text_dim)
        .help(format!("Pan: {}\nDouble-click to center; right-click to automate", pan_text(insert.pan)))
        .on_secondary(AppMessage::Automate(Target::InsertPan(id)))
        .build(move |v| Message::Pan(id, v).into());
    // Where the signal goes, when it is not straight to the master.
    let detector = insert.sends.iter().find(|s| s.sidechain).filter(|_| insert.is_detector_only());
    let output = match (detector, app.project.mixer.output(id)) {
        (Some(send), _) => format!("SC → {}", app.project.insert_label(send.to)),
        (None, Some(to)) if to != daw_model::MASTER => format!("→ {}", app.project.insert_label(to)),
        _ => String::new(),
    };
    // Inserts whose signal drives a sidechain here.
    let sources: Vec<_> = app.project.mixer.sidechain_sources(id).map(|(i, _)| i.id).collect();
    let sidechain = match sources.as_slice() {
        [] => String::new(),
        [one] => format!("SC ← {}", app.project.insert_label(*one)),
        many => format!("SC ← {}", many.len()),
    };
    let small = |line: String| text(line).font(theme::SMALL_TEXT).color(C.text_faint).single_line().truncate().w(STRIP_WIDTH - 10.0).align(rune::Align::Center);
    let name = text(app.project.insert_label(id)).font(theme::LABEL).color(if selected { C.text } else { C.text_dim }).single_line().truncate().w(STRIP_WIDTH - 10.0).align(rune::Align::Center);
    let meters = row().gap(2.0).child(widgets::meter(left, 150.0)).child(widgets::meter(right, 150.0));
    let buttons = row().gap(3.0)
        .child(Btn::new("M").width(28.0).height(22.0).look(if insert.mute { Look::On } else { Look::Raised }).help("Mute insert").build(Message::Mute(id)))
        .child(Btn::new("S").width(28.0).height(22.0).look(if insert.solo { Look::On } else { Look::Raised }).help("Solo insert").build(Message::Solo(id)));
    let body = col().w(STRIP_WIDTH).h_full().items_center().gap(6.0).pt(0.0).pb(8.0)
        .child(el().w_full().h(3.0).fill(color.unwrap_or(rune::Color::TRANSPARENT)))
        .child(name.mt(4.0))
        .child(small(output))
        .child(small(sidechain))
        .child(spacer())
        .child(row().gap(6.0).h(150.0).child(meters).child(fader))
        .child(text(gain_text(insert.volume)).font(theme::NUMBER).color(C.text_dim).single_line())
        .child(pan)
        .child(text(pan_text(insert.pan)).font(theme::NUMBER).color(C.text_faint).single_line())
        .child(buttons);
    let strip = body.radius(theme::RADIUS).clip_rounded(theme::RADIUS)
        .fill(if selected { C.raised } else { C.header })
        .border(1.0, if selected { C.accent.alpha(0.5) } else { C.border })
        .on_press_tx(ui::on(Message::Select(id)))
        .on_secondary_press(move |app: &mut App, _, tx: &mut Tx| ui::send(app, tx, menu::Message::Open(menu::Item::Insert(id)).into()));
    browser::zone(app, DropZone::Insert(id), strip)
}

/// The selected insert: its routing, sends, sidechains, and effects.
fn pane(app: &App) -> El {
    let mut chain = col().gap(10.0).padding(14.0).w(PANE_WIDTH);
    let Some(insert) = app.project.mixer.insert(app.selected_insert) else { return chain };
    chain = chain.child(text(app.project.insert_label(insert.id)).font(theme::TITLE).color(C.text).single_line().truncate());
    let mixer = &app.project.mixer;
    let choice = |i: &Insert, message: Message| Choice::new(app.project.insert_label(i.id), message, false);
    if insert.id != daw_model::MASTER {
        let from = insert.id;
        let current = mixer.output(from);
        let choices: Vec<Choice> = mixer.inserts.iter().filter(|i| mixer.can_route(from, i.id))
            .map(|i| Choice::new(app.project.insert_label(i.id), Message::Output(from, i.id), Some(i.id) == current))
            .collect();
        chain = chain.child(col().gap(4.0)
            .child(label("Output"))
            .child(widgets::select("mixer-output", current.map(|to| app.project.insert_label(to)), "Choose insert", choices, PANE_WIDTH - 28.0)));
        let open = |i: &&Insert| mixer.can_route(from, i.id) && !insert.sends.iter().any(|s| s.to == i.id);
        let audio = mixer.inserts.iter().filter(open).filter(|i| i.id != insert.output).map(|i| choice(i, Message::Send(from, i.id, false))).collect();
        let detectors = mixer.inserts.iter().filter(open).filter(|i| sidechain_ready(app, i) != Some(false)).map(|i| choice(i, Message::Send(from, i.id, true))).collect();
        chain = chain.child(widgets::section("Sends").mt(4.0));
        chain = chain.child(route_row("mixer-send", "Send to", "Send a copy of this insert's audio to another insert", audio));
        chain = chain.child(route_row("mixer-sidechain-to", "Sidechain to", "Drive the sidechain input of an effect on another insert.\nLists inserts with an effect that can take a sidechain.", detectors));
        for send in &insert.sends {
            let to = send.to;
            let kind = if send.sidechain { "Sidechain" } else { "Send" };
            chain = chain.child(send_row(format!("{kind} → {}", app.project.insert_label(to)), from, to, send.level));
        }
        if insert.sends.iter().any(|s| s.sidechain) {
            chain = chain.child(panels_help(
                Btn::new("Detector only").on(insert.detector_only).build(Message::DetectorOnly(from)),
                "Hear this insert only through its sidechain sends.\nIts output and audio sends go silent.",
            ));
        }
    }
    let to = insert.id;
    let sources: Vec<Choice> = mixer.inserts.iter().filter(|i| mixer.can_route(i.id, to) && !i.sends.iter().any(|s| s.to == to)).map(|i| choice(i, Message::Send(i.id, to, true))).collect();
    chain = chain.child(route_row(
        "mixer-sidechain-from",
        "Sidechain from",
        "Use another insert's signal to drive the sidechain input of an effect here,\nsuch as a compressor that ducks the bass when the kick plays",
        sources,
    ));
    let mut fed = false;
    for (source, send) in mixer.sidechain_sources(to) {
        fed = true;
        chain = chain.child(send_row(format!("Sidechain ← {}", app.project.insert_label(source.id)), source.id, to, send.level));
    }
    if fed && sidechain_ready(app, insert) == Some(false) {
        chain = chain.child(text("No effect here has a sidechain input.").font(theme::SMALL_TEXT).color(C.danger).wrap());
    }
    chain = chain.child(widgets::section("Effects").mt(4.0));
    for (index, &instance) in insert.effects.iter().enumerate() {
        let name = app.project.plugin(instance).map(|p| p.plugin.name.clone()).unwrap_or_default();
        let failed = app.session.load_errors.contains_key(&instance);
        let name = if failed { format!("{name} (failed)") } else { name };
        let has_sidechain = app.session.sidechain_input(instance) == Some(true);
        let name = if has_sidechain { format!("{name} · SC") } else { name };
        let hint = if has_sidechain { "Open or close the editor\nHas a sidechain input (SC)" } else { "Open or close the editor" };
        chain = chain.child(row().gap(2.0).center_y().w_full()
            .child(Btn::new(name).width(PANE_WIDTH - 28.0 - 52.0).start().look(if app.session.editor_open(instance) { Look::On } else { Look::Raised }).help(hint).build(AppMessage::TogglePlugin(instance)))
            .child(Btn::new("").icon(&icons::UP).ghost().disabled(index == 0).help("Move up the chain").build(Message::RaiseEffect(insert.id, index)))
            .child(Btn::new("").icon(&icons::CLOSE).ghost().help("Remove effect").build(Message::RemoveEffect(insert.id, index))));
    }
    if insert.effects.is_empty() {
        chain = chain.child(text("Drag effects here from the browser.").font(theme::SMALL_TEXT).color(C.text_faint).wrap());
    }
    chain
}

fn panels_help(content: El, hint: &str) -> El {
    super::help(content, hint)
}

pub fn view(app: &App) -> El {
    let mut strips = row().gap(6.0).h_full().padding(10.0);
    for (index, insert) in app.project.mixer.inserts.iter().enumerate() {
        strips = strips.child(strip(app, index, insert));
    }
    let strips = super::scroll(strips, false, true)
        .key(STRIPS)
        .h_full()
        .on_scroll_offset(|pos| SCROLLED.set(pos.offset[0]))
        .on_wheel(|app: &mut App, w, tx| {
            if w.dy.abs() <= w.dx.abs() { return false; }
            ui::send(app, tx, Message::Wheel(ScrollDelta::Pixels { x: w.dx, y: w.dy }).into());
            true
        });
    let pane = browser::zone_part(app, DropZone::Insert(app.selected_insert), "pane",
        col().w(PANE_WIDTH).h_full().fill(C.header).border_sides([1.0, 0.0, 0.0, 0.0], C.line).child(super::scroll(pane(app), true, false)));
    row().w_full().h_full().child(col().grow(1.0).min_w(0.0).h_full().child(strips)).child(pane)
}
