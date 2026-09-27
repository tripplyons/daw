//! Native windows that host plugin editors, separate from the iced window.

use std::ffi::c_void;

use objc2::{MainThreadMarker, MainThreadOnly};
use objc2::rc::Retained;
use objc2_app_kit::{NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

pub struct EditorWindow {
    window: Retained<NSWindow>,
}

fn main_thread() -> MainThreadMarker {
    MainThreadMarker::new().expect("plugin editors are created on the main thread")
}

impl EditorWindow {
    pub fn new(title: &str, width: f64, height: f64) -> Self {
        let mtm = main_thread();
        let rect = NSRect::new(NSPoint::new(200.0, 200.0), NSSize::new(width.max(50.0), height.max(50.0)));
        let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable;
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect,
                style,
                NSBackingStoreType::Buffered,
                false,
            )
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
