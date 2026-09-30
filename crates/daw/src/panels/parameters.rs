//! Generic parameter list for the selected plugin, or the built-in synth of
//! the selected channel. Right click any row to automate it.

use daw_model::{ChannelId, InstanceId, Source, SynthParams, Target, Waveform};
use iced::widget::{column, container, mouse_area, row, slider, text, text_input};
use iced::{Element, Length};

use super::{label, pick};
use crate::app::{App, Message as AppMessage};
use crate::theme;

/// Rows shown at once; large plugins expose thousands of parameters.
const ROW_LIMIT: usize = 300;

#[derive(Debug, Default)]
pub struct State {
    pub search: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    Search(String),
    Set(InstanceId, u32, f32),
    Synth(ChannelId, SynthParams),
}

impl From<Message> for AppMessage {
    fn from(message: Message) -> Self {
        AppMessage::Params(message)
    }
}

pub fn update(app: &mut App, message: Message) {
    match message {
        Message::Search(search) => app.params.search = search,
        Message::Set(instance, param, value) => {
            app.session.set_plugin_param(instance, param, value);
            app.touched(Target::Plugin { instance, param }, value);
        }
        Message::Synth(id, params) => {
            if app.project.channel(id).is_none_or(|c| c.source == Source::Synth(params)) { return; }
            app.begin_edit();
            let mut cutoff = None;
            if let Some(channel) = app.project.channel_mut(id)
                && let Source::Synth(current) = &mut channel.source
            {
                if current.cutoff != params.cutoff {
                    cutoff = Some(params.cutoff);
                }
                *current = params;
            }
            app.session.set_synth(id, params);
            app.mark_edited();
            if let Some(cutoff) = cutoff {
                app.touched(Target::SynthCutoff(id), cutoff);
            }
        }
    }
}

pub fn toolbar(app: &App) -> Element<'_, AppMessage> {
    let Some(instance) = app.param_instance else { return label("") };
    let name = app.project.plugin(instance).map(|p| p.plugin.name.clone()).unwrap_or_default();
    let mut bar = row![text(name).size(theme::SMALL).color(theme::TEXT)].spacing(4).align_y(iced::Alignment::Center);
    if app.session.has_editor(instance) {
        let open = app.session.editor_open(instance);
        bar = bar.push(super::toggle("window", open, AppMessage::TogglePlugin(instance)));
    }
    bar.into()
}

fn param_row<'a>(name: String, value: f32, shown: String, steps: u32, on_change: impl Fn(f32) -> AppMessage + 'a, target: Option<Target>) -> Element<'a, AppMessage> {
    let step = if steps > 0 { 1.0 / steps as f32 } else { 0.001 };
    let area = mouse_area(
        row![
            text(name).size(theme::SMALL).width(Length::FillPortion(3)),
            slider(0.0..=1.0, value, on_change)
                .step(step)
                .on_release(AppMessage::EndEdit)
                .height(14)
                .width(Length::FillPortion(3))
                .style(theme::fader),
            text(shown).size(theme::SMALL).color(theme::TEXT_DIM).width(Length::FillPortion(2)),
        ]
        .spacing(8)
        .padding([1, 6])
        .align_y(iced::Alignment::Center),
    );
    match target {
        Some(target) => area.on_right_press(AppMessage::Automate(target)).into(),
        None => area.into(),
    }
}

fn synth_view(app: &App, id: ChannelId, params: SynthParams) -> Element<'_, AppMessage> {
    let waveforms = [Waveform::Sine, Waveform::Saw, Waveform::Square];
    let waveform_names: Vec<String> = waveforms.iter().map(|w| format!("{w:?}").to_lowercase()).collect();
    let current = waveforms.iter().position(|&w| w == params.waveform).map(|i| waveform_names[i].clone());
    let names = waveform_names.clone();
    let wave = row![
        text("waveform").size(theme::SMALL).width(Length::FillPortion(3)),
        container(pick(waveform_names, current, move |name: String| {
            let index = names.iter().position(|n| *n == name).unwrap_or(0);
            Message::Synth(id, SynthParams { waveform: waveforms[index], ..params }).into()
        }))
        .width(Length::FillPortion(5)),
    ]
    .spacing(8)
    .padding([1, 6]);
    let cutoff = Target::SynthCutoff(id);
    column![
        wave,
        param_row(
            "cutoff".into(),
            params.cutoff,
            app.value_text(cutoff, params.cutoff),
            0,
            move |v| Message::Synth(id, SynthParams { cutoff: v, ..params }).into(),
            Some(cutoff),
        ),
        param_row(
            "attack".into(),
            params.attack / 2.0,
            format!("{:.0} ms", params.attack * 1000.0),
            0,
            move |v| Message::Synth(id, SynthParams { attack: v * 2.0, ..params }).into(),
            None,
        ),
        param_row(
            "release".into(),
            params.release / 4.0,
            format!("{:.0} ms", params.release * 1000.0),
            0,
            move |v| Message::Synth(id, SynthParams { release: v * 4.0, ..params }).into(),
            None,
        ),
    ]
    .spacing(2)
    .padding([4, 0])
    .into()
}

pub fn view(app: &App) -> Element<'_, AppMessage> {
    let Some(instance) = app.param_instance else {
        let synth = app.selected_channel.and_then(|id| match app.project.channel(id)?.source {
            Source::Synth(params) => Some((id, params)),
            _ => None,
        });
        return match synth {
            Some((id, params)) => synth_view(app, id, params),
            None => container(label("select a channel or plugin")).padding(8).into(),
        };
    };
    if let Some(error) = app.session.load_errors.get(&instance) {
        return container(label(format!("failed to load: {error}"))).padding(8).into();
    }
    let search = text_input("filter parameters", &app.params.search)
        .on_input(|s| Message::Search(s).into())
        .size(theme::SMALL)
        .padding([3, 6])
        .style(theme::input);
    let needle = app.params.search.to_lowercase();
    let params = app.session.params(instance);
    let matching: Vec<_> = params.iter().filter(|p| needle.is_empty() || p.name.to_lowercase().contains(&needle)).collect();
    let mut list = column![].spacing(1);
    for param in matching.iter().take(ROW_LIMIT) {
        let id = param.id;
        let value = app.session.param_value(instance, id);
        let target = Target::Plugin { instance, param: id };
        list = list.push(param_row(
            param.name.clone(),
            value,
            app.session.param_text(instance, id, value),
            param.steps,
            move |v| Message::Set(instance, id, v).into(),
            Some(target),
        ));
    }
    if matching.len() > ROW_LIMIT {
        list = list.push(container(label(format!("{} more; filter to narrow", matching.len() - ROW_LIMIT))).padding(6));
    }
    column![container(search).padding(4), super::scroll(list, true, false).height(Length::Fill)].into()
}
