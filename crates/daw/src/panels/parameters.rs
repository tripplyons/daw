//! Generic parameter list for the selected plugin, or the built-in synth of
//! the selected channel. Right click any row to automate it.

use daw_model::{ChannelId, InstanceId, Source, SynthParams, Target, Waveform};
use rune::{El, col, row, text};

use super::label;
use crate::app::{App, Message as AppMessage};
use crate::theme::{self, C};
use crate::ui::widgets::{self, Slider};

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
            if !(0.0..=1.0).contains(&value)
                || app.project.plugin(instance).is_none() || app.session.param_value(instance, param) == value { return; }
            app.begin_edit();
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

pub fn toolbar(app: &App) -> El {
    let Some(instance) = app.param_instance else { return label("") };
    let name = app.project.plugin(instance).map(|p| p.plugin.name.clone()).unwrap_or_default();
    let mut items = vec![text(name).font(theme::LABEL).color(C.text).single_line()];
    if app.session.has_editor(instance) {
        let open = app.session.editor_open(instance);
        items.push(super::toggle(if open { "Hide window" } else { "Show window" }, open, AppMessage::TogglePlugin(instance)));
    }
    super::bar(items)
}

/// A name, a slider, and the value as the plugin shows it.
fn param_row(name: String, value: f32, shown: String, steps: u32, on_change: impl Fn(f32) -> AppMessage + 'static, target: Option<Target>) -> El {
    // Stepped parameters snap to their steps.
    let on_change = move |v: f32| {
        let v = if steps > 0 { (v * steps as f32).round() / steps as f32 } else { v };
        on_change(v)
    };
    let mut slider = Slider::new(value, (0.0, 1.0)).width(160.0).color(C.accent);
    if let Some(target) = target {
        slider = slider.on_secondary(AppMessage::Automate(target)).help(format!("{name}: {shown}\nRight-click to automate"));
    }
    row().w_full().h(26.0).px(12.0).gap(10.0).center_y().radius(theme::RADIUS).hover(C.hover)
        .child(text(name.clone()).font(theme::BODY).color(C.text_dim).single_line().truncate().grow(1.0).shrink().min_w(0.0))
        .child(slider.build(on_change))
        .child(text(shown).font(theme::NUMBER).color(C.text).single_line().truncate().w(72.0))
}

fn synth_view(app: &App, id: ChannelId, params: SynthParams) -> El {
    let waveforms = [(Waveform::Sine, "Sine"), (Waveform::Saw, "Saw"), (Waveform::Square, "Square")];
    let shape = widgets::segmented(waveforms.iter().map(|&(w, name)| {
        (name.to_string(), w == params.waveform, Message::Synth(id, SynthParams { waveform: w, ..params }).into(), String::new())
    }).collect());
    let cutoff = Target::SynthCutoff(id);
    let name = app.project.channel(id).map(|c| c.name.clone()).unwrap_or_default();
    col().w_full().gap(2.0).py(10.0)
        .child(row().px(12.0).pb(6.0).child(text(format!("{name} · built-in synth")).font(theme::TITLE).color(C.text).single_line()))
        .child(row().w_full().h(30.0).px(12.0).gap(10.0).center_y()
            .child(text("Waveform").font(theme::BODY).color(C.text_dim).grow(1.0))
            .child(shape))
        .child(param_row("Cutoff".into(), params.cutoff, app.value_text(cutoff, params.cutoff), 0,
            move |v| Message::Synth(id, SynthParams { cutoff: v, ..params }).into(), Some(cutoff)))
        .child(param_row("Attack".into(), params.attack / 2.0, format!("{:.0} ms", params.attack * 1000.0), 0,
            move |v| Message::Synth(id, SynthParams { attack: v * 2.0, ..params }).into(), None))
        .child(param_row("Release".into(), params.release / 4.0, format!("{:.0} ms", params.release * 1000.0), 0,
            move |v| Message::Synth(id, SynthParams { release: v * 4.0, ..params }).into(), None))
}

pub fn view(app: &App) -> El {
    let Some(instance) = app.param_instance else {
        let synth = app.selected_channel.and_then(|id| match app.project.channel(id)?.source {
            Source::Synth(params) => Some((id, params)),
            _ => None,
        });
        return match synth {
            Some((id, params)) => synth_view(app, id, params),
            None => widgets::empty_state("Nothing selected", &["Select a channel or a plugin to see its parameters.".to_string()], Vec::new()),
        };
    };
    if let Some(error) = app.session.load_errors.get(&instance) {
        return widgets::empty_state("The plugin failed to load", &[error.to_string()], Vec::new());
    }
    let search = row().w_full().px(10.0).pt(10.0).pb(6.0)
        .child(widgets::field(app, "params-search", &app.params.search, "Filter parameters", |s| Message::Search(s).into(), None).w_full());
    let needle = app.params.search.to_lowercase();
    let params = app.session.params(instance);
    let matching: Vec<_> = params.iter().filter(|p| needle.is_empty() || p.name.to_lowercase().contains(&needle)).collect();
    let mut list = col().w_full().gap(1.0).px(4.0).pb(8.0);
    for param in matching.iter().take(ROW_LIMIT) {
        let id = param.id;
        let value = app.session.param_value(instance, id);
        list = list.child(param_row(
            param.name.clone(),
            value,
            app.session.param_text(instance, id, value),
            param.steps,
            move |v| Message::Set(instance, id, v).into(),
            Some(Target::Plugin { instance, param: id }),
        ));
    }
    if matching.len() > ROW_LIMIT {
        list = list.child(row().padding(10.0).child(label(format!("{} more. Type in the filter to narrow the list.", matching.len() - ROW_LIMIT))));
    }
    if matching.is_empty() && !params.is_empty() {
        list = list.child(row().padding(12.0).child(label("No parameters match the filter.")));
    }
    col().w_full().h_full().child(search).child(super::scroll(list, true, false))
}
