//! The macOS menu bar: File, Edit, View, Transport, and Window menus built
//! from the key bindings, so every command can be found by browsing and each
//! one shows its shortcut.
//!
//! A click on an item sends its message to the app. A shortcut typed on the
//! keyboard also matches the item, but the item gives the key press back to
//! the window, so the key bindings and text fields handle it as before.

use std::sync::Mutex;

use daw_model::layout::Panel;
use iced::futures::channel::mpsc::{self, UnboundedSender};

use crate::app::{App, Message};
use crate::keys::{Action, BINDINGS, Chord};

/// One menu entry, or a separator when `message` is `None`.
#[derive(Debug, Clone)]
pub struct Item {
    pub title: String,
    pub chord: Option<Chord>,
    pub message: Option<Message>,
}

pub struct Menu {
    pub title: &'static str,
    pub items: Vec<Item>,
}

fn separator() -> Item {
    Item { title: String::new(), chord: None, message: None }
}

/// Sentence case for a native menu: "save as" becomes "Save as".
fn title(label: &str) -> String {
    let label = label.replace("export wav", "export WAV");
    let mut chars = label.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

fn binding(app: &App, id: &str) -> Item {
    let binding = BINDINGS.iter().find(|b| b.id == id).expect("menu entries name real bindings");
    Item {
        title: title(binding.label),
        chord: app.keymap.chords(id).first().copied(),
        message: Some(Message::Action(binding.action)),
    }
}

fn group(app: &App, group: &str, skip: &[&str]) -> Vec<Item> {
    BINDINGS
        .iter()
        .filter(|b| b.group == group && !skip.contains(&b.id) && b.action != Action::Palette)
        .map(|b| binding(app, b.id))
        .collect()
}

pub fn menus(app: &App) -> Vec<Menu> {
    let ids = |list: &[&str]| -> Vec<Item> {
        list.iter().map(|&id| if id == "-" { separator() } else { binding(app, id) }).collect()
    };
    let file = ids(&["new", "open", "save", "save-as", "-", "import-audio", "export", "pack", "recover", "-", "settings"]);
    let edit = ids(&["undo", "redo", "-", "delete", "context-menu", "-", "palette"]);
    let mut view: Vec<Item> = Panel::ALL
        .iter()
        .map(|&panel| Item { title: title(&format!("show {panel}")), chord: None, message: Some(Message::ShowPanel(panel)) })
        .collect();
    view.push(separator());
    view.extend(ids(&["zoom", "cycle-panel", "panel-tools"]));
    let mut transport = group(app, "transport", &[]);
    transport.push(separator());
    transport.extend(group(app, "automation", &[]));
    let mut window = group(app, "tiling", &["zoom", "cycle-panel", "panel-tools"]);
    window.push(separator());
    window.extend(group(app, "workspaces", &[]));
    vec![
        Menu { title: "File", items: file },
        Menu { title: "Edit", items: edit },
        Menu { title: "View", items: view },
        Menu { title: "Transport", items: transport },
        Menu { title: "Window", items: window },
    ]
}

/// Build or rebuild the menu bar when the key bindings change. Does nothing
/// off the main thread, as in tests, and on other platforms.
pub fn refresh(app: &App) {
    #[cfg(target_os = "macos")]
    macos::refresh(app);
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

/// Where menu clicks go: the app's subscription, once it runs.
static SENDER: Mutex<Option<UnboundedSender<Message>>> = Mutex::new(None);

/// Menu clicks as messages for the app's subscription.
pub fn clicks() -> impl iced::futures::Stream<Item = Message> {
    let (sender, receiver) = mpsc::unbounded();
    *SENDER.lock().unwrap() = Some(sender);
    receiver
}

#[cfg(target_os = "macos")]
mod macos {
    use std::cell::{Cell, RefCell};

    use objc2::rc::Retained;
    use objc2::runtime::Sel;
    use objc2::{AnyThread, MainThreadMarker, define_class, msg_send, sel};
    use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSEventType, NSMenu, NSMenuItem};
    use objc2_foundation::{NSObject, NSString};

    use super::{Item, menus};
    use crate::app::{App, Message};
    use crate::keys::Chord;

    struct Installed {
        /// The menu bar the items were added to; AppKit may replace it.
        bar: *const NSMenu,
        /// Titles and shortcuts, to skip rebuilding when nothing changed.
        signature: String,
        tops: Vec<Retained<NSMenuItem>>,
    }

    thread_local! {
        /// Shortcut and message per item tag.
        static ITEMS: RefCell<Vec<(Option<Chord>, Message)>> = const { RefCell::new(Vec::new()) };
        static TARGET: RefCell<Option<Retained<Target>>> = const { RefCell::new(None) };
        static INSTALLED: RefCell<Option<Installed>> = const { RefCell::new(None) };
        static FORWARDING: Cell<bool> = const { Cell::new(false) };
    }

    define_class!(
        // SAFETY: NSObject has no subclassing requirements, and this class
        // does not implement Drop.
        #[unsafe(super(NSObject))]
        #[name = "DawMenuTarget"]
        struct Target;

        impl Target {
            #[unsafe(method(run:))]
            fn run(&self, item: &NSMenuItem) {
                choose(item.tag());
            }
        }
    );

    fn choose(tag: isize) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let Some((chord, message)) = ITEMS.with(|items| items.borrow().get(tag as usize).cloned()) else { return };
        let app = NSApplication::sharedApplication(mtm);
        // A typed shortcut: hand the key press back to the window so the key
        // bindings decide, as they do for keys that are not in the menu.
        if let (Some(event), Some(chord), false) = (app.currentEvent(), chord, FORWARDING.get())
            && event.r#type() == NSEventType::KeyDown
            && typed(&event, chord)
            && let Some(window) = app.keyWindow()
        {
            FORWARDING.set(true);
            window.sendEvent(&event);
            FORWARDING.set(false);
            return;
        }
        if let Some(sender) = super::SENDER.lock().unwrap().as_ref() {
            let _ = sender.unbounded_send(message);
        }
    }

    fn typed(event: &objc2_app_kit::NSEvent, chord: Chord) -> bool {
        let flags = event.modifierFlags();
        crate::keys::mac_code(event.keyCode()) == Some(chord.code)
            && flags.contains(NSEventModifierFlags::Command) == chord.cmd
            && flags.contains(NSEventModifierFlags::Control) == chord.ctrl
            && flags.contains(NSEventModifierFlags::Option) == chord.alt
            && flags.contains(NSEventModifierFlags::Shift) == chord.shift
    }

    /// The character AppKit matches and draws for a chord's key.
    fn key_equivalent(chord: Chord) -> Option<String> {
        let name = chord.to_string();
        let key = name.rsplit('+').next()?;
        if key.chars().count() == 1 {
            return Some(key.to_string());
        }
        let c = match key {
            "up" => '\u{F700}',
            "down" => '\u{F701}',
            "left" => '\u{F702}',
            "right" => '\u{F703}',
            "enter" => '\r',
            "space" => ' ',
            "escape" => '\u{1b}',
            "tab" => '\t',
            "delete" => '\u{8}',
            "forwarddelete" => '\u{F728}',
            "home" => '\u{F729}',
            "end" => '\u{F72B}',
            "pageup" => '\u{F72C}',
            "pagedown" => '\u{F72D}',
            "minus" => '-',
            "equal" => '=',
            "bracketleft" => '[',
            "bracketright" => ']',
            "backslash" => '\\',
            "semicolon" => ';',
            "quote" => '\'',
            "backquote" => '`',
            "comma" => ',',
            "period" => '.',
            "slash" => '/',
            f if f.starts_with('f') => char::from_u32(0xF704 + f[1..].parse::<u32>().ok()?.checked_sub(1)?)?,
            _ => return None,
        };
        Some(c.to_string())
    }

    fn modifiers(chord: Chord) -> NSEventModifierFlags {
        let mut flags = NSEventModifierFlags::empty();
        for (on, flag) in [
            (chord.cmd, NSEventModifierFlags::Command),
            (chord.ctrl, NSEventModifierFlags::Control),
            (chord.alt, NSEventModifierFlags::Option),
            (chord.shift, NSEventModifierFlags::Shift),
        ] {
            if on {
                flags |= flag;
            }
        }
        flags
    }

    fn native(mtm: MainThreadMarker, title: &str, action: Sel, key: &str) -> Retained<NSMenuItem> {
        // SAFETY: the actions are standard NSWindow methods, sent to the key
        // window through the responder chain.
        unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(mtm.alloc(), &NSString::from_str(title), Some(action), &NSString::from_str(key)) }
    }

    fn item(mtm: MainThreadMarker, entry: &Item, target: &Target, tag: usize) -> Retained<NSMenuItem> {
        let key = entry.chord.and_then(key_equivalent).unwrap_or_default();
        // SAFETY: `run:` is defined on `Target`, which lives for the whole run.
        let menu_item = unsafe {
            let menu_item = NSMenuItem::initWithTitle_action_keyEquivalent(mtm.alloc(), &NSString::from_str(&entry.title), Some(sel!(run:)), &NSString::from_str(&key));
            menu_item.setTarget(Some(target));
            menu_item
        };
        if let Some(chord) = entry.chord.filter(|_| !key.is_empty()) {
            menu_item.setKeyEquivalentModifierMask(modifiers(chord));
        }
        menu_item.setTag(tag as isize);
        menu_item
    }

    pub fn refresh(app: &App) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let ns_app = NSApplication::sharedApplication(mtm);
        let Some(bar) = ns_app.mainMenu() else { return };
        let menus = menus(app);
        let signature: String = menus
            .iter()
            .flat_map(|m| std::iter::once(m.title.to_string()).chain(m.items.iter().map(|i| format!("{}={:?}", i.title, i.chord.map(|c| c.to_string())))))
            .collect::<Vec<_>>()
            .join("\n");
        let current = INSTALLED.with(|i| i.borrow().as_ref().is_some_and(|i| std::ptr::eq(i.bar, &*bar) && i.signature == signature));
        if current {
            return;
        }
        let target = TARGET.with(|t| t.borrow_mut().get_or_insert_with(|| unsafe { msg_send![Target::alloc(), init] }).clone());
        // Remove the menus a previous build added.
        if let Some(old) = INSTALLED.with(|i| i.borrow_mut().take()) {
            for top in &old.tops {
                if let Some(menu) = unsafe { top.menu() } {
                    menu.removeItem(top);
                }
            }
        }
        let cmd_m_free = !app_uses(app, "cmd+m");
        let mut actions = Vec::new();
        let mut tops = Vec::new();
        for menu in &menus {
            let submenu = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str(menu.title));
            if menu.title == "Window" {
                submenu.addItem(&native(mtm, "Minimize", sel!(performMiniaturize:), if cmd_m_free { "m" } else { "" }));
                submenu.addItem(&native(mtm, "Zoom", sel!(performZoom:), ""));
                submenu.addItem(&NSMenuItem::separatorItem(mtm));
            }
            for entry in &menu.items {
                match &entry.message {
                    None => submenu.addItem(&NSMenuItem::separatorItem(mtm)),
                    Some(message) => {
                        submenu.addItem(&item(mtm, entry, &target, actions.len()));
                        actions.push((entry.chord, message.clone()));
                    }
                }
            }
            if menu.title == "Window" {
                ns_app.setWindowsMenu(Some(&submenu));
            }
            let top = NSMenuItem::new(mtm);
            top.setTitle(&NSString::from_str(menu.title));
            top.setSubmenu(Some(&submenu));
            bar.addItem(&top);
            tops.push(top);
        }
        ITEMS.with(|items| *items.borrow_mut() = actions);
        INSTALLED.with(|i| *i.borrow_mut() = Some(Installed { bar: &*bar as *const NSMenu, signature, tops }));
    }

    fn app_uses(app: &App, chord: &str) -> bool {
        let Some(chord) = Chord::parse(chord) else { return false };
        crate::keys::BINDINGS.iter().any(|b| app.keymap.chords(b.id).contains(&chord))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menus_list_every_command_with_its_shortcut() {
        let mut app = App::boot().0;
        app.keymap = crate::keys::Keymap::default();
        let menus = menus(&app);
        let titles: Vec<&str> = menus.iter().map(|m| m.title).collect();
        assert_eq!(titles, ["File", "Edit", "View", "Transport", "Window"]);
        let all: Vec<&Item> = menus.iter().flat_map(|m| &m.items).collect();
        let save = all.iter().find(|i| i.title == "Save").unwrap();
        assert_eq!(save.chord.map(|c| c.to_string()), Some(if cfg!(target_os = "macos") { "cmd+s" } else { "ctrl+s" }.into()));
        assert!(all.iter().any(|i| i.title == "Export WAV"));
        for panel in Panel::ALL {
            assert!(all.iter().any(|i| matches!(i.message, Some(Message::ShowPanel(p)) if p == panel)), "{panel} is in the View menu");
        }
        for binding in BINDINGS.iter().filter(|b| b.action != Action::Palette) {
            assert!(
                all.iter().any(|i| matches!(i.message, Some(Message::Action(a)) if a == binding.action)) || binding.id == "palette",
                "{} is in a menu",
                binding.id
            );
        }
    }
}
