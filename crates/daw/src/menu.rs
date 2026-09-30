//! Right-click menu for channels, patterns, automation clips, playlist
//! tracks, and mixer inserts. Every menu can rename its item.

use daw_model::{AutomationId, ChannelId, InsertId, PatternId, Project};
use iced::widget::{Column, Space, button, container, mouse_area, opaque, operation, pin, stack, text, text_input};
use iced::{Element, Length, Point, Task};

use crate::app::{App, Message as AppMessage};
use crate::panels::{channel_rack, mixer, playlist};
use crate::theme;

const WIDTH: f32 = 160.0;
const ROW_HEIGHT: f32 = 21.0;
const INPUT: &str = "menu-rename";

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Item {
    Channel(ChannelId),
    Pattern(PatternId),
    Automation(AutomationId),
    Track(usize),
    Insert(InsertId),
}

#[derive(Debug)]
pub struct Menu {
    pub item: Item,
    at: Point,
    /// The new name while renaming.
    pub rename: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// Select the item and open its menu at the cursor.
    Open(Item),
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
    }
}

fn name_mut(project: &mut Project, item: Item) -> Option<&mut String> {
    match item {
        Item::Channel(id) => project.channel_mut(id).map(|c| &mut c.name),
        Item::Pattern(id) => project.pattern_mut(id).map(|p| &mut p.name),
        Item::Automation(id) => project.automation_clip_mut(id).map(|a| &mut a.name),
        Item::Track(index) => project.playlist.tracks.get_mut(index).map(|t| &mut t.name),
        Item::Insert(id) => project.mixer.insert_mut(id).map(|i| &mut i.name),
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
        Message::Open(item) => {
            match item {
                Item::Channel(id) => channel_rack::select(app, id),
                Item::Insert(id) => app.selected_insert = id,
                Item::Track(track) => playlist::update(app, playlist::Message::SelectTrack(track)),
                Item::Pattern(_) | Item::Automation(_) => {}
            }
            app.menu = Some(Menu { item, at: app.cursor, rename: None });
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
        Item::Insert(_) | Item::Automation(_) => Vec::new(),
    }
}

fn entry<'a>(label: &str, message: AppMessage) -> Element<'a, AppMessage> {
    button(text(label.to_string()).size(theme::SMALL))
        .on_press(message)
        .style(theme::menu_entry)
        .padding([3, 8])
        .width(Length::Fill)
        .into()
}

pub fn view<'a>(app: &'a App, menu: &'a Menu) -> Element<'a, AppMessage> {
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
            let mut column = Column::new().push(entry("rename", Message::Rename.into()));
            let entries = entries(menu.item);
            let rows = entries.len() + 1;
            for (label, message) in entries {
                column = column.push(entry(label, Message::Choose(Box::new(message)).into()));
            }
            (column.into(), rows)
        }
    };
    let popup = container(body).width(WIDTH).padding(1).style(theme::popup);
    // Keep the whole menu inside the window.
    let x = menu.at.x.min(app.window.width - WIDTH).max(0.0);
    let y = menu.at.y.min(app.window.height - rows as f32 * ROW_HEIGHT - 2.0).max(0.0);
    let backdrop = mouse_area(Space::new().width(Length::Fill).height(Length::Fill))
        .on_press(Message::Close.into())
        .on_right_press(Message::Close.into());
    stack![backdrop, pin(opaque(popup)).position(Point::new(x, y))].into()
}
