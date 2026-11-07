//! Channels of the selected pattern with FL-style step buttons.

use std::path::PathBuf;

use daw_engine::song::channel_node;
use daw_model::{ChannelId, InsertId, STEP_TICKS, Source, SynthParams, Target};
use rune::{Color, Cursor, El, Tx, col, el, row};

use super::browser::{self, DropZone};
use super::label;
use crate::app::{App, Message as AppMessage};
use crate::menu;
use crate::task::Task;
use crate::theme::{self, C};
use crate::ui::{self, widgets::{self, Btn, Choice, Slider}};

const MAX_STEPS: u64 = 64;

#[derive(Debug, Clone)]
pub enum Message {
    /// Select a channel and show or hide its plugin window, if it has one.
    Select(ChannelId),
    Step(ChannelId, u64),
    Mute(ChannelId),
    /// Set a channel's color, 0xRRGGBB.
    Color(ChannelId, u32),
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
    app.midi_target();
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

pub fn update(app: &mut App, message: Message) -> Task {
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
        Message::Color(id, color) => {
            app.checkpoint();
            if let Some(channel) = app.project.channel_mut(id) {
                channel.color = Some(color);
            }
            app.edited();
        }
        Message::Volume(id, volume) => {
            if app.project.channel(id).is_none_or(|c| c.volume == volume) { return Task::none(); }
            app.begin_edit();
            if let Some(channel) = app.project.channel_mut(id) {
                channel.volume = volume;
            }
            app.session.set_mix(&app.project, Target::ChannelVolume(id), volume);
            app.mark_edited();
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
            return Task::open("WAV audio", &["wav", "wave"], None, |path| Message::SamplePicked(path).into());
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

pub fn toolbar(_app: &App) -> El {
    super::bar(vec![
        Btn::new("Synth").icon(&crate::ui::icons::PLUS).help("Add a built-in synth channel").build(Message::AddSynth),
        Btn::new("Sample").icon(&crate::ui::icons::PLUS).help("Load a WAV file into a sampler channel").build(Message::AddSampler),
        Btn::new("Delete").help("Delete the selected channel").build(Message::Remove),
    ])
}

/// One step pad: lit in the channel's color when on.
fn step_pad(on: bool, shade: bool, lit: Color, message: AppMessage) -> El {
    let fill = match (on, shade) {
        (true, _) => lit,
        (false, true) => theme::RAISED_HOVER,
        (false, false) => theme::RAISED,
    };
    let mut pad = el().size(16.0, 24.0).radius(4.0).fill(fill).cursor(Cursor::Pointer).on_press_tx(ui::on(message));
    pad = if on { pad.hover(Color::WHITE.alpha(0.12)) } else { pad.hover(C.hover).press(C.press) };
    pad
}

pub fn view(app: &App) -> El {
    let Some(pattern) = app.project.pattern(app.selected_pattern) else { return label("No pattern") };
    let steps = (pattern.length / STEP_TICKS).clamp(1, MAX_STEPS);
    let mut rows = col().gap(4.0).padding(10.0);
    if app.project.channels.is_empty() {
        return browser::zone(app, DropZone::Rack, widgets::empty_state(
            "No channels yet",
            &["Add a synth or a sample, or drag an instrument here from the browser.".to_string()],
            vec![
                Btn::new("Synth").icon(&crate::ui::icons::PLUS).build(Message::AddSynth),
                Btn::new("Sample").icon(&crate::ui::icons::PLUS).build(Message::AddSampler),
            ],
        ).w_full().h_full());
    }
    for channel in &app.project.channels {
        let id = channel.id;
        let selected = app.selected_channel == Some(id);
        let color = theme::rgb(app.project.channel_color(id));
        // Lit like an effect button while the plugin window is showing.
        let open = matches!(channel.source, Source::Plugin(instance) if app.session.editor_open(instance));
        let muted = channel.mute;
        let stripe = el().size(4.0, 24.0).radius(2.0).fill(if muted { theme::mix(theme::SURFACE, color, 0.35) } else { color });
        let mute = Btn::new("M").width(24.0).look(if muted { widgets::Look::On } else { widgets::Look::Ghost })
            .help(if muted { "Unmute channel" } else { "Mute channel" })
            .build(Message::Mute(id));
        let name = Btn::new(channel.name.clone()).width(110.0).start()
            .look(if open { widgets::Look::On } else if selected { widgets::Look::Selected } else { widgets::Look::Ghost })
            .help(format!("{}\nSelect channel and show its plugin; right-click to rename, preview, or delete", channel.name))
            .build(Message::Select(id))
            .on_secondary_press(move |app: &mut App, _, tx: &mut Tx| ui::send(app, tx, menu::Message::Open(menu::Item::Channel(id)).into()));
        let volume = Slider::new(channel.volume, (0.0, 1.0)).default(0.8).width(70.0).color(color)
            .help(format!("Channel volume: {:.0}%\nRight-click to automate", channel.volume * 100.0))
            .on_secondary(AppMessage::Automate(Target::ChannelVolume(id)))
            .build(move |v| Message::Volume(id, v).into());
        let choices = app.project.mixer.inserts.iter()
            .map(|i| Choice::new(app.project.insert_label(i.id), Message::Route(id, i.id), i.id == channel.insert))
            .collect();
        let route = widgets::select_with(&format!("route-{}", id.0), Some(app.project.insert_label(channel.insert)), "Insert", choices, 96.0, Some("Mixer insert for this channel".into()));
        let mut step_row = row().gap(3.0).center_y();
        // Audio channels play from playlist clips, not steps.
        let steps = if matches!(channel.source, Source::Audio { .. }) { 0 } else { steps };
        for step in 0..steps {
            if step > 0 && step % 4 == 0 {
                step_row = step_row.child(el().w(4.0));
            }
            let shade = (step / 4) % 2 == 1;
            step_row = step_row.child(step_pad(pattern.step_on(id, step), shade, color, Message::Step(id, step).into()));
        }
        if steps == 0 {
            step_row = step_row.child(label("Plays from audio clips in the playlist"));
        }
        let mut line = row().gap(8.0).center_y().h(32.0).pl(6.0).pr(8.0).radius(theme::RADIUS)
            .child(stripe).child(mute).child(name).child(step_row).child(volume).child(route);
        if selected { line = line.fill(C.well); }
        rows = rows.child(browser::zone(app, DropZone::Channel(id), line));
    }
    let rack = super::scroll(rows, true, true);
    browser::zone(app, DropZone::Rack, col().w_full().h_full().child(rack))
}
