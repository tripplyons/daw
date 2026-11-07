//! The DAW's controls, drawn with Rune primitives in the theme's tokens.
//!
//! Controls do not take keyboard focus when clicked, so Space and the other
//! bindings keep working after a click, as in most audio software.

use rune::{Align, Color, Cursor, Drag, El, Icon, Phase, Side, Tx, col, el, icon, row, text, text_input};

use super::{Msg, on, send};
use crate::app::{App, Message};
use crate::theme::{self, C};

/// How a button looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    /// A raised control: the default.
    Raised,
    /// No fill until hovered, for toolbars and lists.
    Ghost,
    /// The accent fill, for the main action of a dialog.
    Primary,
    /// Accent text on a soft accent fill: a toggle that is on.
    On,
    /// Red: recording is armed.
    Armed,
    /// Selected in a list.
    Selected,
}

/// A button. Build with `Btn::new(label)`, set what it looks like, then
/// `.build(message)`.
pub struct Btn {
    label: String,
    icon: Option<&'static Icon>,
    look: Look,
    help: Option<String>,
    width: Option<f32>,
    height: f32,
    key: Option<String>,
    disabled: bool,
    align_start: bool,
    trailing: Option<String>,
}

impl Btn {
    pub fn new(label: impl Into<String>) -> Self {
        Btn {
            label: label.into(),
            icon: None,
            look: Look::Raised,
            help: None,
            width: None,
            height: theme::CONTROL_HEIGHT,
            key: None,
            disabled: false,
            align_start: false,
            trailing: None,
        }
    }

    pub fn icon(mut self, icon: &'static Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn look(mut self, look: Look) -> Self {
        self.look = look;
        self
    }

    pub fn ghost(self) -> Self {
        self.look(Look::Ghost)
    }

    pub fn primary(self) -> Self {
        self.look(Look::Primary)
    }

    /// Show as on (accent) when `on`, else keep the current look.
    pub fn on(mut self, on: bool) -> Self {
        if on {
            self.look = Look::On;
        }
        self
    }

    pub fn armed(mut self, armed: bool) -> Self {
        if armed {
            self.look = Look::Armed;
        }
        self
    }

    pub fn help(mut self, help: impl Into<String>) -> Self {
        let help = help.into();
        if !help.is_empty() {
            self.help = Some(help);
        }
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }

    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Put the label at the start, for list rows and menus.
    pub fn start(mut self) -> Self {
        self.align_start = true;
        self
    }

    /// Dim text at the end, such as a shortcut.
    pub fn trailing(mut self, text: impl Into<String>) -> Self {
        let text = text.into();
        if !text.is_empty() {
            self.trailing = Some(text);
        }
        self
    }

    /// The button's box without a handler.
    pub fn view(self) -> El {
        let (fill, fg, border) = match self.look {
            Look::Raised => (C.raised, C.text, C.border),
            Look::Ghost => (Color::TRANSPARENT, C.text_dim, Color::TRANSPARENT),
            Look::Primary => (C.accent, C.on_accent, Color::TRANSPARENT),
            Look::On => (C.accent_soft, C.accent, C.accent.alpha(0.35)),
            Look::Armed => (C.record, Color::WHITE, Color::TRANSPARENT),
            Look::Selected => (C.raised_hover, C.text, Color::TRANSPARENT),
        };
        let fg = if self.disabled { C.text_faint } else { fg };
        // Narrow buttons such as M and S center their letter without padding.
        let narrow = self.label.is_empty() || self.width.is_some_and(|w| w < 36.0);
        let mut body = row().h(self.height).px(if narrow { 0.0 } else { 9.0 }).gap(6.0).center_y().radius(theme::RADIUS).fill(fill);
        if border != Color::TRANSPARENT {
            body = body.border(1.0, border);
        }
        if self.label.is_empty() {
            body = body.w(self.width.unwrap_or(self.height)).center();
        } else if let Some(width) = self.width {
            body = body.w(width);
            body = if self.align_start { body.justify_start() } else { body.justify_center() };
        }
        if let Some(i) = self.icon {
            body = body.child(icon(i, 14.0).color(fg));
        }
        if !self.label.is_empty() {
            let mut label = text(self.label).font(theme::LABEL).color(fg).single_line();
            if self.width.is_some() {
                label = label.truncate().shrink();
                if self.align_start || self.trailing.is_some() {
                    label = label.grow(1.0);
                }
            }
            body = body.child(label);
        }
        if let Some(trailing) = self.trailing {
            body = body.child(text(trailing).font(theme::SMALL_TEXT).color(C.text_faint).single_line());
        }
        if !self.disabled {
            body = body.hover(C.hover).press(C.press).cursor(Cursor::Pointer);
        }
        if let Some(help) = self.help {
            body = body.help(help);
        }
        if let Some(key) = self.key {
            body = body.key(key);
        }
        body.role("button")
    }

