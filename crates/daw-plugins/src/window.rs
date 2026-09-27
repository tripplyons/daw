//! Native windows that host plugin editors, separate from the iced window.

use std::cell::RefCell;
use std::ffi::c_void;

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSBackingStoreType, NSEvent, NSEventModifierFlags, NSView, NSWindow, NSWindowStyleMask};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

/// A key press that the plugin editor did not handle.
#[derive(Debug, Clone, Copy)]
pub struct KeyPress {
    /// macOS virtual key code, which names a physical key position.
    pub key_code: u16,
    pub cmd: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub repeat: bool,
}

type KeyHandler = Box<dyn Fn(KeyPress) -> bool>;

thread_local! {
    static UNHANDLED_KEYS: RefCell<Option<KeyHandler>> = const { RefCell::new(None) };
}

/// Receive key presses that reach an editor window because no view in the
/// plugin used them. Return true to mark a key as used; false lets the window
/// handle it as usual (a beep). Must be called on the main thread.
pub fn set_unhandled_keys(handler: impl Fn(KeyPress) -> bool + 'static) {
    UNHANDLED_KEYS.with(|h| *h.borrow_mut() = Some(Box::new(handler)));
}

define_class!(
    // SAFETY: NSWindow has no subclassing requirements, and this class does
    // not implement Drop.
    #[unsafe(super(NSWindow))]
    #[thread_kind = MainThreadOnly]
    #[name = "DawEditorWindow"]
    struct KeyWindow;

    impl KeyWindow {
        /// The end of the responder chain: views pass keys up to here when
        /// they do not use them.
        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let flags = event.modifierFlags();
            let press = KeyPress {
                key_code: event.keyCode(),
                cmd: flags.contains(NSEventModifierFlags::Command),
                ctrl: flags.contains(NSEventModifierFlags::Control),
                alt: flags.contains(NSEventModifierFlags::Option),
                shift: flags.contains(NSEventModifierFlags::Shift),
                repeat: event.isARepeat(),
            };
            let used = UNHANDLED_KEYS.with(|h| h.borrow().as_ref().is_some_and(|handler| handler(press)));
            if !used {
                let _: () = unsafe { msg_send![super(self), keyDown: event] };
            }
        }
    }
);

pub struct EditorWindow {
    window: Retained<KeyWindow>,
}

fn main_thread() -> MainThreadMarker {
    MainThreadMarker::new().expect("plugin editors are created on the main thread")
}

impl EditorWindow {
    pub fn new(title: &str, width: f64, height: f64) -> Self {
        let mtm = main_thread();
        let rect = NSRect::new(NSPoint::new(200.0, 200.0), NSSize::new(width.max(50.0), height.max(50.0)));
        let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable;
        let window: Retained<KeyWindow> = unsafe {
            msg_send![
                KeyWindow::alloc(mtm),
                initWithContentRect: rect,
                styleMask: style,
                backing: NSBackingStoreType::Buffered,
                defer: false,
            ]
        };
        // The title bar close button only hides the window: the object stays
        // alive so the same editor can be shown again.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str(title));
        window.center();
        Self { window }
    }

    pub fn content_view(&self) -> *mut c_void {
        self.window.contentView().map(|v| Retained::as_ptr(&v) as *mut c_void).unwrap_or(std::ptr::null_mut())
    }

    pub fn set_content_view(&self, view: &NSView) {
        let size = view.frame().size;
        self.window.setContentView(Some(view));
        self.window.setContentSize(size);
    }

    /// A callback that resizes the window's content area, for plugin-initiated resizes.
    pub fn resizer(&self) -> impl Fn(i32, i32) + 'static {
        let window = self.window.clone();
        move |width, height| window.setContentSize(NSSize::new(f64::from(width), f64::from(height)))
    }

    pub fn show(&self) {
        self.window.makeKeyAndOrderFront(None);
    }

    /// Take the window off screen without closing it, so it can be shown again.
    pub fn hide(&self) {
        self.window.orderOut(None);
    }

    pub fn is_visible(&self) -> bool {
        self.window.isVisible()
    }

    pub fn close(&self) {
        self.window.close();
    }
}
