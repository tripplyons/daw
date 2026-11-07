//! Icons drawn on a 16-point grid with `currentColor`, so they take the
//! color of the text beside them.

use rune::Icon;

pub static PLAY: Icon = Icon { name: "daw-play", view: 16.0, body: r#"<path d="M5 3.2L12.6 8L5 12.8Z" fill="currentColor" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"/>"# };
pub static PAUSE: Icon = Icon { name: "daw-pause", view: 16.0, body: r#"<rect x="4" y="3.5" width="2.8" height="9" rx="0.8" fill="currentColor"/><rect x="9.2" y="3.5" width="2.8" height="9" rx="0.8" fill="currentColor"/>"# };
pub static STOP: Icon = Icon { name: "daw-stop", view: 16.0, body: r#"<rect x="4" y="4" width="8" height="8" rx="1.4" fill="currentColor"/>"# };
pub static RECORD: Icon = Icon { name: "daw-record", view: 16.0, body: r#"<circle cx="8" cy="8" r="4.2" fill="currentColor"/>"# };
pub static CHEVRON: Icon = Icon { name: "daw-chevron", view: 16.0, body: r#"<path d="M4.5 6.5L8 10L11.5 6.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"# };
pub static CHECK: Icon = Icon { name: "daw-check", view: 16.0, body: r#"<path d="M3.5 8.5L6.5 11.5L12.5 4.5" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"# };
pub static CLOSE: Icon = Icon { name: "daw-close", view: 16.0, body: r#"<path d="M4.5 4.5L11.5 11.5M11.5 4.5L4.5 11.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" fill="none"/>"# };
pub static MORE: Icon = Icon { name: "daw-more", view: 16.0, body: r#"<circle cx="3.8" cy="8" r="1.25" fill="currentColor"/><circle cx="8" cy="8" r="1.25" fill="currentColor"/><circle cx="12.2" cy="8" r="1.25" fill="currentColor"/>"# };
pub static PLUS: Icon = Icon { name: "daw-plus", view: 16.0, body: r#"<path d="M8 3.5V12.5M3.5 8H12.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" fill="none"/>"# };
pub static SEARCH: Icon = Icon { name: "daw-search", view: 16.0, body: r#"<circle cx="7" cy="7" r="4" stroke="currentColor" stroke-width="1.4" fill="none"/><path d="M10 10L13 13" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" fill="none"/>"# };
pub static SETTINGS: Icon = Icon { name: "daw-settings", view: 16.0, body: r#"<path d="M3 5H13M3 11H13" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" fill="none"/><circle cx="6" cy="5" r="1.8" fill="currentColor"/><circle cx="10.5" cy="11" r="1.8" fill="currentColor"/>"# };
pub static FILE: Icon = Icon { name: "daw-file", view: 16.0, body: r#"<path d="M4 2.5H9.5L12 5V13.5H4Z" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" fill="none"/><path d="M9.5 2.5V5H12" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" fill="none"/>"# };
pub static KEYBOARD: Icon = Icon { name: "daw-keyboard", view: 16.0, body: r#"<rect x="1.8" y="4" width="12.4" height="8" rx="1.6" stroke="currentColor" stroke-width="1.3" fill="none"/><path d="M4.5 6.8H5.2M7.6 6.8H8.4M10.8 6.8H11.5M5.5 9.4H10.5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" fill="none"/>"# };
pub static EXPAND: Icon = Icon { name: "daw-expand", view: 16.0, body: r#"<path d="M3 6.5V3H6.5M9.5 3H13V6.5M13 9.5V13H9.5M6.5 13H3V9.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"# };
pub static COLLAPSE: Icon = Icon { name: "daw-collapse", view: 16.0, body: r#"<path d="M6.5 3V6.5H3M13 6.5H9.5V3M9.5 13V9.5H13M3 9.5H6.5V13" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"# };
pub static UP: Icon = Icon { name: "daw-up", view: 16.0, body: r#"<path d="M4.5 9.5L8 6L11.5 9.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"# };
pub static PALETTE: Icon = Icon { name: "daw-command", view: 16.0, body: r#"<path d="M6 6V4.5A1.5 1.5 0 1 0 4.5 6H11.5A1.5 1.5 0 1 0 10 4.5V11.5A1.5 1.5 0 1 0 11.5 10H4.5A1.5 1.5 0 1 0 6 11.5Z" stroke="currentColor" stroke-width="1.2" fill="none"/>"# };
