//! Installed plugins from the scan cache. Clicking an instrument adds a
//! channel and clicking an effect puts it on the selected mixer insert.
//! Dragging a plugin onto the channel rack, a channel, or a mixer insert
//! chooses where it goes.

use daw_model::{ChannelId, InsertId, PluginFormat, Source};
use daw_plugins::{PluginInfo, PluginKind};
use iced::widget::{column, container, mouse_area, row, text, text_input};
use iced::{Element, Length, Point, Task, mouse};

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
    /// The plugin held by the mouse, from a press on its row.
    pub drag: Option<Drag>,
    /// Drop zones under the cursor; the rack contains channel zones.
    hover: Vec<DropZone>,
}

#[derive(Debug, Clone, Copy)]
pub struct Drag {
    /// Index in the catalog.
    pub plugin: usize,
    from: Point,
    /// Whether the cursor has left the press point; a still press is a click.
    pub moved: bool,
}

/// Where a dragged plugin can land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropZone {
    Rack,
    Channel(ChannelId),
    Insert(InsertId),
}

/// What adding a plugin does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// A new channel, routed to the insert if one is given.
    Channel(Option<InsertId>),
    /// The end of an insert's effect chain.
    Effect(InsertId),
}

/// Pixels the cursor must move before a press becomes a drag.
const DRAG_DISTANCE: f32 = 4.0;

#[derive(Debug, Clone)]
pub enum Message {
    Search(String),
    Filter(Filter),
    Grab(usize),
    Enter(DropZone),
    Leave(DropZone),
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
        Message::Grab(plugin) => {
            app.browser.drag = Some(Drag { plugin, from: app.cursor, moved: false });
            app.browser.hover.clear();
        }
        Message::Enter(zone) => {
            if app.browser.drag.is_some() && !app.browser.hover.contains(&zone) { app.browser.hover.push(zone); }
        }
        Message::Leave(zone) => app.browser.hover.retain(|&z| z != zone),
    }
    Task::none()
}

/// Follow the cursor while a plugin is held.
pub fn moved(app: &mut App, point: Point) {
    if let Some(drag) = &mut app.browser.drag
        && !drag.moved
        && (point.x - drag.from.x).hypot(point.y - drag.from.y) >= DRAG_DISTANCE
    {
        drag.moved = true;
    }
}

/// Drop the held plugin: a click adds it in the default place, a drag adds
/// it where it lands, and a drag that lands nowhere does nothing.
pub fn released(app: &mut App) -> Task<AppMessage> {
    let Some(drag) = app.browser.drag.take() else { return Task::none() };
    let hover = std::mem::take(&mut app.browser.hover);
    let Some(info) = app.catalog.plugins.get(drag.plugin).cloned() else { return Task::none() };
    let place = if drag.moved { resolve(app, &info, &hover).map(|(_, place)| place) } else { Some(default_place(app, &info)) };
    match place {
        Some(place) => add(app, &info, place),
        None => {
            let zones = match info.kind {
                PluginKind::Instrument => "the channel rack or a mixer insert",
                PluginKind::Effect => "a channel or a mixer insert",
            };
            app.set_status(format!("drop {} on {zones}", info.plugin.name));
            Task::none()
        }
    }
}

fn default_place(app: &App, info: &PluginInfo) -> Place {
    match info.kind {
        PluginKind::Instrument => Place::Channel(None),
        PluginKind::Effect => Place::Effect(app.selected_insert),
    }
}

/// The most specific hovered zone that takes this kind of plugin, and what
/// dropping there does. Instruments make channels; effects go on inserts.
fn resolve(app: &App, info: &PluginInfo, hover: &[DropZone]) -> Option<(DropZone, Place)> {
    let specific = hover.iter().rev().filter(|&&z| z != DropZone::Rack);
    specific.chain(hover.iter().filter(|&&z| z == DropZone::Rack)).find_map(|&zone| {
        let place = match (info.kind, zone) {
            (PluginKind::Instrument, DropZone::Rack | DropZone::Channel(_)) => Place::Channel(None),
            (PluginKind::Instrument, DropZone::Insert(insert)) => Place::Channel(Some(insert)),
            (PluginKind::Effect, DropZone::Channel(channel)) => Place::Effect(app.project.channel(channel)?.insert),
            (PluginKind::Effect, DropZone::Insert(insert)) => Place::Effect(insert),
            (PluginKind::Effect, DropZone::Rack) => return None,
        };
        Some((zone, place))
    })
}

/// Where a drop would land now, and what it would do.
pub fn drop_target(app: &App) -> Option<(DropZone, Place)> {
    let drag = app.browser.drag.filter(|d| d.moved)?;
    let info = app.catalog.plugins.get(drag.plugin)?;
    resolve(app, info, &app.browser.hover)
}

