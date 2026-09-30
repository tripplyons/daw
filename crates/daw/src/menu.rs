//! Item context menus, panel tools, and transport overflow menus.

use daw_model::{AutomationId, ChannelId, ClipId, InsertId, PatternId, Project};
use daw_model::layout::{Axis, Panel, TileId};
use iced::widget::{Column, Space, button, container, mouse_area, opaque, operation, pin, stack, text, text_input};
use iced::{Element, Length, Point, Task};

use crate::app::{App, Message as AppMessage};
use crate::panels::{self, channel_rack, mixer, playlist};
use crate::keys::Action;
use crate::theme;

const WIDTH: f32 = 220.0;
const ROW_HEIGHT: f32 = 26.0;
const INPUT: &str = "menu-rename";

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Item {
    Channel(ChannelId),
    Pattern(PatternId),
    Automation(AutomationId),
    Track(usize),
    Insert(InsertId),
    Clip(ClipId),
    Tile(TileId),
    Tools(TileId),
    Files,
    PatternTools,
}

#[derive(Debug)]
pub struct Menu {
    pub item: Item,
    at: Point,
    /// The new name while renaming.
    pub rename: Option<String>,
    selected: Option<usize>,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// Select the item and open its menu at the cursor.
    Open(Item),
    OpenAt(Item, Point),
    Move(i32),
    Activate,
    Rename,
    Input(String),
    /// Enter, or a click outside the menu: close it, keeping a typed name.
    /// Escape closes it without renaming.
    Close,
    /// Run an entry's message and close the menu.
    Choose(Box<AppMessage>),
}

impl From<Message> for AppMessage {
    fn from(message: Message) -> Self {
        AppMessage::Menu(message)
    }
}

fn name(project: &Project, item: Item) -> Option<&str> {
    match item {
        Item::Channel(id) => project.channel(id).map(|c| c.name.as_str()),
        Item::Pattern(id) => project.pattern(id).map(|p| p.name.as_str()),
        Item::Automation(id) => project.automation_clip(id).map(|a| a.name.as_str()),
        Item::Track(index) => project.playlist.tracks.get(index).map(|t| t.name.as_str()),
        Item::Insert(id) => project.mixer.insert(id).map(|i| i.name.as_str()),
        Item::Clip(_) | Item::Tile(_) | Item::Tools(_) | Item::Files | Item::PatternTools => None,
    }
}

fn name_mut(project: &mut Project, item: Item) -> Option<&mut String> {
    match item {
        Item::Channel(id) => project.channel_mut(id).map(|c| &mut c.name),
        Item::Pattern(id) => project.pattern_mut(id).map(|p| &mut p.name),
        Item::Automation(id) => project.automation_clip_mut(id).map(|a| &mut a.name),
        Item::Track(index) => project.playlist.tracks.get_mut(index).map(|t| &mut t.name),
        Item::Insert(id) => project.mixer.insert_mut(id).map(|i| &mut i.name),
        Item::Clip(_) | Item::Tile(_) | Item::Tools(_) | Item::Files | Item::PatternTools => None,
    }
}

/// Rename an item as one undo step. Blank or unchanged names do nothing.
pub fn rename(app: &mut App, item: Item, new: &str) {
    let new = new.trim();
    if new.is_empty() || name(&app.project, item).is_none_or(|old| old == new) {
        return;
    }
    app.checkpoint();
    if let Some(slot) = name_mut(&mut app.project, item) {
        *slot = new.to_owned();
    }
    app.set_status(format!("renamed to {new}"));
    app.edited();
}

