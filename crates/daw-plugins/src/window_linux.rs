//! X11 windows that host VST3 editors, which also work under XWayland. Each
//! window has its own X connection, so reading its events never takes events
//! meant for iced or the plugin. Use from the UI thread only.

use std::cell::Cell;
use std::ffi::{CString, c_void};
use std::rc::Rc;

use x11_dl::xlib;

use crate::PluginError;

struct Window {
    xlib: xlib::Xlib,
    display: *mut xlib::Display,
    id: xlib::Window,
    delete: xlib::Atom,
    protocols: xlib::Atom,
    visible: Cell<bool>,
    closed: Cell<bool>,
    size: Cell<(i32, i32)>,
    resizable: bool,
}

impl Window {
    fn resize(&self, width: i32, height: i32) {
        if self.closed.get() || !valid_size(width, height) {
            return;
        }
        self.size.set((width, height));
        self.set_size_hints(width, height);
        unsafe {
            (self.xlib.XResizeWindow)(self.display, self.id, width as u32, height as u32);
            (self.xlib.XFlush)(self.display);
        }
    }

    fn set_size_hints(&self, width: i32, height: i32) {
        let mut hints: xlib::XSizeHints = unsafe { std::mem::zeroed() };
        hints.flags = xlib::PMinSize | xlib::PMaxSize;
        hints.min_width = if self.resizable { 1 } else { width };
        hints.min_height = if self.resizable { 1 } else { height };
        hints.max_width = if self.resizable { 16384 } else { width };
        hints.max_height = if self.resizable { 16384 } else { height };
        unsafe { (self.xlib.XSetWMNormalHints)(self.display, self.id, &mut hints) };
    }

    fn hide(&self) {
        if !self.closed.get() {
            self.visible.set(false);
            unsafe {
                (self.xlib.XUnmapWindow)(self.display, self.id);
                (self.xlib.XFlush)(self.display);
            }
        }
    }

    fn close(&self) {
        if !self.closed.replace(true) {
            self.visible.set(false);
            unsafe {
                (self.xlib.XDestroyWindow)(self.display, self.id);
                (self.xlib.XFlush)(self.display);
            }
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        self.close();
        unsafe { (self.xlib.XCloseDisplay)(self.display) };
    }
}

pub struct EditorWindow(Rc<Window>);

fn valid_size(width: i32, height: i32) -> bool {
    (1..=16384).contains(&width) && (1..=16384).contains(&height)
}

impl EditorWindow {
    pub fn new(title: &str, width: i32, height: i32, resizable: bool) -> Result<Self, PluginError> {
        if !valid_size(width, height) {
            return Err(PluginError::Load(format!("invalid plugin editor size {width}x{height}")));
        }
        let xlib = xlib::Xlib::open().map_err(|e| PluginError::Load(format!("X11 library unavailable: {e}")))?;
        let display = unsafe { (xlib.XOpenDisplay)(std::ptr::null()) };
        if display.is_null() {
            return Err(PluginError::Load("plugin editors need an X11 display; enable XWayland on Wayland desktops".into()));
        }
        let id = unsafe {
            let root = (xlib.XDefaultRootWindow)(display);
            (xlib.XCreateSimpleWindow)(display, root, 200, 200, width as u32, height as u32, 0, 0, 0)
        };
        if id == 0 {
            unsafe { (xlib.XCloseDisplay)(display) };
            return Err(PluginError::Load("could not create X11 plugin window".into()));
        }
        let title = CString::new(title.replace('\0', " ")).unwrap();
        let (delete, protocols) = unsafe {
            (xlib.XStoreName)(display, id, title.as_ptr());
            (xlib.XSelectInput)(display, id, xlib::StructureNotifyMask);
            let mut delete = (xlib.XInternAtom)(display, c"WM_DELETE_WINDOW".as_ptr(), xlib::False);
            let protocols = (xlib.XInternAtom)(display, c"WM_PROTOCOLS".as_ptr(), xlib::False);
            (xlib.XSetWMProtocols)(display, id, &mut delete, 1);
            (delete, protocols)
        };
        let window = Self(Rc::new(Window {
            xlib,
            display,
            id,
            delete,
            protocols,
            visible: Cell::new(false),
            closed: Cell::new(false),
            size: Cell::new((width, height)),
            resizable,
        }));
        window.0.set_size_hints(width, height);
        unsafe { (window.0.xlib.XFlush)(display) };
        Ok(window)
    }