    pub fn build(self, message: impl Into<Message> + Clone + 'static) -> El {
        let disabled = self.disabled;
        let view = self.view();
        if disabled { view } else { view.on_press_tx(on(message)) }
    }
}

/// Options side by side, one of them chosen.
pub fn segmented(items: Vec<(String, bool, Message, String)>) -> El {
    let mut group = row().h(theme::CONTROL_HEIGHT).p(2.0).gap(2.0).radius(theme::RADIUS + 1.0).fill(C.well).border(1.0, C.border).center_y();
    for (label, chosen, message, help) in items {
        let mut item = row().h(theme::CONTROL_HEIGHT - 6.0).px(8.0).center().radius(theme::RADIUS - 1.0)
            .child(text(label).font(theme::LABEL).color(if chosen { C.text } else { C.text_dim }).single_line())
            .cursor(Cursor::Pointer)
            .hover(C.hover)
            .on_press_tx(on(message));
        if chosen {
            item = item.fill(C.raised_hover).shadow(1.0, 2.0, C.shadow);
        }
        if !help.is_empty() {
            item = item.help(help);
        }
        group = group.child(item);
    }
    group
}

/// A text label in the secondary color.
pub fn label(content: impl Into<String>) -> El {
    text(content.into()).font(theme::SMALL_TEXT).color(C.text_dim).single_line()
}

/// A short section name over a group of controls.
pub fn section(content: impl Into<String>) -> El {
    text(content.into()).font(theme::SECTION).color(C.text_faint).single_line()
}

/// A name over its control, kept together when a toolbar wraps.
pub fn labeled(name: &str, control: El) -> El {
    row().gap(6.0).center_y().child(label(name)).child(control)
}

/// A thin vertical line between toolbar groups.
pub fn divider() -> El {
    el().w(1.0).h(18.0).fill(C.border)
}

/// A horizontal hairline.
pub fn rule() -> El {
    el().h(1.0).w_full().fill(C.border)
}

/// A centered hint for an empty panel: a title, a few lines on how to
/// start, and buttons for the first steps.
pub fn empty_state(title: &str, lines: &[String], actions: Vec<El>) -> El {
    let mut body = col().gap(6.0).items_center().max_w(360.0)
        .child(text(title.to_string()).font(theme::TITLE).color(C.text));
    for line in lines {
        body = body.child(text(line.clone()).font(theme::BODY).color(C.text_dim).align(Align::Center).wrap());
    }
    if !actions.is_empty() {
        body = body.child(row().gap(6.0).mt(6.0).children(actions));
    }
    col().w_full().h_full().center().padding(16.0).child(body)
}

/// One choice in a dropdown list.
#[derive(Debug, Clone)]
pub struct Choice {
    pub label: String,
    pub detail: Option<String>,
    pub message: Message,
    pub chosen: bool,
}

impl Choice {
    pub fn new(label: impl Into<String>, message: impl Into<Message>, chosen: bool) -> Self {
        Choice { label: label.into(), detail: None, message: message.into(), chosen }
    }
}

/// An open dropdown list, anchored to the control that opened it.
#[derive(Debug, Clone)]
pub struct Dropdown {
    pub key: String,
    pub choices: Vec<Choice>,
    pub width: f32,
}

/// A control that shows the current choice and opens a list of choices.
pub fn select(key: &str, current: Option<String>, placeholder: &str, choices: Vec<Choice>, width: f32) -> El {
    select_with(key, current, placeholder, choices, width, None)
}

pub fn select_with(key: &str, current: Option<String>, placeholder: &str, choices: Vec<Choice>, width: f32, help: Option<String>) -> El {
    let empty = current.is_none();
    let shown = current.unwrap_or_else(|| placeholder.to_string());
    let open = Dropdown { key: format!("select:{key}"), choices, width: width.max(120.0) };
    let mut trigger = row().key(open.key.clone()).w(width).h(theme::CONTROL_HEIGHT).pl(9.0).pr(4.0).gap(4.0).center_y()
        .radius(theme::RADIUS).fill(C.raised).border(1.0, C.border)
        .child(text(shown).font(theme::LABEL).color(if empty { C.text_faint } else { C.text }).single_line().truncate().grow(1.0).shrink())
        .child(icon(&super::icons::CHEVRON, 12.0).color(C.text_faint))
        .hover(C.hover)
        .press(C.press)
        .cursor(Cursor::Pointer)
        .role("combobox")
        .on_press_tx(on(Message::Ui(Msg::OpenDropdown(open))));
    if let Some(help) = help {
        trigger = trigger.help(help);
    }
    trigger
}

