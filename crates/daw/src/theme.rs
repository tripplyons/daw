//! Grayscale dark theme. White is reserved for selection, focus, and the playhead.

use iced::widget::{button, container, pick_list, slider, text_input};
use iced::{Background, Border, Color, Theme};

const fn gray(level: u8) -> Color {
    Color::from_rgb8(level, level, level)
}

pub const BG: Color = gray(0x14);
pub const HEADER: Color = gray(0x1b);
pub const CONTROL: Color = gray(0x25);
pub const CONTROL_HOVER: Color = gray(0x30);
pub const CONTROL_ACTIVE: Color = gray(0x3c);
pub const LINE: Color = gray(0x26);
pub const GRID: Color = gray(0x1e);
pub const GRID_STRONG: Color = gray(0x2e);
pub const TEXT: Color = gray(0xd0);
pub const TEXT_DIM: Color = gray(0x80);
pub const TEXT_FAINT: Color = gray(0x55);
pub const BRIGHT: Color = gray(0xf4);
pub const FILL: Color = gray(0x8a);
pub const FILL_DIM: Color = gray(0x4a);
pub const SELECTED: Color = gray(0xe8);

pub const TEXT_SIZE: f32 = 12.0;
pub const SMALL: f32 = 11.0;
pub const HEADER_HEIGHT: f32 = 22.0;

pub fn theme() -> Theme {
    Theme::custom(
        "grayscale",
        iced::theme::Palette {
            background: BG,
            text: TEXT,
            primary: gray(0x9a),
            success: gray(0xb0),
            warning: gray(0xc0),
            danger: gray(0xd8),
        },
    )
}

fn flat(background: Color, text: Color) -> button::Style {
    button::Style {
        background: Some(Background::Color(background)),
        text_color: text,
        border: Border::default(),
        ..button::Style::default()
    }
}

/// Default flat button.
pub fn control(_: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Hovered => flat(CONTROL_HOVER, BRIGHT),
        button::Status::Pressed => flat(CONTROL_ACTIVE, BRIGHT),
        button::Status::Disabled => flat(CONTROL, TEXT_FAINT),
        button::Status::Active => flat(CONTROL, TEXT),
    }
}

/// Button that shows an on/off state by inverting.
pub fn toggle(on: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        if on {
            match status {
                button::Status::Hovered | button::Status::Pressed => flat(BRIGHT, BG),
                _ => flat(SELECTED, BG),
            }
        } else {
            control(theme, status)
        }
    }
}

/// Text-only button, for list rows and headers.
pub fn plain(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let text = if selected { BRIGHT } else { TEXT };
        match status {
            button::Status::Hovered | button::Status::Pressed => flat(CONTROL, BRIGHT),
            _ if selected => flat(CONTROL_ACTIVE, text),
            _ => button::Style { background: None, text_color: text, ..button::Style::default() },
        }
    }
}

/// Right-click menu row, highlighted like a pick list menu row.
pub fn menu_entry(_: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Hovered | button::Status::Pressed => flat(SELECTED, BG),
        _ => button::Style { background: None, text_color: TEXT, ..button::Style::default() },
    }
}

/// Right-click menu box, matching pick list menus.
pub fn popup(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(CONTROL)),
        border: Border { color: LINE, width: 1.0, radius: 0.0.into() },
        text_color: Some(TEXT),
        ..container::Style::default()
    }
}

pub fn panel(_: &Theme) -> container::Style {
    container::Style { background: Some(Background::Color(BG)), text_color: Some(TEXT), ..container::Style::default() }
}

pub fn header(_: &Theme) -> container::Style {
    container::Style { background: Some(Background::Color(HEADER)), text_color: Some(TEXT), ..container::Style::default() }
}

pub fn fill(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style { background: Some(Background::Color(color)), ..container::Style::default() }
}

pub fn input(_: &Theme, status: text_input::Status) -> text_input::Style {
    let background = match status {
        text_input::Status::Focused { .. } => CONTROL_ACTIVE,
        text_input::Status::Hovered => CONTROL_HOVER,
        _ => CONTROL,
    };
    text_input::Style {
        background: Background::Color(background),
        border: Border::default(),
        icon: TEXT_DIM,
        placeholder: TEXT_FAINT,
        value: BRIGHT,
        selection: FILL_DIM,
    }
}

pub fn pick(_: &Theme, status: pick_list::Status) -> pick_list::Style {
    let background = match status {
        pick_list::Status::Hovered | pick_list::Status::Opened { .. } => CONTROL_HOVER,
        _ => CONTROL,
    };
    pick_list::Style {
        text_color: TEXT,
        placeholder_color: TEXT_FAINT,
        handle_color: TEXT_DIM,
        background: Background::Color(background),
        border: Border::default(),
    }
}

pub fn menu(_: &Theme) -> iced::overlay::menu::Style {
    iced::overlay::menu::Style {
        background: Background::Color(CONTROL),
        border: Border { color: LINE, width: 1.0, radius: 0.0.into() },
        text_color: TEXT,
        selected_text_color: BG,
        selected_background: Background::Color(SELECTED),
        shadow: iced::Shadow::default(),
    }
}

pub fn fader(_: &Theme, status: slider::Status) -> slider::Style {
    let handle = match status {
        slider::Status::Hovered | slider::Status::Dragged => BRIGHT,
        slider::Status::Active => TEXT,
    };
    slider::Style {
        rail: slider::Rail {
            backgrounds: (Background::Color(FILL), Background::Color(CONTROL)),
            width: 2.0,
            border: Border::default(),
        },
        handle: slider::Handle {
            shape: slider::HandleShape::Rectangle { width: 8, border_radius: 0.0.into() },
            background: Background::Color(handle),
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
        },
    }
}

pub fn scrollable(theme: &Theme, status: iced::widget::scrollable::Status) -> iced::widget::scrollable::Style {
    use iced::widget::scrollable::{Rail, Scroller, Status};
    let mut style = iced::widget::scrollable::default(theme, status);
    let active = matches!(status, Status::Hovered { .. } | Status::Dragged { .. });
    let rail = Rail {
        background: None,
        border: Border::default(),
        scroller: Scroller { background: Background::Color(if active { FILL } else { FILL_DIM }), border: Border::default() },
    };
    style.vertical_rail = rail;
    style.horizontal_rail = rail;
    style.gap = None;
    style
}
