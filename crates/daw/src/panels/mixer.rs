//! Mixer inserts with meters, and the effect chain of the selected insert.

use daw_model::automation::{insert_volume_to_normalized, pan_to_normalized};
use daw_model::{InsertId, Send, Target};
use iced::widget::operation::{self, AbsoluteOffset};
use iced::widget::{Space, button, column, container, mouse_area, row, slider, text, vertical_slider};
use iced::{Element, Length, Task, mouse};

use super::{InsertChoice, label, pick, tool, toggle};
use crate::app::{App, Message as AppMessage};
use crate::menu;
use crate::theme;
use crate::units::{gain_text, pan_text};

const STRIP_WIDTH: f32 = 58.0;
const STRIPS: &str = "mixer-strips";

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
    /// A wheel step the strip area did not use: plain up/down scrolling,
    /// turned into sideways scrolling since the strips only scroll sideways.
    Wheel(mouse::ScrollDelta),
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

pub fn update(app: &mut App, message: Message) -> Task<AppMessage> {
    match message {
        Message::Send(from, to, sidechain) => {
            if !app.project.mixer.can_route(from, to) { app.set_status("that send would create feedback"); return Task::none(); }
            app.checkpoint();
            app.project.mixer.set_send(from, Send { to, level: 1.0, sidechain });
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
                let x = -dy * 60.0;
                return operation::scroll_by(STRIPS, AbsoluteOffset { x, y: 0.0 });
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

pub fn toolbar(app: &App) -> Element<'_, AppMessage> {
    let delete: Element<'_, AppMessage> = if app.selected_insert == daw_model::MASTER {
        label("")
    } else {
        tool("delete insert", Message::DeleteInsert.into())
    };
    row![tool("+ insert", Message::AddInsert.into()), delete].spacing(2).into()
}

fn meter<'a>(peak: f32) -> Element<'a, AppMessage> {
    let db = if peak > 0.0 { 20.0 * peak.log10() } else { -120.0 };
    let fraction = ((db + 60.0) / 66.0).clamp(0.0, 1.0);
    let lit = (fraction * 1000.0).round() as u16;
    let color = if db > 0.0 { theme::BRIGHT } else { theme::FILL };
    let mut bar = column![].width(3).height(Length::Fill);
    if lit < 1000 {
        bar = bar.push(Space::new().height(Length::FillPortion(1000 - lit)));
    }
    if lit > 0 {
        bar = bar.push(container(Space::new()).width(3).height(Length::FillPortion(lit)).style(theme::fill(color)));
    }
    container(bar).style(theme::fill(theme::HEADER)).height(Length::Fill).into()
}

pub fn view(app: &App) -> Element<'_, AppMessage> {
    let mut strips = row![].spacing(1).height(Length::Fill);
    for (index, insert) in app.project.mixer.inserts.iter().enumerate() {
        let id = insert.id;
        let selected = app.selected_insert == id;
        let (left, right) = app.meters.get(index).copied().unwrap_or((0.0, 0.0));
        let fader = mouse_area(
            vertical_slider(0.0..=2.0, insert.volume, move |v| Message::Volume(id, v).into())
                .step(0.001_f32)
                .on_release(AppMessage::EndEdit)
                .width(12)
                .style(theme::fader),
        )
        .on_right_press(AppMessage::Automate(Target::InsertVolume(id)));
        let pan = mouse_area(
            slider(-1.0..=1.0, insert.pan, move |v| Message::Pan(id, v).into())
                .step(0.01_f32)
                .on_release(AppMessage::EndEdit)
                .height(12)
                .style(theme::fader),
        )
        .on_right_press(AppMessage::Automate(Target::InsertPan(id)));
        let name = button(text(insert.name.clone()).size(theme::SMALL))
            .on_press(Message::Select(id).into())
            .style(theme::plain(selected))
            .padding([1, 3])
            .width(Length::Fill);
        // Where the signal goes, when it is not straight to the master.
        let output = match app.project.mixer.output(id) {
            Some(to) if to != daw_model::MASTER => app.project.mixer.insert(to).map(|i| format!("> {}", i.name)),
            _ => None,
        };
        // About 5 px per character at size 10.
        let output: String = output.unwrap_or_default().chars().take(((STRIP_WIDTH - 6.0) / 5.0) as usize).collect();
        let output = text(output).size(10).color(theme::TEXT_DIM);
        let strip = column![
            name,
            output,
            row![meter(left), meter(right), fader].spacing(2).height(Length::Fill),
            text(gain_text(insert.volume)).size(10).color(theme::TEXT_DIM),
            pan,
            text(pan_text(insert.pan)).size(10).color(theme::TEXT_DIM),
            row![
                toggle("m", insert.mute, Message::Mute(id).into()),
                toggle("s", insert.solo, Message::Solo(id).into()),
            ]
            .spacing(2),
        ]
        .spacing(3)
        .padding(3)
        .width(STRIP_WIDTH)
        .align_x(iced::Alignment::Center);
        let background = if selected { theme::HEADER } else { theme::BG };
        strips = strips.push(
            mouse_area(container(strip).height(Length::Fill).style(theme::fill(background)))
                .on_press(Message::Select(id).into())
                .on_right_press(menu::Message::Open(menu::Item::Insert(id)).into()),
        );
    }

    let mut chain = column![label("effects")].spacing(2).padding(4).width(190);
    if let Some(insert) = app.project.mixer.insert(app.selected_insert) {
        chain = chain.push(text(insert.name.clone()).size(theme::TEXT_SIZE).color(theme::BRIGHT));
        if insert.id != daw_model::MASTER {
            let mixer = &app.project.mixer;
            let from = insert.id;
            let choices: Vec<InsertChoice> = mixer
                .inserts
                .iter()
                .filter(|i| mixer.can_route(from, i.id))
                .map(|i| InsertChoice { id: i.id, name: i.name.clone() })
                .collect();
            let current = mixer.output(from).and_then(|to| choices.iter().find(|c| c.id == to).cloned());
            chain = chain.push(
                row![label("output"), pick(choices, current, move |c: InsertChoice| Message::Output(from, c.id).into())]
                    .spacing(4)
                    .align_y(iced::Alignment::Center),
            );
        }
        if insert.id != daw_model::MASTER {
            let from = insert.id;
            let choices: Vec<InsertChoice> = app.project.mixer.inserts.iter()
                .filter(|i| app.project.mixer.can_route(from, i.id) && !insert.sends.iter().any(|s| s.to == i.id))
                .map(|i| InsertChoice { id: i.id, name: i.name.clone() }).collect();
            let audio = choices.iter().filter(|i| i.id != insert.output).cloned().collect();
            chain = chain.push(row![label("send"), pick(audio, None, move |c: InsertChoice| Message::Send(from, c.id, false).into())].spacing(4));
            chain = chain.push(row![label("sidechain"), pick(choices, None, move |c: InsertChoice| Message::Send(from, c.id, true).into())].spacing(4));
            for send in &insert.sends {
                let to = send.to;
                let name = app.project.mixer.insert(to).map(|i| i.name.clone()).unwrap_or_default();
                chain = chain.push(row![label(format!("{} {name}", if send.sidechain { "sc" } else { "send" })),
                    tool("x", Message::RemoveSend(from, to).into())].spacing(4));
                chain = chain.push(slider(Send::LEVEL, send.level, move |v| Message::SendLevel(from, to, v).into())
                    .step(0.01_f32).on_release(AppMessage::EndEdit).style(theme::fader));
            }
        }
        for (index, &instance) in insert.effects.iter().enumerate() {
            let name = app.project.plugin(instance).map(|p| p.plugin.name.clone()).unwrap_or_default();
            let failed = app.session.load_errors.contains_key(&instance);
            let name = if failed { format!("{name} (failed)") } else { name };
            chain = chain.push(
                row![
                    button(text(name).size(theme::SMALL))
                        .on_press(AppMessage::TogglePlugin(instance))
                        .style(theme::toggle(app.session.editor_open(instance)))
                        .padding([1, 4])
                        .width(Length::Fill),
                    tool("up", Message::RaiseEffect(insert.id, index).into()),
                    tool("x", Message::RemoveEffect(insert.id, index).into()),
                ]
                .spacing(2),
            );
        }
        if insert.effects.is_empty() {
            chain = chain.push(label("add effects from the browser"));
        }
    }
    row![
        mouse_area(super::scroll(strips, false, true).id(STRIPS).width(Length::Fill).height(Length::Fill))
            .on_scroll(|delta| Message::Wheel(delta).into()),
        container(super::scroll(chain, true, false)).height(Length::Fill).style(theme::fill(theme::HEADER)),
    ]
    .spacing(3)
    .into()
}
