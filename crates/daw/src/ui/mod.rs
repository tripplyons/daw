//! The Rune shell: starts the window, routes input and background messages
//! into `App::update`, and carries out the tasks it returns.

pub mod canvas;
pub mod icons;
pub mod widgets;

use std::any::Any;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rune::{AppHandle, El, FileDialogOptions, KeyEvent, TextEdit, Tx};

use crate::app::{App, Message};
use crate::input::{self, Key, Modifiers, key::{Code, Named}};
use crate::task::{Effect, Task};

/// Messages from other threads and native callbacks: plugin scans, menu
/// clicks, files opened from Finder, and keys from plugin windows.
static INBOX: LazyLock<(Sender<Message>, Mutex<Receiver<Message>>)> = LazyLock::new(|| {
    let (sender, receiver) = std::sync::mpsc::channel();
    (sender, Mutex::new(receiver))
});

static HANDLE: OnceLock<AppHandle<App>> = OnceLock::new();

/// Send a message to the app from any thread.
pub fn post(message: Message) {
    let _ = INBOX.0.send(message);
    wake();
}

pub fn sender() -> Sender<Message> {
    INBOX.0.clone()
}

/// UI state that is not part of the app's model: text field editors, the
/// open dropdown, and the pointer.
#[derive(Default)]
pub struct State {
    /// Editors for text fields by key. Interior mutability lets the view
    /// bring a field in line with the app's value while it builds it.
    pub fields: RefCell<HashMap<&'static str, TextEdit>>,
    /// Fields the view being built shows. A focused field that the view
    /// stops showing, such as the palette's search after a command runs,
    /// loses focus; otherwise keys would still go to it as typing.
    pub shown_fields: RefCell<HashSet<&'static str>>,
    /// Fields that had focus at the last poll, to report focus loss.
    focused: Vec<&'static str>,
    /// The open dropdown list.
    pub dropdown: Option<widgets::Dropdown>,
    /// The pointer over a canvas, in that canvas's points.
    pub pointer: Option<(&'static str, input::Point)>,
    /// The canvas a drag started in, until the button is released.
    pub dragging: Option<&'static str>,
    /// Each canvas's state, such as the drag in progress.
    pub canvases: RefCell<HashMap<&'static str, Box<dyn Any>>>,
    /// Each canvas's size at the last layout.
    pub sizes: RefCell<HashMap<&'static str, input::Size>>,
    /// Modifier keys held, as of the last key or pointer event.
    pub modifiers: Modifiers,
    last_tick: Option<Instant>,
    /// Room the window's own controls take at the transport's leading edge, in UI points.
    pub chrome_inset: std::cell::Cell<f32>,
    /// The window title last set, so it changes only when the project name or dirty state does.
    title: String,
    /// What the last tick showed, to skip drawing when nothing moved.
    shown: u64,
}

impl State {
    /// The editor for a field, created on first use.
    pub fn field(&mut self, key: &'static str) -> &mut TextEdit {
        self.fields.get_mut().entry(key).or_insert_with(|| TextEdit::new(""))
    }