pub fn update(app: &mut App, message: Message) -> Task<AppMessage> {
    match message {
        Message::Open(item) => return update(app, Message::OpenAt(item, app.cursor)),
        Message::OpenAt(item, at) => {
            match item {
                Item::Channel(id) => channel_rack::select(app, id),
                Item::Insert(id) => app.selected_insert = id,
                Item::Track(track) => playlist::update(app, playlist::Message::SelectTrack(track)),
                Item::Clip(id) => playlist::update(app, playlist::Message::SelectClip(id)),
                Item::Tile(tile) | Item::Tools(tile) => {
                    if !app.layout().leaves().contains(&tile) { return Task::none(); }
                    app.layout_mut().focused = tile;
                }
                Item::Pattern(_) | Item::Automation(_) | Item::Files | Item::PatternTools => {}
            }
            app.menu = Some(Menu { item, at, rename: None, selected: None });
        }
        Message::Move(delta) => {
            let Some(menu) = &mut app.menu else { return Task::none() };
            let count = entries(menu.item).len() + usize::from(name(&app.project, menu.item).is_some());
            if count == 0 { return Task::none(); }
            let current = menu.selected.map(|n| n as i32).unwrap_or(if delta > 0 { -1 } else { 0 });
            menu.selected = Some((current + delta).rem_euclid(count as i32) as usize);
        }
        Message::Activate => {
            let Some(menu) = &app.menu else { return Task::none() };
            let rename = name(&app.project, menu.item).is_some();
            let selected = menu.selected.unwrap_or(0);
            if rename && selected == 0 { return update(app, Message::Rename); }
            if let Some((_, message)) = entries(menu.item).into_iter().nth(selected - usize::from(rename)) {
                return update(app, Message::Choose(Box::new(message)));
            }
        }
        Message::Rename => {
            let Some(menu) = &mut app.menu else { return Task::none() };
            menu.rename = name(&app.project, menu.item).map(str::to_owned);
            return operation::focus(INPUT).chain(operation::select_all(INPUT));
        }
        Message::Input(text) => {
            if let Some(menu) = &mut app.menu {
                menu.rename = Some(text);
            }
        }
        Message::Close => {
            if let Some(Menu { item, rename: Some(new), .. }) = app.menu.take() {
                rename(app, item, &new);
            }
        }
        Message::Choose(message) => {
            app.menu = None;
            return app.update(*message);
        }
    }
    Task::none()
}

/// Entries after rename, with the message each one sends.
fn entries(item: Item) -> Vec<(&'static str, AppMessage)> {
    match item {
        Item::Channel(id) => vec![
            ("preview", channel_rack::Message::Preview(id).into()),
            ("delete", channel_rack::Message::Remove.into()),
        ],
        Item::Track(_) => vec![("delete", playlist::Message::DeleteTrack.into())],
        Item::Insert(id) if id != daw_model::MASTER => vec![("delete", mixer::Message::DeleteInsert.into())],
        Item::Pattern(id) => vec![("clone pattern", AppMessage::ClonePattern(id))],
        Item::Clip(id) => vec![
            ("open", playlist::Message::Open(id).into()),
            ("duplicate", playlist::Message::Duplicate.into()),
            ("make unique", playlist::Message::MakeUnique.into()),
            ("consolidate", playlist::Message::Consolidate.into()),
            ("mute / unmute", playlist::Message::MuteSelection.into()),
            ("delete", playlist::Message::DeleteSelection.into()),
        ],
        Item::Tile(tile) => vec![
            ("focus / restore", AppMessage::TileAction(tile, Action::Zoom)),
            ("split side by side", AppMessage::SplitTile(tile, Axis::Horizontal)),
            ("split stacked", AppMessage::SplitTile(tile, Axis::Vertical)),
            ("close tile", AppMessage::TileAction(tile, Action::Close)),
        ],
        Item::Files => vec![
            ("new", AppMessage::Action(Action::New)),
            ("open", AppMessage::Action(Action::Open)),
            ("save", AppMessage::Action(Action::Save)),
            ("save as", AppMessage::Action(Action::SaveAs)),
            ("export WAV", AppMessage::Action(Action::Export)),
            ("import audio", AppMessage::Action(Action::ImportAudio)),
            ("recover backup", AppMessage::Action(Action::Recover)),
            ("package project", AppMessage::Action(Action::Pack)),
        ],
        Item::Tools(_) | Item::PatternTools => Vec::new(),
        Item::Insert(_) | Item::Automation(_) => Vec::new(),
    }
}

