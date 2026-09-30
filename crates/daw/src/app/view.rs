//! The window: transport, background jobs, and the tiled panels.

use daw_engine::song::PlayMode;
use daw_model::layout::{Axis, Node, Panel, TileId};
use daw_model::PatternId;
use daw_plugins::scan::Progress;
use iced::widget::{button, column, container, mouse_area, pick_list, progress_bar, row, rule, stack, text, text_input};
use iced::{Element, Length, mouse};

use super::{App, JOBS_HEIGHT, Message, TRANSPORT_HEIGHT};
use crate::keys::Action;
use crate::menu;
use crate::panels::{self, automation, browser, channel_rack, mixer, parameters, piano_roll, playlist, settings};
use crate::theme;

const GUTTER: f32 = 3.0;

impl App {
    pub fn view(&self) -> Element<'_, Message> {
        let layout = self.layout();
        let tiles = if layout.zoomed { self.tile(layout.focused) } else { self.node(&layout.root, &mut Vec::new()) };
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

    fn node<'a>(&'a self, node: &Node, path: &mut Vec<bool>) -> Element<'a, Message> {
        match node {
            Node::Leaf(id) => self.tile(*id),
            Node::Split { axis, ratio, first, second } => {
                let a = ((ratio * 1000.0).round() as u16).max(1);
                let b = (1000u16.saturating_sub(a)).max(1);
                path.push(false);
                let first = self.node(first, path);
                path.pop();
                path.push(true);
                let second = self.node(second, path);
                path.pop();
                let handle = path.clone();
                match axis {
                    Axis::Horizontal => {
                        let gutter = mouse_area(
                            container(rule::vertical(1).style(|_| rule_style()))
                                .width(GUTTER)
                                .height(Length::Fill)
                                .center_x(GUTTER),
                        )
                        .on_press(Message::SplitDrag(handle))
                        .interaction(mouse::Interaction::ResizingHorizontally);
                        row![
                            container(first).width(Length::FillPortion(a)),
                            gutter,
                            container(second).width(Length::FillPortion(b))
                        ]
                        .height(Length::Fill)
                        .into()
                    }
                    Axis::Vertical => {
                        let gutter = mouse_area(
                            container(rule::horizontal(1).style(|_| rule_style()))
                                .height(GUTTER)
                                .width(Length::Fill)
                                .center_y(GUTTER),
                        )
                        .on_press(Message::SplitDrag(handle))
                        .interaction(mouse::Interaction::ResizingVertically);
                        column![
                            container(first).height(Length::FillPortion(a)),
                            gutter,
                            container(second).height(Length::FillPortion(b))
                        ]
                        .width(Length::Fill)
                        .into()
                    }
                }
            }
        }
    }

    fn tile(&self, id: TileId) -> Element<'_, Message> {
        let layout = self.layout();
        let panel = layout.panel(id);
        let focused = layout.focused == id;
        let name_color = if focused { theme::BRIGHT } else { theme::TEXT_DIM };
        let picker = pick_list(Panel::ALL, Some(panel), move |p| Message::SetPanel(id, p))
            .text_size(theme::SMALL)
            .padding([2, 6])
            .style(move |theme, status| {
                let mut style = theme::pick(theme, status);
                style.text_color = name_color;
                style.background = iced::Background::Color(iced::Color::TRANSPARENT);
                style
            })
            .menu_style(theme::menu);
        let tools = match panel {
            Panel::Browser => browser::toolbar(self),
            Panel::ChannelRack => channel_rack::toolbar(self),
            Panel::PianoRoll => piano_roll::toolbar(self),
            Panel::Playlist => playlist::toolbar(self),
            Panel::Mixer => mixer::toolbar(self),
            Panel::Automation => automation::toolbar(self),
            Panel::Parameters => parameters::toolbar(self),
            Panel::Settings => settings::toolbar(self),
        };
        let tools = if panel == Panel::Playlist {
            crate::panels::scroll(tools, false, true).width(Length::Fill).height(theme::HEADER_HEIGHT).into()
        } else { tools };
        let marker = if layout.zoomed { text("focus").size(theme::SMALL).color(theme::TEXT_DIM) } else { text("") };
        let header = container(row![picker, tools, marker].spacing(6).align_y(iced::Alignment::Center))
            .height(theme::HEADER_HEIGHT)
            .width(Length::Fill)
            .padding([0, 4])
            .align_y(iced::Alignment::Center)
            .style(move |t| {
                let mut style = theme::header(t);
                if focused {
                    style.background = Some(iced::Background::Color(theme::CONTROL));
                }
                style
            });
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
        container(column![header, container(body).width(Length::Fill).height(Length::Fill)])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::panel)
            .into()
    }

    fn transport(&self) -> Element<'_, Message> {
        let small = |label: &str| text(label.to_string()).size(theme::SMALL);
        let playing = self.playing;
        let mode_label = match self.mode {
            PlayMode::Song => "song",
            PlayMode::Pattern(_) => "pattern",
        };
        let beats = self.position / f64::from(daw_model::time::TICKS_PER_BEAT);
        let beats_per_bar = f64::from(self.project.signature.numerator);
        let position = format!(
            "{}.{}.{:02}",
            (beats / beats_per_bar).floor() as u32 + 1,
            (beats % beats_per_bar).floor() as u32 + 1,
            ((beats.fract()) * 100.0).floor() as u32
        );
        let pattern_names: Vec<PatternChoice> =
            self.project.patterns.iter().map(|p| PatternChoice { id: p.id, name: p.name.clone() }).collect();
        let selected = pattern_names.iter().find(|p| p.id == self.selected_pattern).cloned();
        let bar = self.project.signature.ticks_per_bar();
        let bars = self.project.pattern(self.selected_pattern).map(|p| p.length.div_ceil(bar).max(1));
        let mut bar_choices: Vec<u64> = (1..=16).collect();
        if let Some(bars) = bars
            && !bar_choices.contains(&bars)
        {
            bar_choices.push(bars);
        }
        let scan = match self.scan {
            Some(Progress { done, total }) if total > 0 => format!("scanning plugins {done}/{total}"),
            Some(_) => "scanning plugins".into(),
            None => String::new(),
        };
        let bar = row![
            button(small(if playing { "stop" } else { "play" }))
                .on_press(Message::Action(Action::PlayPause))
                .style(theme::toggle(playing))
                .padding([3, 8]),
            button(small(mode_label)).on_press(Message::Action(Action::ToggleMode)).style(theme::control).padding([3, 8]),
            text(position).size(theme::TEXT_SIZE).font(iced::Font::MONOSPACE).width(70),
            text_input("bpm", &self.bpm_text.clone().unwrap_or_else(|| format!("{}", self.project.bpm)))
                .id("tempo")
                .on_submit(Message::BpmDone)
                .on_input(Message::SetBpm)
                .size(theme::SMALL)
                .width(48)
                .padding([3, 4])
                .style(theme::input),
            small("bpm").color(theme::TEXT_DIM),
            mouse_area(
                pick_list(pattern_names, selected, |p: PatternChoice| Message::SelectPattern(p.id))
                    .text_size(theme::SMALL)
                    .padding([3, 6])
                    .style(theme::pick)
                    .menu_style(theme::menu)
            )
            .on_right_press(menu::Message::Open(menu::Item::Pattern(self.selected_pattern)).into()),
            pick_list(bar_choices, bars, Message::PatternBars)
                .text_size(theme::SMALL)
                .padding([3, 6])
                .style(theme::pick)
                .menu_style(theme::menu),
            small("bars").color(theme::TEXT_DIM),
            button(small("+ pattern")).on_press(Message::NewPattern).style(theme::control).padding([3, 8]),
            button(small("bind")).on_press(Message::Action(Action::ToggleBind)).style(theme::toggle(self.bind_mode)).padding([3, 8]),
            button(small("rec")).on_press(Message::Action(Action::ToggleRecord)).style(theme::toggle(self.record)).padding([3, 8]),
            button(small("rec midi")).on_press(Message::Action(Action::ToggleMidiRecord))
                .style(theme::toggle(self.midi.recording)).padding([3, 8]),
            button(small("rec audio"))
                .on_press(Message::Action(Action::ToggleAudioRecord))
                .style(theme::toggle(self.session.recording_armed()))
                .padding([3, 8]),
            text(self.status.clone()).size(theme::SMALL).color(theme::TEXT_DIM).width(Length::Fill),
            small(&scan).color(theme::TEXT_DIM),
            button(small("export")).on_press(Message::Action(Action::Export)).style(theme::control).padding([3, 8]),
            button(small("open")).on_press(Message::Action(Action::Open)).style(theme::control).padding([3, 8]),
            button(small("save")).on_press(Message::Action(Action::Save)).style(theme::control).padding([3, 8]),
            button(small("settings")).on_press(Message::Action(Action::Settings)).style(theme::control).padding([3, 8]),
        ]
        .spacing(4)
        .align_y(iced::Alignment::Center);
        container(bar).height(TRANSPORT_HEIGHT).width(Length::Fill).padding([0, 4]).center_y(TRANSPORT_HEIGHT).style(theme::header).into()
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

fn rule_style() -> rule::Style {
    rule::Style { color: theme::LINE, radius: 0.0.into(), fill_mode: rule::FillMode::Full, snap: true }
}
