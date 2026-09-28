//! Channels of the selected pattern with FL-style step buttons.

use std::path::PathBuf;

use daw_engine::song::channel_node;
use daw_model::{ChannelId, InsertId, STEP_TICKS, Source, SynthParams, Target};
use iced::widget::{Space, button, column, container, mouse_area, row, slider, text};
use iced::{Element, Length, Task};

use super::{InsertChoice, label, pick, tool};
use crate::app::{App, Message as AppMessage};
use crate::menu;
use crate::theme;

const MAX_STEPS: u64 = 64;

#[derive(Debug, Clone)]
pub enum Message {
    /// Select a channel and show or hide its plugin window, if it has one.
    Select(ChannelId),
    Step(ChannelId, u64),
    Mute(ChannelId),
    Volume(ChannelId, f32),
    Route(ChannelId, InsertId),
    Remove,
    AddSynth,
    AddSampler,
    SamplePicked(Option<PathBuf>),
    Preview(ChannelId),
}

impl From<Message> for AppMessage {
    fn from(message: Message) -> Self {
        AppMessage::Rack(message)
    }
}

pub fn select(app: &mut App, id: ChannelId) {
    app.selected_channel = Some(id);
    if let Some(channel) = app.project.channel(id) {
        app.selected_insert = channel.insert;
        app.param_instance = match channel.source {
            Source::Plugin(instance) => Some(instance),
            _ => None,
        };
    }
}

/// Delete the selected channel with its notes, plugin, and automation.
pub fn delete_selected(app: &mut App) -> bool {
    let Some(id) = app.selected_channel.filter(|&c| app.project.channel(c).is_some()) else { return false };
    app.checkpoint();
    let name = app.project.channel(id).map(|c| c.name.clone()).unwrap_or_default();
    let index = app.project.channels.iter().position(|c| c.id == id).unwrap_or(0);
    app.project.remove_channel(id);
    let next = app.project.channels.get(index).or(app.project.channels.last()).map(|c| c.id);
    app.selected_channel = None;
    if let Some(next) = next {
        select(app, next);
    }
    app.set_status(format!("deleted {name}"));
    app.edited();
    true
}

pub fn update(app: &mut App, message: Message) -> Task<AppMessage> {
    match message {
        Message::Select(id) => {
            select(app, id);
            if let Some(Source::Plugin(instance)) = app.project.channel(id).map(|c| &c.source) {
                return Task::done(AppMessage::TogglePlugin(*instance));
            }
        }
        Message::Step(id, step) => {
            app.checkpoint();
            let pattern = app.selected_pattern;
            if let Some(pattern) = app.project.pattern_mut(pattern) {
                pattern.toggle_step(id, step);
            }
            select(app, id);
            app.edited();
        }
        Message::Mute(id) => {
            app.checkpoint();
            if let Some(channel) = app.project.channel_mut(id) {
                channel.mute = !channel.mute;
            }
            app.edited();
        }
        Message::Volume(id, volume) => {
            app.begin_edit();
            if let Some(channel) = app.project.channel_mut(id) {
                channel.volume = volume;
            }
            app.edited();
            app.touched(Target::ChannelVolume(id), volume);
        }
        Message::Route(id, insert) => {
            app.checkpoint();
            if let Some(channel) = app.project.channel_mut(id) {
                channel.insert = insert;
            }
            app.selected_insert = insert;
            app.edited();
        }
        Message::Remove => {
            delete_selected(app);
        }
        Message::AddSynth => {
            app.checkpoint();
            let number = app.project.channels.len() + 1;
            let id = app.project.add_channel(&format!("synth {number}"), Source::Synth(SynthParams::default()));
            select(app, id);
            app.edited();
        }
        Message::AddSampler => {
            return Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .add_filter("audio", &["wav", "wave"])
                        .pick_file()
                        .await
                        .map(|f| f.path().to_owned())
                },
                |path| Message::SamplePicked(path).into(),
            );
        }
        Message::SamplePicked(Some(path)) => {
            app.checkpoint();
            let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "sample".into());
            let source = Source::Sampler { path: path.to_string_lossy().into_owned(), root_key: daw_model::DEFAULT_KEY };
            let id = app.project.add_channel(&name, source);
            select(app, id);
            app.edited();
        }
        Message::SamplePicked(None) => {}
        Message::Preview(id) => {
            if let Some(channel) = app.project.channel(id) {
                let node = channel_node(&channel.source, id);
                app.session.note(node, daw_model::DEFAULT_KEY, 0.8);
                app.session.note(node, daw_model::DEFAULT_KEY, 0.0);
            }
            select(app, id);
        }
    }
    Task::none()
}

