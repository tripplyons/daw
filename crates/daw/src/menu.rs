//! Item context menus, panel tools, and transport overflow menus.

use daw_model::{AutomationId, ChannelId, ClipId, InsertId, PatternId, Project};
use daw_model::layout::{Axis, Panel, TileId};
use rune::{Cursor, El, Tx, col, el, row, text};

use crate::app::{App, Message as AppMessage};
use crate::input::Point;
use crate::keys::Action;
use crate::panels::{self, channel_rack, mixer, playlist};
use crate::task::Task;
use crate::theme::{self, C};
use crate::ui::{self, widgets};

const WIDTH: f32 = 232.0;
const ROW_HEIGHT: f32 = 28.0;
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

pub fn update(app: &mut App, message: Message) -> Task {
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
            return Task::focus(INPUT);
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
            ("Preview", channel_rack::Message::Preview(id).into()),
            ("Delete", channel_rack::Message::Remove.into()),
        ],
        Item::Track(_) => vec![("Delete", playlist::Message::DeleteTrack.into())],
        Item::Insert(id) if id != daw_model::MASTER => vec![("Delete", mixer::Message::DeleteInsert.into())],
        Item::Pattern(id) => vec![("Clone pattern", AppMessage::ClonePattern(id))],
        Item::Clip(id) => vec![
            ("Open", playlist::Message::Open(id).into()),
            ("Duplicate", playlist::Message::Duplicate.into()),
            ("Make unique", playlist::Message::MakeUnique.into()),
            ("Consolidate", playlist::Message::Consolidate.into()),
            ("Mute or unmute", playlist::Message::MuteSelection.into()),
            ("Delete", playlist::Message::DeleteSelection.into()),
        ],
        Item::Tile(tile) => vec![
            ("Focus or restore", AppMessage::TileAction(tile, Action::Zoom)),
            ("Split side by side", AppMessage::SplitTile(tile, Axis::Horizontal)),
            ("Split stacked", AppMessage::SplitTile(tile, Axis::Vertical)),
            ("Close tile", AppMessage::TileAction(tile, Action::Close)),
        ],
        Item::Files => vec![
            ("New project", AppMessage::Action(Action::New)),
            ("Open…", AppMessage::Action(Action::Open)),
            ("Save", AppMessage::Action(Action::Save)),
            ("Save as…", AppMessage::Action(Action::SaveAs)),
            ("Export WAV…", AppMessage::Action(Action::Export)),
            ("Import audio…", AppMessage::Action(Action::ImportAudio)),
            ("Recover backup…", AppMessage::Action(Action::Recover)),
            ("Package project…", AppMessage::Action(Action::Pack)),
        ],
        Item::Tools(_) | Item::PatternTools => Vec::new(),
        Item::Insert(_) | Item::Automation(_) => Vec::new(),
    }
}

/// The key that does what an entry does, so menus teach shortcuts. Actions
/// follow the keymap; panel keys are fixed.
fn shortcut(app: &App, message: &AppMessage) -> Option<String> {
    let action = match message {
        AppMessage::Action(action) | AppMessage::TileAction(_, action) => *action,
        AppMessage::Playlist(playlist::Message::Duplicate) => return Some("cmd+d".into()),
        AppMessage::Playlist(playlist::Message::MakeUnique) => return Some("cmd+u".into()),
        AppMessage::Playlist(playlist::Message::Consolidate) => return Some("cmd+shift+c".into()),
        AppMessage::Playlist(playlist::Message::MuteSelection) => return Some("m".into()),
        AppMessage::Playlist(playlist::Message::DeleteSelection | playlist::Message::DeleteTrack)
        | AppMessage::Rack(channel_rack::Message::Remove)
        | AppMessage::Mixer(mixer::Message::DeleteInsert) => Action::Delete,
        _ => return None,
    };
    let binding = crate::keys::BINDINGS.iter().find(|b| b.action == action)?;
    app.keymap.chords(binding.id).first().map(ToString::to_string)
}

fn entry(label: &str, shortcut: Option<String>, message: AppMessage, selected: bool) -> El {
    let keys = text(shortcut.as_deref().map(crate::keys::pretty).unwrap_or_default()).font(theme::LABEL).color(C.text_faint).single_line();
    let line = row().w_full().h(ROW_HEIGHT).px(10.0).gap(12.0).center_y().radius(theme::RADIUS - 2.0)
        .child(text(label.to_string()).font(theme::BODY).color(C.text).single_line().grow(1.0).shrink().min_w(0.0))
        .child(keys)
        .hover(C.hover)
        .press(C.press)
        .cursor(Cursor::Pointer)
        .role("menuitem")
        .on_press_tx(ui::on(message));
    if selected { line.fill(C.hover) } else { line }
}

