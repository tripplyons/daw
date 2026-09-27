//! Files opened from Finder (double click, Open With, or dropping on the Dock
//! icon). macOS sends these as an "open documents" Apple Event rather than as
//! command-line arguments, and winit does not handle it, so install our own
//! handler just before launch finishes, as Apple recommends.

use std::path::PathBuf;
use std::sync::Mutex;

use iced::futures::channel::mpsc::{self, UnboundedReceiver};

/// Paths waiting for the app's subscription to take them.
static RECEIVER: Mutex<Option<UnboundedReceiver<PathBuf>>> = Mutex::new(None);

/// Take the stream of opened files. Returns `None` after the first call.
pub fn take() -> Option<UnboundedReceiver<PathBuf>> {
    RECEIVER.lock().unwrap().take()
}

#[cfg(target_os = "macos")]
mod macos {
    use std::path::PathBuf;
    use std::ptr::NonNull;
    use std::sync::OnceLock;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::{AnyThread, define_class, msg_send, sel};
    use objc2_app_kit::NSApplicationWillFinishLaunchingNotification;
    use objc2_foundation::{NSObject, NSString};

    use iced::futures::channel::mpsc::UnboundedSender;

    /// Four-character Apple Event codes.
    const fn code(c: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*c)
    }
    const CORE_EVENT_CLASS: u32 = code(b"aevt");
    const OPEN_DOCUMENTS: u32 = code(b"odoc");
    const DIRECT_OBJECT: u32 = code(b"----");

    static SENDER: OnceLock<UnboundedSender<PathBuf>> = OnceLock::new();

    define_class!(
        // SAFETY: NSObject has no subclassing requirements, and this class
        // does not implement Drop.
        #[unsafe(super(NSObject))]
        #[name = "DawOpenFiles"]
        struct Handler;

        impl Handler {
            #[unsafe(method(handleOpen:withReplyEvent:))]
            fn handle_open(&self, event: &AnyObject, _reply: &AnyObject) {
                for path in paths(event) {
                    log::info!("open from Finder: {}", path.display());
                    if let Some(sender) = SENDER.get() {
                        let _ = sender.unbounded_send(path);
                    }
                }
            }
        }
    );

    /// File paths in an open documents event: a list of file URLs.
    fn paths(event: &AnyObject) -> Vec<PathBuf> {
        let list: Option<Retained<AnyObject>> = unsafe { msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT] };
        let Some(list) = list else { return Vec::new() };
        let count: isize = unsafe { msg_send![&*list, numberOfItems] };
        // Descriptor lists are 1-based.
        (1..=count)
            .filter_map(|index| {
                let item: Option<Retained<AnyObject>> = unsafe { msg_send![&*list, descriptorAtIndex: index] };
                let url: Option<Retained<AnyObject>> = unsafe { msg_send![&*item?, fileURLValue] };
                let path: Option<Retained<NSString>> = unsafe { msg_send![&*url?, path] };
                Some(PathBuf::from(path?.to_string()))
            })
            .collect()
    }

    pub fn install(sender: UnboundedSender<PathBuf>) {
        let _ = SENDER.set(sender);
        let Some(center_class) = AnyClass::get(c"NSNotificationCenter") else { return };
        let center: Retained<AnyObject> = unsafe { msg_send![center_class, defaultCenter] };
        let block = RcBlock::new(|_: NonNull<AnyObject>| register_handler());
        let name = unsafe { NSApplicationWillFinishLaunchingNotification };
        let observer: Option<Retained<AnyObject>> = unsafe {
            msg_send![
                &*center,
                addObserverForName: name,
                object: None::<&AnyObject>,
                queue: None::<&AnyObject>,
                usingBlock: &*block,
            ]
        };
        // Both live for the whole run.
        std::mem::forget(observer);
        std::mem::forget(block);
    }

    /// Replace AppKit's open documents handler, which would pass the files to
    /// winit's application delegate and drop them.
    fn register_handler() {
        let Some(manager_class) = AnyClass::get(c"NSAppleEventManager") else { return };
        let manager: Retained<AnyObject> = unsafe { msg_send![manager_class, sharedAppleEventManager] };
        let handler: Retained<Handler> = unsafe { msg_send![Handler::alloc(), init] };
        let () = unsafe {
            msg_send![
                &*manager,
                setEventHandler: &*handler,
                andSelector: sel!(handleOpen:withReplyEvent:),
                forEventClass: CORE_EVENT_CLASS,
                andEventID: OPEN_DOCUMENTS,
            ]
        };
        std::mem::forget(handler);
    }
}

/// Start listening for files opened from Finder. Call on the main thread
/// before the app starts running.
pub fn install() {
    let (sender, receiver) = mpsc::unbounded();
    *RECEIVER.lock().unwrap() = Some(receiver);
    #[cfg(target_os = "macos")]
    macos::install(sender);
    #[cfg(not(target_os = "macos"))]
    drop(sender);
}
