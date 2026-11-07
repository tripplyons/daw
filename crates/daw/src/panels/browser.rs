//! Installed plugins from the scan cache. Clicking an instrument adds a
//! channel and clicking an effect puts it on the selected mixer insert.
//! Dragging a plugin onto the channel rack, a channel, or a mixer insert
//! chooses where it goes.

use daw_model::layout::Panel;
use daw_model::{ChannelId, InsertId, PluginFormat, Source};
use daw_plugins::{PluginInfo, PluginKind};
use rune::{Cursor, Drag as Gesture, El, Phase, Tx, col, el, row, text};

use super::label;
use crate::app::{App, Message as AppMessage};
use crate::input::Point;
use crate::task::Task;
use crate::theme::{self, C};
use crate::ui::{self, widgets::{self, Btn}};

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
    /// The drop zones under the pointer, outermost first.
    Over(Vec<DropZone>),
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

pub fn update(app: &mut App, message: Message) -> Task {
    match message {
        Message::Search(search) => app.browser.search = search,
        Message::Filter(filter) => app.browser.filter = filter,
        Message::Grab(plugin) => {
            app.browser.drag = Some(Drag { plugin, from: app.cursor, moved: false });
            app.browser.hover.clear();
        }
        Message::Over(zones) => {
            if app.browser.drag.is_some() { app.browser.hover = zones; }
        }
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
pub fn released(app: &mut App) -> Task {
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
pub fn add(app: &mut App, info: &PluginInfo, place: Place) -> Task {
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
    let status = format!("added {} to {}", info.plugin.name, place_label(app, place));
    let panel = match place {
        Place::Channel(_) => Panel::ChannelRack,
        Place::Effect(_) => Panel::Mixer,
    };
    if app.panel_visible(panel) {
        app.set_status(status);
    } else {
        app.set_status_with(status, "show", AppMessage::ShowPanel(panel));
    }
    Task::done(AppMessage::OpenPlugin(instance))
}

/// The key that marks a drop zone in the element tree.
fn zone_key(zone: DropZone) -> String {
    match zone {
        DropZone::Rack => "drop-rack".into(),
        DropZone::Channel(id) => format!("drop-ch-{}", id.0),
        DropZone::Insert(id) => format!("drop-ins-{}", id.0),
    }
}

/// The drop zones named in an element path, outermost first.
fn zones_in(path: &str) -> Vec<DropZone> {
    path.split('/').filter_map(|segment| {
        let rest = segment.strip_prefix("drop-")?;
        if rest == "rack" { return Some(DropZone::Rack); }
        if let Some(id) = rest.strip_prefix("ch-") { return Some(DropZone::Channel(ChannelId(id.parse().ok()?))); }
        let id = rest.strip_prefix("ins-")?;
        let id = id.split('-').next()?;
        Some(DropZone::Insert(InsertId(id.parse().ok()?)))
    }).collect()
}

/// Make `content` a place to drop plugins, outlined while a drop would land
/// there. `part` tells two zones for the same target apart.
pub fn zone(app: &App, zone: DropZone, content: El) -> El {
    zone_part(app, zone, "", content)
}

pub fn zone_part(app: &App, zone: DropZone, part: &str, content: El) -> El {
    let lit = drop_target(app).is_some_and(|(z, _)| z == zone);
    let key = if part.is_empty() { zone_key(zone) } else { format!("{}-{part}", zone_key(zone)) };
    let content = content.key(key);
    if lit { content.outline(1.5, -1.5, C.accent).fill(C.accent_soft) } else { content }
}

/// A label that follows the cursor while a plugin is dragged.
pub fn drag_label(app: &App) -> Option<El> {
    let drag = app.browser.drag.filter(|d| d.moved)?;
    let info = app.catalog.plugins.get(drag.plugin)?;
    let (note, ok) = match drop_target(app) {
        Some((_, place)) => (format!("Add {} to {}", info.plugin.name, place_label(app, place)), true),
        None => (info.plugin.name.clone(), false),
    };
    let tag = widgets::popover(row().h(26.0).px(10.0).gap(6.0).center_y()
        .child(el().size(6.0, 6.0).radius(3.0).fill(if ok { C.accent } else { C.text_faint }))
        .child(text(note).font(theme::LABEL).color(C.text).single_line()));
    Some(tag.absolute(app.cursor.x + 14.0, app.cursor.y + 10.0).key("drag-label"))
}

fn filters(app: &App) -> El {
    let filter = app.browser.filter;
    let item = |label: &str, f: Filter| (label.to_string(), filter == f, AppMessage::from(Message::Filter(f)), String::new());
    let mut items = vec![item("All", Filter::All), item("Instruments", Filter::Instruments), item("Effects", Filter::Effects)];
    // Only offer the failures when there are some.
    if !app.catalog.failed.is_empty() || filter == Filter::Failed {
        items.push(item("Failed", Filter::Failed));
    }
    widgets::segmented(items)
}

pub fn toolbar(app: &App) -> El {
    let rescan = if app.scan.is_some() { label("Scanning…") } else { super::tool("Rescan", AppMessage::Rescan) };
    super::bar(vec![filters(app), rescan])
}

/// A plugin row: press to add it, or drag it onto a channel or insert.
fn plugin_row(app: &App, index: usize, info: &PluginInfo) -> El {
    let format = match info.plugin.format {
        PluginFormat::Vst3 => "VST3",
        PluginFormat::AudioUnit => "AU",
    };
    let kind = match info.kind {
        PluginKind::Instrument => "Instrument",
        PluginKind::Effect => "Effect",
    };
    let held = app.browser.drag.is_some_and(|d| d.plugin == index);
    let dot = if info.kind == PluginKind::Instrument { C.accent } else { C.focus };
    let name = col().grow(1.0).min_w(0.0).gap(1.0)
        .child(text(info.plugin.name.clone()).font(theme::BODY).color(C.text).single_line().truncate())
        .child(text(info.plugin.vendor.clone()).font(theme::SMALL_TEXT).color(C.text_faint).single_line().truncate());
    let tag = text(format!("{kind} · {format}")).font(theme::SMALL_TEXT).color(C.text_faint).single_line();
    let place = place_label(app, default_place(app, info));
    let hint = format!("{}\n{} · {format} · {kind}\nClick to add to {place}, or drag onto a channel or mixer insert", info.plugin.name, info.plugin.vendor);
    let mut line = row().w_full().h(40.0).px(10.0).gap(10.0).center_y().radius(theme::RADIUS)
        .child(el().size(6.0, 6.0).radius(3.0).fill(dot))
        .child(name)
        .child(tag)
        .hover(C.hover)
        .cursor(if held { Cursor::Grabbing } else { Cursor::Grab })
        .help(hint)
        .on_drag(move |app: &mut App, d: &mut Gesture, tx: &mut Tx| {
            let [x, y] = d.window_point();
            let scale = f32::from(app.config.ui_scale) / 100.0;
            let point = Point::new(x / scale, y / scale);
            match d.phase {
                Phase::Down => {
                    ui::send(app, tx, AppMessage::MouseMoved(point));
                    ui::send(app, tx, Message::Grab(index).into());
                }
                Phase::Move => {
                    ui::send(app, tx, AppMessage::MouseMoved(point));
                    // Only the tiles hold drop zones; toasts and the drag label float above them.
                    tx.element_at_in("desk", x, y, |app: &mut App, picked, tx| {
                        let zones = picked.map(|p| zones_in(p.path())).unwrap_or_default();
                        ui::send(app, tx, Message::Over(zones).into());
                    });
                }
                Phase::Up => {
                    if d.cancelled() { app.browser.drag = None; }
                    ui::send(app, tx, AppMessage::MouseReleased);
                }
            }
        });
    if held { line = line.fill(C.raised_hover); }
    line
}

pub fn view(app: &App) -> El {
    let state = &app.browser;
    let search = row().px(10.0).pt(10.0).pb(6.0).w_full()
        .child(widgets::field(app, "browser-search", &state.search, "Search plugins", |s| Message::Search(s).into(), None).w_full());
    let mut list = col().gap(1.0).px(4.0).pb(8.0).w_full();
    if state.filter == Filter::Failed {
        for failure in app.catalog.failed.iter().filter(|f| matches(&state.search, &[&f.name, &f.error])) {
            list = list.child(col().px(10.0).py(6.0).gap(2.0)
                .child(text(failure.name.clone()).font(theme::BODY).color(C.text).single_line().truncate())
                .child(text(failure.error.clone()).font(theme::SMALL_TEXT).color(C.danger).wrap()));
        }
        if app.catalog.failed.is_empty() {
            list = list.child(row().padding(10.0).child(label("No plugins failed to load.")));
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
            list = list.child(plugin_row(app, index, info));
        }
    }
    let body = if app.catalog.plugins.is_empty() && app.scan.is_some() {
        widgets::empty_state("Scanning plugins", &["Installed VST3 and Audio Unit plugins appear here as they are found.".to_string()], Vec::new())
    } else if app.catalog.plugins.is_empty() && state.filter != Filter::Failed {
        let lines = vec!["Install VST3 or Audio Unit plugins, then scan again.".to_string()];
        widgets::empty_state("No plugins found", &lines, vec![Btn::new("Rescan").build(AppMessage::Rescan)])
    } else {
        super::scroll(list, true, false)
    };
    let mut top = col().w_full().gap(6.0).child(search).child(row().px(10.0).child(filters(app)));
    // Name where a click puts effects, since the mixer may not be on screen.
    if state.filter != Filter::Instruments && state.filter != Filter::Failed {
        let insert = app.project.insert_label(app.selected_insert);
        top = top.child(row().px(12.0).pb(4.0).gap(4.0)
            .child(label("Effects go to"))
            .child(text(insert).font(theme::SMALL_TEXT).color(C.text).single_line())
            .help("Clicking an effect adds it to the selected mixer insert. Select another insert in the mixer, or drag the effect onto a channel or insert."));
    }
    col().w_full().h_full().child(top).child(body)
}
