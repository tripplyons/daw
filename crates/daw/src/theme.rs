//! Design tokens: a layered graphite dark theme with one warm accent.
//!
//! Surfaces step up in lightness from the window background to raised
//! controls. The accent marks what is on or active (toggles, the selected
//! tool, the playhead); red is reserved for recording. Channel colors tint
//! what belongs to a channel. Type is Inter, with tabular figures wherever
//! numbers change in place.
//!
//! Colors are authored in OKLCH (neutrals at chroma 0.008, hue 265) and
//! stored as linear sRGB, which is what Rune draws with, so they can be
//! constants.

use rune::{Color, TextStyle};

/// The window behind everything, and gutters between tiles.
pub const BG: Color = Color { r: 0.00319, g: 0.00374, b: 0.00506, a: 1.000 };
/// Panel bodies.
pub const SURFACE: Color = Color { r: 0.00605, g: 0.00689, b: 0.00884, a: 1.000 };
/// Recessed wells: tracks, meters, inputs.
pub const WELL: Color = Color { r: 0.00389, g: 0.00451, b: 0.00600, a: 1.000 };
/// Panel headers, the transport, side panes, and rulers.
pub const HEADER: Color = Color { r: 0.00890, g: 0.00998, b: 0.01245, a: 1.000 };
/// Buttons and other controls at rest.
pub const RAISED: Color = Color { r: 0.01703, g: 0.01869, b: 0.02240, a: 1.000 };
/// Controls under the pointer, and selected list rows.
pub const RAISED_HOVER: Color = Color { r: 0.02496, g: 0.02710, b: 0.03183, a: 1.000 };
/// Popovers, menus, and dialogs.
pub const POPOVER: Color = Color { r: 0.01173, g: 0.01303, b: 0.01597, a: 1.000 };
/// Hairlines between areas.
pub const LINE: Color = Color { r: 0.00161, g: 0.00196, b: 0.00283, a: 1.000 };
/// Borders on controls and popovers.
pub const BORDER: Color = Color { r: 1.00000, g: 1.00000, b: 1.00000, a: 0.080 };
/// Timeline grid at the finest step.
pub const GRID: Color = Color { r: 0.00890, g: 0.00998, b: 0.01245, a: 1.000 };
/// Timeline grid on bars.
pub const GRID_BAR: Color = Color { r: 0.02496, g: 0.02710, b: 0.03183, a: 1.000 };
/// Primary text.
pub const TEXT: Color = Color { r: 0.81036, g: 0.83184, b: 0.87691, a: 1.000 };
/// Secondary text.
pub const TEXT_DIM: Color = Color { r: 0.39271, g: 0.40598, b: 0.43402, a: 1.000 };
/// Hints and disabled text.
pub const TEXT_FAINT: Color = Color { r: 0.16846, g: 0.17603, b: 0.19218, a: 1.000 };
/// Laid over any fill under the pointer.
pub const HOVER: Color = Color { r: 1.00000, g: 1.00000, b: 1.00000, a: 0.060 };
/// Laid over any fill while pressed.
pub const PRESS: Color = Color { r: 1.00000, g: 1.00000, b: 1.00000, a: 0.100 };
/// What is on or active: toggles, the chosen tool, the playhead.
pub const ACCENT: Color = Color { r: 0.95963, g: 0.40332, b: 0.06621, a: 1.000 };
/// The accent at low strength, behind accent text.
pub const ACCENT_SOFT: Color = Color { r: 0.95963, g: 0.40332, b: 0.06621, a: 0.160 };
/// Text on an accent fill.
pub const ON_ACCENT: Color = Color { r: 0.01444, g: 0.00633, b: 0.00229, a: 1.000 };
/// Armed recording.
pub const RECORD: Color = Color { r: 0.84677, g: 0.06831, b: 0.06710, a: 1.000 };
/// Errors.
pub const DANGER: Color = Color { r: 0.92133, g: 0.15215, b: 0.13454, a: 1.000 };
/// Healthy levels and finished jobs.
pub const SUCCESS: Color = Color { r: 0.18028, g: 0.62164, b: 0.25563, a: 1.000 };
/// Selected notes, clips, and points.
pub const SELECTION: Color = Color { r: 0.91871, g: 0.84963, b: 0.74455, a: 1.000 };
/// The focused tile's outline.
pub const FOCUS: Color = Color { r: 0.13531, g: 0.39965, b: 0.84509, a: 1.000 };
/// Dims the workspace behind a dialog.
pub const SCRIM: Color = Color { r: 0.00000, g: 0.00000, b: 0.00000, a: 0.550 };
/// Drop shadows.
pub const SHADOW: Color = Color { r: 0.00000, g: 0.00000, b: 0.00000, a: 0.450 };