pub fn view(app: &App, menu: &Menu) -> El {
    let width = if matches!(menu.item, Item::Tools(_) | Item::PatternTools) { 400.0 } else { WIDTH };
    let width = width.min((app.window.width - 16.0).max(1.0));
    let (body, height) = match &menu.rename {
        Some(new) => {
            let input = widgets::field(app, INPUT, new, "Name", |text| Message::Input(text).into(), Some(Message::Close.into()));
            let body = col().gap(6.0).padding(10.0)
                .child(widgets::section("Rename"))
                .child(input.w_full())
                .child(text("Enter keeps the name; Escape cancels.").font(theme::SMALL_TEXT).color(C.text_faint));
            (body, 96.0)
        }
        None => {
            if let Item::Tools(tile) = menu.item {
                let panel = app.layout().panel(tile);
                let tools = panels::toolbar(app, panel);
                let picker = panels::labeled("Panel", panels::pick("menu-panel", Panel::ALL.to_vec(), Some(panel), move |p| AppMessage::SetPanel(tile, p)));
                let mut body = col().gap(8.0).padding(10.0)
                    .child(widgets::section(format!("{panel} tools")))
                    .child(picker)
                    .child(tools)
                    .child(widgets::rule())
                    .child(widgets::section("Tile"));
                let mut list = col().gap(1.0).mx(-6.0);
                for (label, message) in entries(Item::Tile(tile)) {
                    let keys = shortcut(app, &message);
                    list = list.child(entry(label, keys, Message::Choose(Box::new(message)).into(), false));
                }
                body = body.child(list);
                return popup(app, menu, body, width, 420.0);
            }
            if menu.item == Item::PatternTools {
                return popup(app, menu, col().padding(10.0).child(app.pattern_tools()), width, 160.0);
            }
            let mut list = col().gap(1.0).padding(4.0);
            let rename = name(&app.project, menu.item).is_some();
            if let Some(title) = name(&app.project, menu.item) {
                list = list.child(row().h(24.0).px(10.0).center_y().child(text(title.to_string()).font(theme::SECTION).color(C.text_faint).single_line().truncate()));
                list = list.child(entry("Rename…", None, Message::Rename.into(), menu.selected == Some(0)));
            }
            let entries = entries(menu.item);
            let mut rows = entries.len() + usize::from(rename);
            for (index, (label, message)) in entries.into_iter().enumerate() {
                let hint = match &message {
                    AppMessage::Action(action) | AppMessage::TileAction(_, action) => panels::action_hint(app, *action),
                    _ => panels::hint(&message).unwrap_or(label).into(),
                };
                let keys = shortcut(app, &message);
                list = list.child(panels::help(entry(label, keys, Message::Choose(Box::new(message)).into(), menu.selected == Some(index + usize::from(rename))), hint));
            }
            if let Item::Channel(id) = menu.item {
                list = list.child(swatches(app, id));
                rows += 1;
            }
            let extra = if rename { 24.0 } else { 0.0 };
            (list, rows as f32 * (ROW_HEIGHT + 1.0) + 10.0 + extra)
        }
    };
    popup(app, menu, body, width, height)
}

/// A row of channel colors; clicking one recolors the channel and closes the menu.
fn swatches(app: &App, id: daw_model::ChannelId) -> El {
    let current = app.project.channel_color(id);
    let mut swatches = row().gap(4.0).px(10.0).py(6.0).flex_wrap();
    for color in daw_model::CHANNEL_COLORS {
        let fill = theme::rgb(color);
        let chosen = color == current;
        let message: AppMessage = Message::Choose(Box::new(channel_rack::Message::Color(id, color).into())).into();
        let mut swatch = el().size(16.0, 16.0).radius(8.0).fill(fill).cursor(Cursor::Pointer).help("Channel color")
            .on_press_tx(ui::on(message));
        swatch = if chosen { swatch.outline(2.0, 2.0, C.text) } else { swatch.hover(rune::Color::WHITE.alpha(0.2)) };
        swatches = swatches.child(swatch);
    }
    swatches
}

fn popup(app: &App, menu: &Menu, body: El, width: f32, height: f32) -> El {
    let height = height.min((app.window.height - 16.0).max(1.0));
    // Keep the whole menu inside the window.
    let x = menu.at.x.min(app.window.width - width - 4.0).max(4.0);
    let y = menu.at.y.min(app.window.height - height - 4.0).max(4.0);
    let panel = widgets::popover(col().w(width).max_h(height).child(body.w_full().scroll(rune::ScrollSpec { stick_bottom: false })))
        .key("context-menu")
        .absolute(x, y)
        .role("menu")
        .on_press(|_: &mut App| {});
    let close = |app: &mut App, tx: &mut Tx| ui::send(app, tx, Message::Close.into());
    let backdrop = el().absolute(0.0, 0.0).w_full().h_full()
        .on_press_tx(close)
        .on_secondary_press(move |app: &mut App, _, tx: &mut Tx| close(app, tx));
    el().absolute(0.0, 0.0).w(app.window.width).h(app.window.height).child(backdrop).child(panel)
}