fn place_label(app: &App, place: Place) -> String {
    match place {
        Place::Channel(None) => "the channel rack".into(),
        Place::Channel(Some(insert)) => format!("a new channel on {}", app.project.insert_label(insert)),
        Place::Effect(insert) => app.project.insert_label(insert),
    }
}

/// Add a catalog plugin as one undo step and open its window.
pub fn add(app: &mut App, info: &PluginInfo, place: Place) -> Task<AppMessage> {
    app.checkpoint();
    let instance = app.project.add_plugin(info.plugin.clone());
    match place {
        Place::Channel(route) => {
            let channel = app.project.add_channel(&info.plugin.name, Source::Plugin(instance));
            if let Some(insert) = route
                && let Some(channel) = app.project.channel_mut(channel)
            {
                channel.insert = insert;
            }
            super::channel_rack::select(app, channel);
        }
        Place::Effect(insert) => {
            if let Some(slot) = app.project.mixer.insert_mut(insert) {
                slot.effects.push(instance);
            }
            app.selected_insert = insert;
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
    app.set_status(format!("added {} to {}", info.plugin.name, place_label(app, place)));
    Task::done(AppMessage::OpenPlugin(instance))
}

/// Make `content` a place to drop plugins, outlined while a drop would land there.
pub fn zone<'a>(app: &App, zone: DropZone, content: impl Into<Element<'a, AppMessage>>) -> Element<'a, AppMessage> {
    let lit = drop_target(app).is_some_and(|(z, _)| z == zone);
    let content = container(content).style(move |_| container::Style {
        border: iced::Border { color: if lit { theme::BRIGHT } else { iced::Color::TRANSPARENT }, width: 1.0, radius: 0.0.into() },
        ..container::Style::default()
    });
    if app.browser.drag.is_none() { return content.into(); }
    mouse_area(content).on_enter(Message::Enter(zone).into()).on_exit(Message::Leave(zone).into()).into()
}

/// A label that follows the cursor while a plugin is dragged.
pub fn drag_label(app: &App) -> Option<Element<'_, AppMessage>> {
    let drag = app.browser.drag.filter(|d| d.moved)?;
    let info = app.catalog.plugins.get(drag.plugin)?;
    let note = match drop_target(app) {
        Some((_, place)) => format!("add {} to {}", info.plugin.name, place_label(app, place)),
        None => info.plugin.name.clone(),
    };
    let tag = container(text(note).size(theme::SMALL).color(theme::BRIGHT)).padding([3, 6]).style(theme::popup);
    Some(iced::widget::pin(tag).x(app.cursor.x + 14.0).y(app.cursor.y + 10.0).into())
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
    .wrap()
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
                PluginFormat::Vst3 => "VST3",
                PluginFormat::AudioUnit => "AU",
            };
            let (kind, short) = match info.kind {
                PluginKind::Instrument => ("instrument", "inst"),
                PluginKind::Effect => ("effect", "fx"),
            };
            let held = state.drag.is_some_and(|d| d.plugin == index);
            let name = text(info.plugin.name.clone()).size(theme::SMALL).wrapping(text::Wrapping::None).width(Length::Fill);
            let tags = text(format!("{short} {format}")).size(theme::SMALL).color(theme::TEXT_DIM);
            let line = container(row![container(name).clip(true).width(Length::Fill), tags].spacing(6))
                .padding([3, 6])
                .width(Length::Fill)
                .style(move |_| container::Style { background: held.then(|| theme::CONTROL_HOVER.into()), ..container::Style::default() });
            let place = place_label(app, default_place(app, info));
            let hint = format!("{}\n{} · {format} · {kind}\nClick to add to {place}, or drag onto a channel or mixer insert", info.plugin.name, info.plugin.vendor);
            list = list.push(super::help(
                mouse_area(line).on_press(Message::Grab(index).into()).interaction(mouse::Interaction::Grab),
                hint,
            ));
        }
    }
    let body: Element<'_, AppMessage> = if app.catalog.plugins.is_empty() && app.scan.is_some() {
        container(label("scanning installed plugins")).padding(8).into()
    } else if app.catalog.plugins.is_empty() && state.filter != Filter::Failed {
        let lines = vec!["Install VST3 or Audio Unit plugins, then rescan.".to_string()];
        super::empty_state("no plugins found", &lines, vec![tool("rescan", AppMessage::Rescan)])
    } else {
        super::scroll(list, true, false).height(Length::Fill).into()
    };
    let filters = row![
        toggle("all", state.filter == Filter::All, Message::Filter(Filter::All).into()),
        toggle("inst", state.filter == Filter::Instruments, Message::Filter(Filter::Instruments).into()),
        toggle("fx", state.filter == Filter::Effects, Message::Filter(Filter::Effects).into()),
        toggle("failed", state.filter == Filter::Failed, Message::Filter(Filter::Failed).into()),
    ].spacing(2).wrap();
    column![container(search).padding(4), container(filters).padding([0, 4]), body].spacing(4).into()
}