    /// The parent for `IPlugView::attached`: VST3 passes the X window id
    /// itself as the pointer.
    pub fn content_view(&self) -> *mut c_void {
        self.0.id as usize as *mut c_void
    }

    pub fn resizer(&self) -> impl Fn(i32, i32) + 'static {
        let window = self.0.clone();
        move |width, height| window.resize(width, height)
    }

    pub fn resize(&self, width: i32, height: i32) {
        self.0.resize(width, height);
    }

    pub fn show(&self) {
        if !self.0.closed.get() {
            self.0.visible.set(true);
            unsafe {
                (self.0.xlib.XMapRaised)(self.0.display, self.0.id);
                (self.0.xlib.XFlush)(self.0.display);
            }
        }
    }

    pub fn hide(&self) {
        self.0.hide();
    }

    pub fn close(&self) {
        self.0.close();
    }

    pub fn is_visible(&self) -> bool {
        self.0.visible.get() && !self.0.closed.get()
    }

    /// Handle this window's pending events: the close button hides it, and a
    /// user resize returns the new size. The plugin reads its own connection
    /// through the run loop.
    pub fn poll(&self) -> Option<(i32, i32)> {
        if self.0.closed.get() {
            return None;
        }
        let mut resized = None;
        while unsafe { (self.0.xlib.XPending)(self.0.display) } > 0 {
            let mut event: xlib::XEvent = unsafe { std::mem::zeroed() };
            unsafe { (self.0.xlib.XNextEvent)(self.0.display, &mut event) };
            match event.get_type() {
                xlib::ClientMessage => {
                    let message = unsafe { event.client_message };
                    if message.window == self.0.id
                        && message.message_type == self.0.protocols
                        && message.data.get_long(0) as xlib::Atom == self.0.delete
                    {
                        self.hide();
                    }
                }
                xlib::ConfigureNotify => {
                    let event = unsafe { event.configure };
                    let size = (event.width, event.height);
                    if event.window == self.0.id && valid_size(size.0, size.1) && size != self.0.size.get() {
                        self.0.size.set(size);
                        resized = Some(size);
                    }
                }
                xlib::DestroyNotify if unsafe { event.destroy_window.window } == self.0.id => {
                    self.0.closed.set(true);
                    self.0.visible.set(false);
                }
                _ => {}
            }
        }
        resized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_editor_dimensions() {
        for (width, height) in [(0, 300), (400, -1), (16385, 300), (400, i32::MAX)] {
            assert!(EditorWindow::new("invalid", width, height, false).is_err());
        }
        assert!(valid_size(400, 300));
    }

    #[test]
    #[ignore = "requires an X11 display"]
    fn window_manager_close_hides_and_reopens_same_parent() {
        let window = EditorWindow::new("DAW X11 lifecycle test", 400, 300, false).unwrap();
        window.show();
        assert!(window.is_visible());
        let mut message: xlib::XClientMessageEvent = unsafe { std::mem::zeroed() };
        message.type_ = xlib::ClientMessage;
        message.display = window.0.display;
        message.window = window.0.id;
        message.message_type = window.0.protocols;
        message.format = 32;
        message.data.set_long(0, window.0.delete as _);
        let mut event = xlib::XEvent { client_message: message };
        unsafe {
            (window.0.xlib.XSendEvent)(window.0.display, window.0.id, xlib::False, 0, &mut event);
            (window.0.xlib.XSync)(window.0.display, xlib::False);
        }
        window.poll();
        assert!(!window.is_visible());
        let parent = window.content_view();
        window.show();
        assert!(window.is_visible());
        assert_eq!(window.content_view(), parent);
        window.close();
        window.close();
        assert!(!window.is_visible());
    }
}