/// The open dropdown, over everything, below its control.
pub fn dropdown_layer(app: &App) -> Option<El> {
    let open = app.ui.dropdown.as_ref()?;
    let mut list = col().p(4.0).gap(1.0);
    for choice in &open.choices {
        let message = choice.message.clone();
        let mut item = row().h(26.0).px(8.0).gap(8.0).center_y().radius(theme::RADIUS - 2.0)
            .child(el().w(14.0).child_if(choice.chosen, || icon(&super::icons::CHECK, 12.0).color(C.accent)))
            .child(text(choice.label.clone()).font(theme::BODY).color(C.text).single_line().truncate().grow(1.0).shrink())
            .hover(C.hover)
            .press(C.press)
            .cursor(Cursor::Pointer)
            .on_press_tx(move |app: &mut App, tx: &mut Tx| {
                app.ui.dropdown = None;
                send(app, tx, message.clone());
            });
        if let Some(detail) = &choice.detail {
            item = item.child(text(detail.clone()).font(theme::SMALL_TEXT).color(C.text_faint).single_line());
        }
        list = list.child(item);
    }
    if open.choices.is_empty() {
        list = list.child(row().h(26.0).px(8.0).center_y().child(label("Nothing to choose")));
    }
    let height = (open.choices.len().max(1) as f32 * 27.0 + 8.0).min(app.window.height * 0.6);
    let panel = popover(col().w(open.width).max_h(height).child(list.scroll(rune::ScrollSpec { stick_bottom: false })))
        .anchored(open.key.clone(), Side::Bottom, Align::Start, 4.0);
    // A click outside closes the list without choosing.
    let scrim = el().absolute(0.0, 0.0).w_full().h_full().on_press_tx(on(Message::Ui(Msg::CloseDropdown)));
    Some(el().absolute(0.0, 0.0).w_full().h_full().child(scrim).child(panel))
}

/// A floating surface for menus, lists, and tooltips.
pub fn popover(content: El) -> El {
    content.fill(C.popover).radius(theme::RADIUS_LG - 2.0).border(1.0, C.border).shadow(8.0, 24.0, C.shadow).clip_rounded(theme::RADIUS_LG - 2.0)
}

/// A one-line text field bound to an app value. `on_change` turns the
/// typed text into a message; Enter sends `on_submit` when given.
pub fn field(app: &App, key: &'static str, value: &str, placeholder: &str, on_change: fn(String) -> Message, on_submit: Option<Message>) -> El {
    let edit = {
        let mut fields = app.ui.fields.borrow_mut();
        let edit = fields.entry(key).or_insert_with(|| rune::TextEdit::new(value));
        if edit.text != value {
            edit.replace_text(value);
        }
        edit.clone()
    };
    let focused = edit.focused;
    let mut input = text_input(&edit, theme::BODY)
        .key(key)
        .single_line()
        .placeholder(placeholder, C.text_faint)
        .caret(C.accent, C.accent.alpha(0.3))
        .color(C.text)
        .on_edit(move |app: &mut App| app.ui.field(key))
        .on_input_change(move |app: &mut App, tx: &mut Tx| {
            let text = app.ui.field(key).text.clone();
            send(app, tx, on_change(text));
        })
        .grow(1.0);
    if let Some(submit) = on_submit {
        input = input.on_submit(move |app: &mut App, tx: &mut Tx| send(app, tx, submit.clone()));
    }
    row().h(theme::CONTROL_HEIGHT).px(8.0).center_y().radius(theme::RADIUS)
        .fill(C.well)
        .border(1.0, if focused { C.accent.alpha(0.6) } else { C.border })
        .child(input)
}

/// A horizontal slider. `on_change` gets the new value while dragging;
/// releasing ends the edit, and a double click resets to `default`.
pub struct Slider {
    pub value: f32,
    pub range: (f32, f32),
    pub default: Option<f32>,
    /// Fill from the middle, for pan.
    pub bipolar: bool,
    pub width: Option<f32>,
    pub color: Color,
    pub help: Option<String>,
    pub on_secondary: Option<Message>,
}

impl Slider {
    pub fn new(value: f32, range: (f32, f32)) -> Self {
        Slider { value, range, default: None, bipolar: false, width: None, color: C.text_dim, help: None, on_secondary: None }
    }

    pub fn default(mut self, value: f32) -> Self {
        self.default = Some(value);
        self
    }