pub fn toolbar(_app: &App) -> Element<'_, AppMessage> {
    row![
        tool("+ synth", Message::AddSynth.into()),
        tool("+ sample", Message::AddSampler.into()),
        tool("delete", Message::Remove.into()),
    ]
    .spacing(2)
    .align_y(iced::Alignment::Center)
    .into()
}

fn step_button<'a>(on: bool, beat_shade: bool, message: AppMessage) -> Element<'a, AppMessage> {
    let color = match (on, beat_shade) {
        (true, _) => theme::FILL,
        (false, true) => theme::CONTROL_HOVER,
        (false, false) => theme::CONTROL,
    };
    button(Space::new().width(12).height(16))
        .on_press(message)
        .padding(0)
        .style(move |_, status| {
            let background = match status {
                button::Status::Hovered if !on => theme::CONTROL_ACTIVE,
                button::Status::Hovered => theme::SELECTED,
                _ => color,
            };
            button::Style { background: Some(background.into()), ..button::Style::default() }
        })
        .into()
}

pub fn view(app: &App) -> Element<'_, AppMessage> {
    let Some(pattern) = app.project.pattern(app.selected_pattern) else { return label("no pattern") };
    let steps = (pattern.length / STEP_TICKS).clamp(1, MAX_STEPS);
    let inserts: Vec<InsertChoice> =
        app.project.mixer.inserts.iter().map(|i| InsertChoice { id: i.id, name: i.name.clone() }).collect();
    let mut rows = column![].spacing(2).padding(4);
    for channel in &app.project.channels {
        let id = channel.id;
        let selected = app.selected_channel == Some(id);
        // Lit like an effect button while the plugin window is showing.
        let open = matches!(channel.source, Source::Plugin(instance) if app.session.editor_open(instance));
        let name = mouse_area(
            button(text(channel.name.clone()).size(theme::SMALL))
                .on_press(Message::Select(id).into())
                .style(move |t, status| if open { theme::toggle(true)(t, status) } else { theme::plain(selected)(t, status) })
                .padding([1, 6])
                .width(110),
        )
        .on_right_press(menu::Message::Open(menu::Item::Channel(id)).into());
        let volume = mouse_area(
            slider(0.0..=1.0, channel.volume, move |v| Message::Volume(id, v).into())
                .step(0.001_f32)
                .on_release(AppMessage::EndEdit)
                .width(56)
                .height(16)
                .style(theme::fader),
        )
        .on_right_press(AppMessage::Automate(Target::ChannelVolume(id)));
        let current = inserts.iter().find(|i| i.id == channel.insert).cloned();
        let route = container(pick(inserts.clone(), current, move |i: InsertChoice| Message::Route(id, i.id).into())).width(76);
        let mut step_row = row![].spacing(1);
        for step in 0..steps {
            if step > 0 && step % 4 == 0 {
                step_row = step_row.push(Space::new().width(3));
            }
            let shade = (step / 4) % 2 == 1;
            step_row = step_row.push(step_button(pattern.step_on(id, step), shade, Message::Step(id, step).into()));
        }
        let muted = channel.mute;
        let mute = button(Space::new().width(8).height(8))
            .on_press(Message::Mute(id).into())
            .padding(4)
            .style(move |_, _| {
                let color = if muted { theme::FILL_DIM } else { theme::SELECTED };
                button::Style { background: Some(color.into()), ..button::Style::default() }
            });
        rows = rows.push(row![mute, name, volume, route, step_row].spacing(4).align_y(iced::Alignment::Center));
    }
    super::scroll(rows, true, true).width(Length::Fill).height(Length::Fill).into()
}