    pub fn field_focused(&self) -> bool {
        self.fields.borrow().values().any(|f| f.focused)
    }
}

/// Messages for the UI layer itself.
#[derive(Debug, Clone)]
pub enum Msg {
    OpenDropdown(widgets::Dropdown),
    CloseDropdown,
    /// A field lost keyboard focus.
    Blurred(&'static str),
}

pub fn update(app: &mut App, message: Msg) -> Task {
    match message {
        Msg::OpenDropdown(dropdown) => {
            let same = app.ui.dropdown.as_ref().is_some_and(|d| d.key == dropdown.key);
            app.ui.dropdown = (!same).then_some(dropdown);
        }
        Msg::CloseDropdown => app.ui.dropdown = None,
        Msg::Blurred(key) => return app.field_blurred(key),
    }
    Task::none()
}

/// Run a message through the app and carry out what it asks for.
pub fn send(app: &mut App, tx: &mut Tx, message: Message) {
    let task = app.update(message);
    execute(app, tx, task);
    let title = app.title();
    if title != app.ui.title {
        tx.set_title(title.clone());
        app.ui.title = title;
    }
}

fn execute(app: &mut App, tx: &mut Tx, task: Task) {
    for effect in task.0 {
        match effect {
            Effect::Message(message) => send(app, tx, message),
            Effect::Dialog(dialog) => {
                let extensions: Vec<&str> = dialog.extensions.iter().map(String::as_str).collect();
                let mut options = FileDialogOptions { title: dialog.title.clone(), ..Default::default() }.filter(&dialog.filter, &extensions);
                options.directory = dialog.folder.clone();
                options.file_name = dialog.file_name.clone();
                let then = dialog.then;
                let answer = move |s: &mut App, result: Result<Option<std::path::PathBuf>, rune::FileDialogError>, tx: &mut Tx| {
                    let path = match result {
                        Ok(path) => path,
                        Err(error) => {
                            s.set_status(format!("could not show the file dialog: {error}"));
                            None
                        }
                    };
                    send(s, tx, then(path));
                };
                if dialog.save { tx.save_file(options, answer) } else { tx.open_file(options, answer) }
            }
            Effect::Thread(work) => {
                let sender = sender();
                std::thread::spawn(move || {
                    work(sender);
                    wake();
                });
            }
            Effect::Focus(key) => {
                let field = app.ui.field(key);
                field.focused = true;
                let end = field.text.len();
                field.set_selection(0, end);
            }
            Effect::ScrollX(key, by) => tx.scroll_to_x(key, by),
            Effect::Exit => tx.quit(),
        }
    }
}

/// Wake the shell so it takes queued messages.
pub fn wake() {
    if let Some(handle) = HANDLE.get() {
        handle.wake();
    }
}

/// Handle a message from a view, then run what the update asked for. The
/// press first focuses the tile under the pointer.
pub fn on<M: Into<Message> + Clone + 'static>(message: M) -> impl Fn(&mut App, &mut Tx) + 'static {
    move |app, tx| {
        touch(app);
        send(app, tx, message.clone().into())
    }
}

/// A press anywhere: focus the tile under the pointer.
pub fn touch(app: &mut App) {
    let _ = app.update(Message::MousePressed);
}

/// Before each frame: take background messages, advance the transport
/// clock, and report focus changes.
fn poll(app: &mut App, _clock: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    let queued: Vec<Message> = INBOX.1.lock().map(|r| r.try_iter().collect()).unwrap_or_default();
    for message in queued {
        send(app, tx, message);
        changed = true;
    }
    // Fields that lost focus apply their edits, as the tempo field does.
    let focused: Vec<&'static str> = app.ui.fields.borrow().iter().filter(|(_, f)| f.focused).map(|(k, _)| *k).collect();
    let blurred: Vec<&'static str> = app.ui.focused.iter().copied().filter(|k| !focused.contains(k)).collect();
    app.ui.focused = focused;
    for key in blurred {
        send(app, tx, Message::Ui(Msg::Blurred(key)));
        changed = true;
    }
    let now = Instant::now();
    let interval = if app.playing { Duration::from_millis(16) } else { Duration::from_millis(33) };
    if app.ui.last_tick.is_none_or(|t| now - t >= interval) {
        app.ui.last_tick = Some(now);
        send(app, tx, Message::Tick);
        let shown = app.tick_signature();
        if shown != app.ui.shown {
            app.ui.shown = shown;
            changed = true;
        }
    }
    tx.wake_after(interval);
    changed
}

/// Window keys after focused fields and controls had their turn.
fn on_key(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
    let typing = app.ui.field_focused();
    let modifiers = modifiers(event.mods);
    app.ui.modifiers = modifiers;
    // Escape closes the open dropdown first.
    if app.ui.dropdown.is_some() && matches!(event.key, rune::Key::Escape) {
        app.ui.dropdown = None;
        return true;
    }
    let code = tx.physical_key().and_then(input::code_from_rune).or_else(|| code_for(&event.key));
    let key = logical(&event.key);
    let text = match &event.key {
        rune::Key::Text(t) => Some(t.clone()),
        _ => None,
    };
    let event = input::Event::pressed(code, key, modifiers, text);
    send(app, tx, Message::Key(event, typing));
    true
}

fn on_key_up(app: &mut App, event: &KeyEvent, tx: &mut Tx) -> bool {
    let typing = app.ui.field_focused();
    let modifiers = modifiers(event.mods);
    app.ui.modifiers = modifiers;
    let code = tx.physical_key().and_then(input::code_from_rune).or_else(|| code_for(&event.key));
    let event = input::Event::released(code, logical(&event.key), modifiers);
    send(app, tx, Message::Key(event, typing));
    false
}

pub fn modifiers(mods: rune::Mods) -> Modifiers {
    let mut m = Modifiers::empty();
    m.set(Modifiers::SHIFT, mods.shift);
    m.set(Modifiers::ALT, mods.alt);
    m.set(Modifiers::CTRL, mods.ctrl);
    m.set(Modifiers::LOGO, mods.logo || (cfg!(target_os = "macos") && mods.cmd));
    if !cfg!(target_os = "macos") && mods.cmd {
        m.insert(Modifiers::CTRL);
    }
    m
}

fn logical(key: &rune::Key) -> Key {
    use rune::Key as R;
    let named = |n| Key::Named(n);
    match key {
        R::Text(t) if t == " " => named(Named::Space),
        R::Text(t) => Key::Character(t.to_lowercase()),
        R::Command(c) => Key::Character(c.to_string()),
        R::Enter => named(Named::Enter),
        R::Backspace => named(Named::Backspace),
        R::Delete => named(Named::Delete),
        R::Left => named(Named::ArrowLeft),
        R::Right => named(Named::ArrowRight),
        R::Up => named(Named::ArrowUp),
        R::Down => named(Named::ArrowDown),
        R::Home => named(Named::Home),
        R::End => named(Named::End),
        R::PageUp => named(Named::PageUp),
        R::PageDown => named(Named::PageDown),
        R::Escape => named(Named::Escape),
        R::Tab => named(Named::Tab),
        R::Function(n @ 1..=12) => {
            const F: [Named; 12] = [Named::F1, Named::F2, Named::F3, Named::F4, Named::F5, Named::F6, Named::F7, Named::F8, Named::F9, Named::F10, Named::F11, Named::F12];
            named(F[usize::from(*n) - 1])
        }
        _ => Key::Unidentified,
    }
}

/// The key's position, when the platform did not report one (headless
/// scripts), guessed from what it types on a US layout.
fn code_for(key: &rune::Key) -> Option<Code> {
    use rune::Key as R;
    let ch = match key {
        R::Text(t) => t.chars().next()?.to_ascii_lowercase(),
        R::Command(c) => c.to_ascii_lowercase(),
        R::Enter => return Some(Code::Enter),
        R::Backspace => return Some(Code::Backspace),
        R::Delete => return Some(Code::Delete),
        R::Left => return Some(Code::ArrowLeft),
        R::Right => return Some(Code::ArrowRight),
        R::Up => return Some(Code::ArrowUp),
        R::Down => return Some(Code::ArrowDown),
        R::Home => return Some(Code::Home),
        R::End => return Some(Code::End),
        R::PageUp => return Some(Code::PageUp),
        R::PageDown => return Some(Code::PageDown),
        R::Escape => return Some(Code::Escape),
        R::Tab => return Some(Code::Tab),
        R::Function(n) => {
            const F: [Code; 12] = [Code::F1, Code::F2, Code::F3, Code::F4, Code::F5, Code::F6, Code::F7, Code::F8, Code::F9, Code::F10, Code::F11, Code::F12];
            return F.get(usize::from(*n).checked_sub(1)?).copied();
        }
        _ => return None,
    };
    const LETTERS: [Code; 26] = [
        Code::KeyA, Code::KeyB, Code::KeyC, Code::KeyD, Code::KeyE, Code::KeyF, Code::KeyG, Code::KeyH, Code::KeyI,
        Code::KeyJ, Code::KeyK, Code::KeyL, Code::KeyM, Code::KeyN, Code::KeyO, Code::KeyP, Code::KeyQ, Code::KeyR,
        Code::KeyS, Code::KeyT, Code::KeyU, Code::KeyV, Code::KeyW, Code::KeyX, Code::KeyY, Code::KeyZ,
    ];
    const DIGITS: [Code; 10] = [Code::Digit0, Code::Digit1, Code::Digit2, Code::Digit3, Code::Digit4, Code::Digit5, Code::Digit6, Code::Digit7, Code::Digit8, Code::Digit9];
    Some(match ch {
        'a'..='z' => LETTERS[(ch as u8 - b'a') as usize],
        '0'..='9' => DIGITS[(ch as u8 - b'0') as usize],
        ' ' => Code::Space,
        '-' => Code::Minus,
        '=' => Code::Equal,
        '[' => Code::BracketLeft,
        ']' => Code::BracketRight,
        '\\' => Code::Backslash,
        ';' => Code::Semicolon,
        '\'' => Code::Quote,
        '`' => Code::Backquote,
        ',' => Code::Comma,
        '.' => Code::Period,
        '/' => Code::Slash,
        _ => return None,
    })
}

fn view(app: &App, cx: &mut rune::Cx<App>) -> El {
    // The transport is the titlebar: it grows with the UI scale, and the traffic
    // lights sit in it, so its controls start after them.
    let scale = f32::from(app.config.ui_scale) / 100.0;
    cx.titlebar_height(crate::app::TRANSPORT_HEIGHT * scale);
    app.ui.chrome_inset.set(cx.chrome_insets().leading / scale);
    app.ui.shown_fields.borrow_mut().clear();
    let root = app.root();
    let shown = app.ui.shown_fields.borrow();
    for (key, field) in app.ui.fields.borrow_mut().iter_mut() {
        if field.focused && !shown.contains(key) { field.focused = false; }
    }
    root
}

fn init() -> App {
    let (app, task) = App::boot();
    // The plugin scan starts after the first frame (see `on_ready`).
    *BOOT_TASK.lock().unwrap() = Some(task);
    app
}

/// What `App::boot` asked for, run once the window shows.
static BOOT_TASK: Mutex<Option<Task>> = Mutex::new(None);

fn setup(app: &mut App, _waker: rune::Waker) {
    crate::native_menu::refresh(app);
    // Work that needs no window, such as the plugin scan, starts now.
    if let Some(task) = BOOT_TASK.lock().unwrap().take() {
        for effect in task.0 {
            if let Effect::Thread(work) = effect {
                let sender = sender();
                std::thread::spawn(move || {
                    work(sender);
                    wake();
                });
            }
        }
    }
}

/// Review scenes for headless snapshots: `daw --snap out.png --scene NAME`.
const SCENES: &[(&str, fn(&mut App))] = &[
    ("demo", |app| crate::app::scenes::demo(app)),
    ("settings", |app| {
        crate::app::scenes::demo(app);
        app.settings.open = true;
    }),
    ("palette", |app| {
        crate::app::scenes::demo(app);
        let _ = crate::palette::open(app);
    }),
    ("mixer", |app| crate::app::scenes::mixer(app)),
    ("compact", |app| crate::app::scenes::demo(app)),
    ("empty", |_| {}),
];

/// Whether the command line asks for Rune's headless tools instead of a window.
pub fn headless_args(args: &[String]) -> bool {
    args.iter().any(|a| matches!(a.as_str(), "--snap" | "--tree" | "--script" | "--frames" | "--list-scenes" | "--video" | "--strip"))
}

/// Open the window, or with `headless`, run Rune's review tools on the
/// command line's arguments.
pub fn run(headless: bool) -> Result<(), String> {
    if !headless { rune::prewarm_gpu(); }
    let spec = rune::AppSpec {
        min_size: Some((640.0, 420.0)),
        scenes: SCENES,
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        ..rune::AppSpec::new("DAW", (1400.0, 860.0), init, view)
    };
    let app = rune::App::new(spec)
        .titlebar(rune::Titlebar::new(crate::app::TRANSPORT_HEIGHT))
        .keep_running()
        .on_close(|app, tx| {
            // Closing asks to save unsaved changes first; the window comes
            // back while the question is open.
            send(app, tx, Message::CloseRequested);
            if app.closing_pending() {
                if let Some(handle) = HANDLE.get() { handle.open(); }
            }
        });
    let argv: Vec<String> = if headless { std::env::args().skip(1).collect() } else { Vec::new() };
    let app = app.args(argv);
    let _ = HANDLE.set(app.handle());
    app.try_run()
}

/// The root element: the UI scale, key releases, files dropped on the
/// window, and pointer tracking.
pub fn root(app: &App, content: El) -> El {
    let scale = f32::from(app.config.ui_scale) / 100.0;
    let content = content.key("workspace").size(app.window.width, app.window.height);
    let content = if (scale - 1.0).abs() > 0.001 { content.scale(scale).origin(0.0, 0.0) } else { content };
    rune::el()
        .key("root")
        .w_full()
        .h_full()
        .fill(crate::theme::C.bg)
        .child(content)
        .on_layout_tx(move |app: &mut App, [_, _, w, h], tx| {
            let size = input::Size::new(w / scale, h / scale);
            if app.window != size {
                send(app, tx, Message::WindowResized(size));
            }
        })
        .on_key_up(on_key_up)
        .on_drop_files(|app: &mut App, files, tx| {
            for path in files {
                send(app, tx, Message::Dropped(path.clone()));
            }
        })
        .on_pointer_move_local(move |app: &mut App, p, tx| {
            let point = input::Point::new(p.window_x / scale, p.window_y / scale);
            send(app, tx, Message::MouseMoved(point));
            // Only drags draw at the pointer; other moves need no new view.
            if !app.follows_pointer() { tx.live_only(); }
        })
        .on_press_tx(|app: &mut App, tx: &mut Tx| {
            // A click on empty space focuses its tile and leaves text fields.
            touch(app);
            tx.blur();
        })
}