    pub fn bipolar(mut self) -> Self {
        self.bipolar = true;
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    pub fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn on_secondary(mut self, message: Message) -> Self {
        self.on_secondary = Some(message);
        self
    }

    pub fn build(self, on_change: impl Fn(f32) -> Message + 'static) -> El {
        let (lo, hi) = self.range;
        let t = ((self.value - lo) / (hi - lo)).clamp(0.0, 1.0);
        let width = self.width.unwrap_or(120.0);
        let inner = width - 10.0;
        let (from, to) = if self.bipolar { (t.min(0.5), t.max(0.5)) } else { (0.0, t) };
        let track = el().absolute(5.0, 7.0).size(inner, 4.0).radius(2.0).fill(C.well)
            .child(el().absolute(inner * from, 0.0).size((inner * (to - from)).max(0.0), 4.0).radius(2.0).fill(self.color));
        let knob = el().absolute(inner * t, 3.0).size(12.0, 12.0).radius(6.0).fill(C.text).shadow(1.0, 3.0, C.shadow);
        let change = std::rc::Rc::new(on_change);
        let drag_change = change.clone();
        let mut slider = el().w(width).h(18.0).cursor(Cursor::Pointer).child(track).child(knob)
            .on_drag(move |app: &mut App, d: &mut Drag, tx: &mut Tx| {
                let value = lo + ((d.x - 5.0) / inner).clamp(0.0, 1.0) * (hi - lo);
                if d.phase != Phase::Up {
                    send(app, tx, drag_change(value));
                } else {
                    send(app, tx, Message::EndEdit);
                }
            });
        if let Some(default) = self.default {
            slider = slider.on_double_press_tx(move |app: &mut App, tx: &mut Tx| {
                send(app, tx, change(default));
                send(app, tx, Message::EndEdit);
            });
        }
        if let Some(message) = self.on_secondary {
            slider = slider.on_secondary_press(move |app: &mut App, _, tx: &mut Tx| send(app, tx, message.clone()));
        }
        if let Some(help) = self.help {
            slider = slider.help(help);
        }
        slider.role("slider")
    }
}

/// A vertical fader for mixer strips.
pub fn fader(value: f32, range: (f32, f32), default: f32, height: f32, on_change: impl Fn(f32) -> Message + 'static) -> El {
    let (lo, hi) = range;
    let t = ((value - lo) / (hi - lo)).clamp(0.0, 1.0);
    let travel = (height - 14.0).max(1.0);
    let y = travel * (1.0 - t);
    let unity = travel * (1.0 - ((default - lo) / (hi - lo)).clamp(0.0, 1.0));
    let change = std::rc::Rc::new(on_change);
    let drag_change = change.clone();
    el().w(22.0).h(height).cursor(Cursor::ResizeRow)
        .child(el().absolute(9.5, 7.0).size(3.0, travel).radius(1.5).fill(theme::BG))
        .child(el().absolute(9.5, 7.0 + y).size(3.0, (travel - y).max(0.0)).radius(1.5).fill(C.text_faint))
        .child(el().absolute(4.0, 7.0 + unity).size(14.0, 1.0).fill(C.text_faint.alpha(0.6)))
        .child(el().absolute(1.0, y).size(20.0, 14.0).radius(4.0).fill(C.text).shadow(1.0, 4.0, C.shadow)
            .child(el().absolute(4.0, 6.5).size(12.0, 1.0).fill(C.raised)))
        .on_drag(move |app: &mut App, d: &mut Drag, tx: &mut Tx| {
            if d.phase == Phase::Up {
                send(app, tx, Message::EndEdit);
                return;
            }
            let value = lo + (1.0 - ((d.y - 7.0) / travel).clamp(0.0, 1.0)) * (hi - lo);
            send(app, tx, drag_change(value));
        })
        .on_double_press_tx(move |app: &mut App, tx: &mut Tx| {
            send(app, tx, change(default));
            send(app, tx, Message::EndEdit);
        })
        .role("slider")
}

/// A peak meter from silence to +6 dB, lit from the bottom.
pub fn meter(peak: f32, height: f32) -> El {
    let db = if peak > 0.0 { 20.0 * peak.log10() } else { -120.0 };
    let fraction = ((db + 60.0) / 66.0).clamp(0.0, 1.0);
    let lit = height * fraction;
    let color = if db > 0.0 { C.record } else if db > -6.0 { C.accent } else { C.success };
    el().w(4.0).h(height).radius(2.0).fill(theme::BG)
        .child(el().absolute(0.0, height - lit).size(4.0, lit).radius(2.0).fill(color))
}

/// A progress bar from 0 to 1.
pub fn progress(value: f32, width: f32) -> El {
    el().w(width).h(6.0).radius(3.0).fill(C.well)
        .child(el().absolute(0.0, 0.0).size(width * value.clamp(0.0, 1.0), 6.0).radius(3.0).fill(C.accent))
}

pub trait ElExt {
    /// Add a child only when `show` is true.
    fn child_if(self, show: bool, child: impl FnOnce() -> El) -> El;
    /// Padding on all sides.
    fn p(self, p: f32) -> El;
}

impl ElExt for El {
    fn child_if(self, show: bool, child: impl FnOnce() -> El) -> El {
        if show { self.child(child()) } else { self }
    }

    fn p(self, p: f32) -> El {
        self.padding(p)
    }
}
