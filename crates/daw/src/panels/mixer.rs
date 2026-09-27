//! Mixer inserts with meters, and the effect chain of the selected insert.

use daw_model::{InsertId, Target};
use iced::widget::{Space, button, column, container, mouse_area, row, slider, text, vertical_slider};
use iced::{Element, Length};

use super::{label, tool, toggle};
use crate::app::{App, Message as AppMessage, gain_text, pan_text};
use crate::theme;

const STRIP_WIDTH: f32 = 58.0;

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

pub fn update(app: &mut App, message: Message) {
    match message {
        Message::Select(id) => app.selected_insert = id,
        Message::Volume(id, volume) => {
            app.begin_edit();
            if let Some(insert) = app.project.mixer.insert_mut(id) {
                insert.volume = volume;
            }
            app.edited();
            app.touched(Target::InsertVolume(id), volume / 2.0);
        }
        Message::Pan(id, pan) => {
            app.begin_edit();
            if let Some(insert) = app.project.mixer.insert_mut(id) {
                insert.pan = pan;
            }
            app.edited();
            app.touched(Target::InsertPan(id), (pan + 1.0) / 2.0);
        }
        Message::Mute(id) => {
            app.checkpoint();
            if let Some(insert) = app.project.mixer.insert_mut(id) {
                insert.mute = !insert.mute;
            }
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
            let Some(instance) = app.project.mixer.insert(id).and_then(|i| i.effects.get(index).copied()) else { return };
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
        let strip = column![
            name,
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
        strips = strips.push(mouse_area(container(strip).height(Length::Fill).style(theme::fill(background))).on_press(Message::Select(id).into()));
    }

    let mut chain = column![label("effects")].spacing(2).padding(4).width(190);
    if let Some(insert) = app.project.mixer.insert(app.selected_insert) {
        chain = chain.push(text(insert.name.clone()).size(theme::TEXT_SIZE).color(theme::BRIGHT));
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
        super::scroll(strips, false, true).width(Length::Fill).height(Length::Fill),
        container(chain).height(Length::Fill).style(theme::fill(theme::HEADER)),
    ]
    .spacing(3)
    .into()
}
