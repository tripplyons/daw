//! The window: transport, background jobs, and the tiled panels.

use daw_engine::song::PlayMode;
use daw_model::layout::{Axis, Node, Panel, Rect, Split, TileId};
use daw_model::PatternId;
use daw_plugins::scan::Progress;
use iced::widget::{Space, Stack, column, container, mouse_area, pick_list, pin, progress_bar, responsive, row, rule, stack, text, text_input};
use iced::{Element, Length, mouse};

use super::{App, JOBS_HEIGHT, Message, TRANSPORT_HEIGHT};
use crate::keys::Action;
use crate::menu;
use crate::panels::{self, automation, browser, channel_rack, mixer, parameters, piano_roll, playlist, settings};
use crate::theme;

/// Width of the line between tiles.
pub(super) const GUTTER: f32 = 1.0;
/// Width of the invisible handle that drags a split line.
const HANDLE: f32 = 8.0;

impl App {
    pub fn view(&self) -> Element<'_, Message> {
        let layout = self.layout();
        let tiles = if layout.zoomed { self.tile(layout.focused) } else { stack![self.node(&layout.root), self.tile_overlay()].into() };
        let mut base = column![self.transport()];
        if self.showing_jobs() { base = base.push(self.jobs()); }
        let base = base.push(tiles);
        match &self.menu {
            Some(open) => stack![base, menu::view(self, open)].into(),
            None => base.into(),
        }
    }

    fn jobs(&self) -> Element<'_, Message> {
        let mut jobs = row![].spacing(8).align_y(iced::Alignment::Center);
        if let Some(progress) = self.session.audio_progress() {
            jobs = jobs.push(job("audio", progress, Message::CancelAudio));
        } else if self.session.preparation_error.is_some() {
            jobs = jobs.push(panels::tool("retry audio processing", Message::RetryAudio));
        }
        if let Some((label, progress)) = self.rendering.progress() {
            jobs = jobs.push(job(label, progress, Message::CancelRender));
        }
        container(jobs).height(JOBS_HEIGHT).padding([2, 4]).into()
    }

    fn node<'a>(&'a self, node: &Node) -> Element<'a, Message> {
        match node {
            Node::Leaf(id) => self.tile(*id),
            Node::Split { axis, ratio, first, second } => {
                let a = ((ratio * 1000.0).round() as u16).max(1);
                let b = (1000u16.saturating_sub(a)).max(1);
                let line = container(Space::new()).style(theme::fill(theme::LINE));
                match axis {
                    Axis::Horizontal => row![
                        container(self.node(first)).width(Length::FillPortion(a)),
                        line.width(GUTTER).height(Length::Fill),
                        container(self.node(second)).width(Length::FillPortion(b))
                    ]
                    .height(Length::Fill)
                    .into(),
                    Axis::Vertical => column![
                        container(self.node(first)).height(Length::FillPortion(a)),
                        line.height(GUTTER).width(Length::Fill),
                        container(self.node(second)).height(Length::FillPortion(b))
                    ]
                    .width(Length::Fill)
                    .into(),
                }
            }
        }
    }

    /// Split handles wider than the lines they drag, and the focused tile's outline, drawn over the tiles.
    fn tile_overlay(&self) -> Element<'_, Message> {
        let layout = self.layout();
        let area = self.tile_area();
        let area = Rect { x: 0.0, y: 0.0, ..area };
        let mut layers: Vec<Element<'_, Message>> = layout.splits_with_gap(area, GUTTER).into_iter().map(|split| {
            let rect = handle_rect(&split);
            let interaction = match split.axis {
                Axis::Horizontal => mouse::Interaction::ResizingHorizontally,
                Axis::Vertical => mouse::Interaction::ResizingVertically,
            };
            let handle = mouse_area(Space::new().width(rect.width).height(rect.height))
                .on_press(Message::SplitDrag(split.path))
                .interaction(interaction);
            pin(handle).x(rect.x).y(rect.y).into()
        }).collect();
        if layout.leaves().len() > 1 && let Some((_, rect)) = layout.rects_with_gap(area, GUTTER).into_iter().find(|(id, _)| *id == layout.focused) {
            // Cover the shared lines around the tile, staying inside the window at its edges.
            let x = (rect.x - GUTTER).max(0.0);
            let y = (rect.y - GUTTER).max(0.0);
            let right = (rect.x + rect.width + GUTTER).min(area.width);
            let bottom = (rect.y + rect.height + GUTTER).min(area.height);
            let outline = container(Space::new()).width(right - x).height(bottom - y).style(|_| container::Style {
                border: iced::Border { color: theme::TEXT_DIM, width: 1.0, radius: 0.0.into() },
                ..container::Style::default()
            });
            layers.push(pin(outline).x(x).y(y).into());
        }
        Stack::with_children(layers).width(Length::Fill).height(Length::Fill).into()
    }

    fn tile(&self, id: TileId) -> Element<'_, Message> {
        let layout = self.layout();
        let panel = layout.panel(id);
        let focused = layout.focused == id;
        let highlight = focused && !layout.zoomed && layout.leaves().len() > 1;
        let header = responsive(move |size| {
            let more = panels::help(
                panels::tool("more", menu::Message::Open(menu::Item::Tools(id)).into()),
                format!("{panel}: {}", panels::action_hint(self, Action::PanelTools)),
            );
            let mut bar = row![].spacing(4).align_y(iced::Alignment::Center);
            if size.width >= 120.0 {
                let picker = pick_list(Panel::ALL, Some(panel), move |p| Message::SetPanel(id, p))
                    .text_size(theme::SMALL)
                    .width((size.width - 66.0).clamp(40.0, 110.0))
                    .padding([3, 6])
                    .style(theme::pick)
                    .menu_style(theme::menu);
                bar = bar.push(panels::help(picker, "Change this tile's panel"));
                let extra = if size.width >= 300.0 { 72.0 } else { 0.0 };
                let available = (size.width - 170.0 - extra).max(0.0);
                bar = bar.push(container(self.primary_tools(panel, available)).width(Length::Fill));
            } else {
                bar = bar.push(container(text(panel.to_string()).size(theme::SMALL).wrapping(iced::widget::text::Wrapping::None))
                    .width(Length::Fill).clip(true));
            }
            bar = bar.push(more);
            if size.width >= 300.0 {
                let focus = panels::tool(if layout.zoomed { "restore" } else { "focus" }, Message::TileAction(id, Action::Zoom));
                bar = bar.push(panels::help(focus, panels::action_hint(self, Action::Zoom)));
                bar = bar.push(panels::help(
                    panels::tool("tile", menu::Message::Open(menu::Item::Tile(id)).into()),
                    "Split or close this tile",
                ));
            }
            container(bar).padding([0, 4]).height(theme::HEADER_HEIGHT).center_y(theme::HEADER_HEIGHT)
                .style(move |t| {
                    let mut style = theme::header(t);
                    if highlight { style.background = Some(theme::CONTROL.into()); }
                    style
                }).into()
        }).height(theme::HEADER_HEIGHT);
        let body = match panel {
            Panel::Browser => browser::view(self),
            Panel::ChannelRack => channel_rack::view(self),
            Panel::PianoRoll => piano_roll::view(self, focused),
            Panel::Playlist => playlist::view(self, focused),
            Panel::Mixer => mixer::view(self),
            Panel::Automation => automation::view(self, focused),
            Panel::Parameters => parameters::view(self),
            Panel::Settings => settings::view(self),
        };
        container(column![header, container(body).width(Length::Fill).height(Length::Fill).clip(true)])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::panel)
            .into()
    }

    fn primary_tools(&self, panel: Panel, width: f32) -> Element<'_, Message> {
        let mut tools = row![].spacing(4).align_y(iced::Alignment::Center);
        match panel {
            Panel::Browser if width >= 52.0 => tools = tools.push(panels::tool("rescan", Message::Rescan)),
            Panel::Playlist => {
                if width >= 62.0 { tools = tools.push(panels::action(self, "+ audio", Action::ImportAudio, None)); }
                if width >= 120.0 { tools = tools.push(panels::tool("+ track", playlist::Message::AddTrack.into())); }
                if width >= 190.0 { tools = tools.push(panels::toggle("stretch", self.playlist.stretch_mode, playlist::Message::StretchMode.into())); }
            }
            Panel::ChannelRack => {
                if width >= 64.0 { tools = tools.push(panels::tool("+ synth", channel_rack::Message::AddSynth.into())); }
                if width >= 134.0 { tools = tools.push(panels::tool("+ sample", channel_rack::Message::AddSampler.into())); }
            }
            Panel::PianoRoll if width >= 95.0 => tools = tools.push(row![
                panels::label("snap"),
                panels::pick(daw_model::time::Grid::CHOICES.to_vec(), Some(self.project.grid), |g| piano_roll::Message::Grid(g).into()),
            ].spacing(4).align_y(iced::Alignment::Center)),
            Panel::Mixer if width >= 70.0 => tools = tools.push(panels::tool("+ insert", mixer::Message::AddInsert.into())),
            Panel::Automation if width >= 132.0 => {
                use automation::Tool;
                for (label, tool) in [("edit", Tool::Edit), ("draw", Tool::Draw), ("line", Tool::Line)] {
                    tools = tools.push(panels::toggle(label, self.automation.tool == tool, automation::Message::Tool(tool).into()));
                }
            }
            _ => {}
        }
        tools.into()
    }

    fn pattern_picker(&self, width: f32) -> Element<'_, Message> {
        let patterns: Vec<PatternChoice> = self.project.patterns.iter().map(|p| PatternChoice { id: p.id, name: p.name.clone() }).collect();
        let selected = patterns.iter().find(|p| p.id == self.selected_pattern).cloned();
        panels::help(mouse_area(
            pick_list(patterns, selected, |p: PatternChoice| Message::SelectPattern(p.id))
                .text_size(theme::SMALL).width(width).padding([3, 6]).style(theme::pick).menu_style(theme::menu)
        ).on_right_press(menu::Message::Open(menu::Item::Pattern(self.selected_pattern)).into()),
            "Selected pattern; right-click to rename or clone")
    }

    pub(crate) fn pattern_tools(&self) -> Element<'_, Message> {
        let bar = self.project.signature.ticks_per_bar();
        let bars = self.project.pattern(self.selected_pattern).map(|p| p.length.div_ceil(bar).max(1));
        let mut choices: Vec<u64> = (1..=16).collect();
        if let Some(bars) = bars && !choices.contains(&bars) { choices.push(bars); }
        column![
            panels::label("pattern"), self.pattern_picker(250.0),
            row![panels::label("length"), panels::pick(choices, bars, Message::PatternBars), panels::label("bars")].spacing(6).align_y(iced::Alignment::Center),
            row![panels::tool("+ pattern", Message::NewPattern), panels::tool("clone", Message::ClonePattern(self.selected_pattern))].spacing(4),
        ].spacing(8).padding(8).into()
    }

    fn transport(&self) -> Element<'_, Message> {
        let beats = self.position / f64::from(daw_model::time::TICKS_PER_BEAT);
        let beats_per_bar = f64::from(self.project.signature.numerator);
        let position = format!("{}.{}.{:02}", (beats / beats_per_bar).floor() as u32 + 1,
            (beats % beats_per_bar).floor() as u32 + 1, ((beats.fract()) * 100.0).floor() as u32);
        let mode = match self.mode { PlayMode::Song => "song", PlayMode::Pattern(_) => "pattern" };
        let play = panels::action(self, if self.playing { "pause" } else { "play" }, Action::PlayPause, Some(self.playing));
        let playback = row![
            play,
            panels::action(self, "rewind", Action::Stop, None),
            panels::action(self, mode, Action::ToggleMode, None),
            panels::help(text(position).size(theme::TEXT_SIZE).font(iced::Font::MONOSPACE).width(70), "Position: bar.beat.fraction"),
            text_input("bpm", &self.bpm_text.clone().unwrap_or_else(|| format!("{}", self.project.bpm)))
                .id("tempo").on_submit(Message::BpmDone).on_input(Message::SetBpm).size(theme::SMALL)
                .width(48).padding([3, 4]).style(theme::input),
            panels::label("bpm"),
        ].spacing(4).align_y(iced::Alignment::Center);
        let pattern: Element<'_, Message> = if self.window.width >= 880.0 {
            row![self.pattern_picker(150.0), panels::tool("more", menu::Message::Open(menu::Item::PatternTools).into())].spacing(4).into()
        } else { panels::tool("pattern", menu::Message::Open(menu::Item::PatternTools).into()) };
        let mut files = row![].spacing(4).align_y(iced::Alignment::Center);
        if self.window.width >= 1000.0 { files = files.push(panels::action(self, "save", Action::Save, None)); }
        if self.window.width >= 1200.0 { files = files.push(panels::action(self, "export", Action::Export, None)); }
        files = files.push(panels::help(panels::tool("file", menu::Message::Open(menu::Item::Files).into()), "New, open, save, import, and export"))
            .push(panels::action(self, "settings", Action::Settings, None));
        let top = row![playback, divider(), pattern, Space::new().width(Length::Fill), files].spacing(8).align_y(iced::Alignment::Center);
        let recording = row![
            panels::label("record"),
            panels::action(self, "automation", Action::ToggleRecord, Some(self.record)),
            panels::action(self, "MIDI", Action::ToggleMidiRecord, Some(self.midi.recording)),
            panels::action(self, "audio", Action::ToggleAudioRecord, Some(self.session.recording_armed())),
            divider(), panels::action(self, "bind automation", Action::ToggleBind, Some(self.bind_mode)),
        ].spacing(4).align_y(iced::Alignment::Center);
        let scan = match self.scan {
            Some(Progress { done, total }) if total > 0 => format!("scanning plugins {done}/{total}"),
            Some(_) => "scanning plugins".into(), None => self.status.clone(),
        };
        let status = container(panels::help(
            text(scan.clone()).size(theme::SMALL).color(theme::TEXT_DIM).wrapping(iced::widget::text::Wrapping::None), scan,
        )).width(Length::Fill).clip(true);
        let bottom = row![recording, divider(), status].spacing(8).align_y(iced::Alignment::Center);
        container(column![container(top).height(30).center_y(30), container(bottom).height(30).center_y(30)])
            .height(TRANSPORT_HEIGHT).width(Length::Fill).padding([2, 6]).style(theme::header).into()
    }

}

#[derive(Debug, Clone, PartialEq)]
struct PatternChoice {
    id: PatternId,
    name: String,
}

impl std::fmt::Display for PatternChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

/// A running job's progress, with a button that cancels it.
fn job(label: &str, progress: f32, cancel: Message) -> Element<'_, Message> {
    row![
        panels::label(format!("{label} {:.0}%", progress * 100.0)),
        progress_bar(0.0..=1.0, progress).length(120).girth(8),
        panels::tool("cancel", cancel)
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center)
    .into()
}

/// The draggable area around a split line, centered on it.
pub(super) fn handle_rect(split: &Split) -> Rect {
    let line = split.line;
    match split.axis {
        Axis::Horizontal => Rect { x: line.x + (line.width - HANDLE) / 2.0, width: HANDLE, ..line },
        Axis::Vertical => Rect { y: line.y + (line.height - HANDLE) / 2.0, height: HANDLE, ..line },
    }
}

fn rule_style() -> rule::Style {
    rule::Style { color: theme::LINE, radius: 0.0.into(), fill_mode: rule::FillMode::Full, snap: true }
}

fn divider<'a>() -> Element<'a, Message> {
    container(rule::vertical(1).style(|_| rule_style())).height(18).width(1).into()
}
