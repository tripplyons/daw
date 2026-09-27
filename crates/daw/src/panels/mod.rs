//! Panels shown in tiles. Each owns its view state, messages, and edit handling.

pub mod automation;
pub mod browser;
pub mod channel_rack;
pub mod mixer;
pub mod parameters;
pub mod piano_roll;
pub mod playlist;
pub mod settings;
pub mod timeline;

use iced::widget::{button, pick_list, scrollable, text};
use iced::widget::scrollable::{Direction, Scrollbar};
use iced::{Element, mouse};

use crate::app::Message;
use crate::theme;

/// Small flat button used in toolbars.
pub fn tool<'a>(label: &str, message: Message) -> Element<'a, Message> {
    button(text(label.to_string()).size(theme::SMALL)).on_press(message).style(theme::control).padding([2, 6]).into()
}

/// Small toggle button used in toolbars.
pub fn toggle<'a>(label: &str, on: bool, message: Message) -> Element<'a, Message> {
    button(text(label.to_string()).size(theme::SMALL)).on_press(message).style(theme::toggle(on)).padding([2, 6]).into()
}

pub fn label<'a>(label: impl ToString) -> Element<'a, Message> {
    text(label.to_string()).size(theme::SMALL).color(theme::TEXT_DIM).into()
}

pub fn pick<'a, T>(options: Vec<T>, selected: Option<T>, on: impl Fn(T) -> Message + 'a) -> Element<'a, Message>
where
    T: ToString + PartialEq + Clone + 'a,
{
    pick_list(options, selected, on)
        .text_size(theme::SMALL)
        .padding([2, 6])
        .style(theme::pick)
        .menu_style(theme::menu)
        .into()
}

/// Wheel delta in lines, whatever the device reports.
pub fn wheel_lines(delta: mouse::ScrollDelta) -> (f32, f32) {
    match delta {
        mouse::ScrollDelta::Lines { x, y } => (x, y),
        mouse::ScrollDelta::Pixels { x, y } => (x / 20.0, y / 20.0),
    }
}

/// Scroll area with thin scrollbars. `vertical` and `horizontal` pick the axes.
pub fn scroll<'a>(content: impl Into<Element<'a, Message>>, vertical: bool, horizontal: bool) -> scrollable::Scrollable<'a, Message> {
    let bar = || Scrollbar::new().width(4).scroller_width(4).margin(0);
    let direction = match (vertical, horizontal) {
        (true, true) => Direction::Both { vertical: bar(), horizontal: bar() },
        (false, true) => Direction::Horizontal(bar()),
        _ => Direction::Vertical(bar()),
    };
    scrollable(content).direction(direction).style(theme::scrollable)
}

pub fn pick_or<'a, T>(options: Vec<T>, selected: Option<T>, placeholder: &str, on: impl Fn(T) -> Message + 'a) -> Element<'a, Message>
where
    T: ToString + PartialEq + Clone + 'a,
{
    pick_list(options, selected, on)
        .placeholder(placeholder.to_string())
        .text_size(theme::SMALL)
        .padding([2, 6])
        .style(theme::pick)
        .menu_style(theme::menu)
        .into()
}