/// Mid gray for values without a channel color.
pub const FILL: Color = Color { r: 0.21000, g: 0.21900, b: 0.23800, a: 1.000 };
pub const FILL_DIM: Color = Color { r: 0.05200, g: 0.05550, b: 0.06300, a: 1.000 };

// Names the canvases use.
pub const BRIGHT: Color = TEXT;
pub const SELECTED: Color = SELECTION;
pub const CONTROL_HOVER: Color = RAISED_HOVER;
pub const GRID_STRONG: Color = GRID_BAR;

/// Every color, for views that read them as fields.
pub struct Palette {
    pub bg: Color,
    pub surface: Color,
    pub well: Color,
    pub header: Color,
    pub raised: Color,
    pub raised_hover: Color,
    pub popover: Color,
    pub line: Color,
    pub border: Color,
    pub text: Color,
    pub text_dim: Color,
    pub text_faint: Color,
    pub hover: Color,
    pub press: Color,
    pub accent: Color,
    pub accent_soft: Color,
    pub on_accent: Color,
    pub record: Color,
    pub danger: Color,
    pub success: Color,
    pub focus: Color,
    pub scrim: Color,
    pub shadow: Color,
}

pub const C: Palette = Palette {
    bg: BG,
    surface: SURFACE,
    well: WELL,
    header: HEADER,
    raised: RAISED,
    raised_hover: RAISED_HOVER,
    popover: POPOVER,
    line: LINE,
    border: BORDER,
    text: TEXT,
    text_dim: TEXT_DIM,
    text_faint: TEXT_FAINT,
    hover: HOVER,
    press: PRESS,
    accent: ACCENT,
    accent_soft: ACCENT_SOFT,
    on_accent: ON_ACCENT,
    record: RECORD,
    danger: DANGER,
    success: SUCCESS,
    focus: FOCUS,
    scrim: SCRIM,
    shadow: SHADOW,
};

/// A model color, 0xRRGGBB, as linear sRGB.
pub fn rgb(color: u32) -> Color {
    let channel = |shift: u32| decode(((color >> shift) & 0xff) as f32 / 255.0);
    Color { r: channel(16), g: channel(8), b: channel(0), a: 1.0 }
}

/// An sRGB channel, 0 to 1, as linear light.
fn decode(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// `a` moved toward `b` by `t`, from 0 to 1, in Oklab, so steps look even.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let (x, y) = (a.to_oklab(), b.to_oklab());
    Color::from_oklab(std::array::from_fn(|i| x[i] + (y[i] - x[i]) * t))
}

/// Body text in controls and lists.
pub const BODY: TextStyle = TextStyle::new(12.5, 400.0);
/// Labels on buttons and fields.
pub const LABEL: TextStyle = TextStyle::new(12.0, 500.0);
/// Secondary text: hints, captions, field labels.
pub const SMALL_TEXT: TextStyle = TextStyle::new(11.0, 400.0);
/// Section names in panes and dialogs.
pub const SECTION: TextStyle = TextStyle::new(11.0, 600.0).tracking(0.2);
/// Titles of dialogs and empty states.
pub const TITLE: TextStyle = TextStyle::new(14.0, 600.0);
/// Changing numbers: values, times, meters.
pub const NUMBER: TextStyle = TextStyle::new(11.0, 500.0).features("'tnum' 1");
/// The transport's position readout.
pub const CLOCK: TextStyle = TextStyle::new(17.0, 500.0).features("'tnum' 1, 'zero' 1");
/// Small print inside canvases: bar numbers, key names, clip names.
pub const CANVAS: TextStyle = TextStyle::new(10.5, 500.0).features("'tnum' 1");

pub const SMALL: f32 = 11.0;
pub const HEADER_HEIGHT: f32 = 34.0;
pub const CONTROL_HEIGHT: f32 = 24.0;
pub const RADIUS: f32 = 6.0;
pub const RADIUS_LG: f32 = 10.0;