fn entry<'a>(label: &str, message: AppMessage, selected: bool) -> Element<'a, AppMessage> {
    button(text(label.to_string()).size(theme::SMALL))
        .on_press(message)
        .style(move |t, status| if selected { theme::toggle(true)(t, status) } else { theme::menu_entry(t, status) })
        .padding([3, 8])
        .width(Length::Fill)
        .into()
}

pub fn view<'a>(app: &'a App, menu: &'a Menu) -> Element<'a, AppMessage> {
    let width = if matches!(menu.item, Item::Tools(_) | Item::PatternTools) { 380.0 } else { WIDTH };
    let width = width.min((app.window.width - 16.0).max(1.0));
    let (body, rows): (Element<'a, AppMessage>, usize) = match &menu.rename {
        Some(new) => {
            let input = text_input("name", new)
                .id(INPUT)
                .on_input(|text| Message::Input(text).into())
                .on_submit(Message::Close.into())
                .size(theme::SMALL)
                .padding([3, 6])
                .style(theme::input);
            (input.into(), 1)
        }
        None => {
            if let Item::Tools(tile) = menu.item {
                let panel = app.layout().panel(tile);
                let tools = panels::toolbar(app, panel);
                let picker = panels::labeled("panel", panels::pick(Panel::ALL.to_vec(), Some(panel), move |p| AppMessage::SetPanel(tile, p)));
                let mut body = iced::widget::column![panels::label(format!("{panel} tools")), picker, tools, panels::label("tile")].spacing(8).padding(8);
                for (label, message) in entries(Item::Tile(tile)) {
                    body = body.push(entry(label, Message::Choose(Box::new(message)).into(), false));
                }
                return popup(app, menu, body.into(), width, 400.0);
            }
            if menu.item == Item::PatternTools {
                return popup(app, menu, app.pattern_tools(), width, 140.0);
            }
            let mut column = Column::new();
            let rename = name(&app.project, menu.item).is_some();
            if rename { column = column.push(entry("rename", Message::Rename.into(), menu.selected == Some(0))); }
            let entries = entries(menu.item);
            let rows = entries.len() + usize::from(rename);
            for (index, (label, message)) in entries.into_iter().enumerate() {
                let hint = match &message {
                    AppMessage::Action(action) | AppMessage::TileAction(_, action) => panels::action_hint(app, *action),
                    _ => panels::hint(&message).unwrap_or(label).into(),
                };
                column = column.push(panels::help(entry(label, Message::Choose(Box::new(message)).into(), menu.selected == Some(index + usize::from(rename))), hint));
            }
            (column.into(), rows)
        }
    };
    popup(app, menu, body, width, rows as f32 * ROW_HEIGHT + 2.0)
}

fn popup<'a>(app: &'a App, menu: &Menu, body: Element<'a, AppMessage>, width: f32, height: f32) -> Element<'a, AppMessage> {
    let height = height.min((app.window.height - 16.0).max(1.0));
    let popup = container(panels::scroll(body, true, false).height(Length::Shrink)).width(width).max_height(height).padding(1).style(theme::popup);
    // Keep the whole menu inside the window.
    let x = menu.at.x.min(app.window.width - width - 4.0).max(4.0);
    let y = menu.at.y.min(app.window.height - height - 4.0).max(4.0);
    let backdrop = mouse_area(Space::new().width(Length::Fill).height(Length::Fill))
        .on_press(Message::Close.into())
        .on_right_press(Message::Close.into());
    stack![backdrop, pin(opaque(popup)).position(Point::new(x, y))].into()
}
