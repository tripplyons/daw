//! Installed plugins from the scan cache. Instruments add a channel; effects
//! go on the selected mixer insert.

use daw_model::{PluginFormat, Source};
use daw_plugins::PluginKind;
use iced::widget::{button, column, container, row, text, text_input};
use iced::{Element, Length, Task};

use super::{label, tool, toggle};
use crate::app::{App, Message as AppMessage};
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Filter {
    #[default]
    All,
    Instruments,
    Effects,
    Failed,
}

#[derive(Debug, Default)]
pub struct State {
    pub search: String,
    pub filter: Filter,
}

#[derive(Debug, Clone)]
pub enum Message {
    Search(String),
    Filter(Filter),
    Add(usize),
}

impl From<Message> for AppMessage {
    fn from(message: Message) -> Self {
        AppMessage::Browser(message)
    }
}

fn matches(search: &str, fields: &[&str]) -> bool {
    let search = search.to_lowercase();
    search.split_whitespace().all(|word| fields.iter().any(|f| f.to_lowercase().contains(word)))
}

pub fn update(app: &mut App, message: Message) -> Task<AppMessage> {
    match message {
        Message::Search(search) => app.browser.search = search,
        Message::Filter(filter) => app.browser.filter = filter,
        Message::Add(index) => {
            let Some(info) = app.catalog.plugins.get(index).cloned() else { return Task::none() };
            app.checkpoint();
            let instance = app.project.add_plugin(info.plugin.clone());
            match info.kind {
                PluginKind::Instrument => {
                    let channel = app.project.add_channel(&info.plugin.name, Source::Plugin(instance));
                    app.selected_channel = Some(channel);
                }
                PluginKind::Effect => {
                    let insert = app.selected_insert;
                    if let Some(insert) = app.project.mixer.insert_mut(insert) {
                        insert.effects.push(instance);
                    }
                }
            }
            app.edited();
            if let Some(error) = app.session.load_errors.get(&instance).cloned() {
                app.project.remove_plugin(instance);
                app.project.channels.retain(|c| c.source != Source::Plugin(instance));
                app.refresh();
                app.set_status(format!("{} failed to load: {error}", info.plugin.name));
                return Task::none();
            }
            let target = match info.kind {
                PluginKind::Instrument => "channel rack".to_string(),
                PluginKind::Effect => app.project.mixer.insert(app.selected_insert).map(|i| i.name.clone()).unwrap_or_default(),
            };
            app.set_status(format!("added {} to {target}", info.plugin.name));
            return Task::done(AppMessage::OpenPlugin(instance));
        }
    }
    Task::none()
}

pub fn toolbar(app: &App) -> Element<'_, AppMessage> {
    let filter = app.browser.filter;
    let rescan: Element<'_, AppMessage> = if app.scan.is_some() { label("scanning") } else { tool("rescan", AppMessage::Rescan) };
    row![
        toggle("all", filter == Filter::All, Message::Filter(Filter::All).into()),
        toggle("inst", filter == Filter::Instruments, Message::Filter(Filter::Instruments).into()),
        toggle("fx", filter == Filter::Effects, Message::Filter(Filter::Effects).into()),
        toggle("failed", filter == Filter::Failed, Message::Filter(Filter::Failed).into()),
        rescan,
    ]
    .spacing(2)
    .into()
}

pub fn view(app: &App) -> Element<'_, AppMessage> {
    let state = &app.browser;
    let search = text_input("search plugins", &state.search)
        .on_input(|s| Message::Search(s).into())
        .size(theme::SMALL)
        .padding([3, 6])
        .style(theme::input);
    let mut list = column![].width(Length::Fill);
    if state.filter == Filter::Failed {
        for failure in app.catalog.failed.iter().filter(|f| matches(&state.search, &[&f.name, &f.error])) {
            list = list.push(
                column![
                    text(failure.name.clone()).size(theme::SMALL),
                    text(failure.error.clone()).size(theme::SMALL).color(theme::TEXT_DIM),
                ]
                .padding([2, 6]),
            );
        }
    } else {
        for (index, info) in app.catalog.plugins.iter().enumerate() {
            let kind_ok = match state.filter {
                Filter::Instruments => info.kind == PluginKind::Instrument,
                Filter::Effects => info.kind == PluginKind::Effect,
                _ => true,
            };
            if !kind_ok || !matches(&state.search, &[&info.plugin.name, &info.plugin.vendor, &info.category]) {
                continue;
            }
            let format = match info.plugin.format {
                PluginFormat::Vst3 => "vst3",
                PluginFormat::AudioUnit => "au",
            };
            let kind = match info.kind {
                PluginKind::Instrument => "inst",
                PluginKind::Effect => "fx",
            };
            let line = row![
                text(info.plugin.name.clone()).size(theme::SMALL).width(Length::Fill),
                text(info.plugin.vendor.clone()).size(theme::SMALL).color(theme::TEXT_FAINT),
                text(kind).size(theme::SMALL).color(theme::TEXT_DIM).width(28),
                text(format).size(theme::SMALL).color(theme::TEXT_DIM).width(28),
            ]
            .spacing(6);
            list = list.push(
                button(line).on_press(Message::Add(index).into()).style(theme::plain(false)).padding([2, 6]).width(Length::Fill),
            );
        }
    }
    let empty = app.catalog.plugins.is_empty() && app.scan.is_some();
    let body: Element<'_, AppMessage> = if empty {
        container(label("scanning installed plugins")).padding(8).into()
    } else {
        super::scroll(list, true, false).height(Length::Fill).into()
    };
    column![container(search).padding(4), body].into()
}
