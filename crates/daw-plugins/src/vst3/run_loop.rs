//! The host run loop VST3 editors use on Linux (`IRunLoop`). Plugins register
//! file descriptors (usually their X connection) and timers, and the host
//! calls them back on the UI thread; `pump` runs from `take_touches`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use vst3::Steinberg::Linux::{
    FileDescriptor, IEventHandler, IEventHandlerTrait, ITimerHandler, ITimerHandlerTrait, TimerInterval,
};
use vst3::Steinberg::{kInvalidArgument, kResultFalse, kResultOk, tresult};
use vst3::{ComPtr, ComRef};

struct Watch {
    handler: ComPtr<IEventHandler>,
    fd: FileDescriptor,
}

struct Timer {
    handler: ComPtr<ITimerHandler>,
    interval: Duration,
    next: Cell<Instant>,
}

#[derive(Default)]
pub(super) struct RunLoop {
    watches: RefCell<Vec<Rc<Watch>>>,
    timers: RefCell<Vec<Rc<Timer>>>,
}

impl RunLoop {
    pub(super) unsafe fn register_event_handler(&self, handler: *mut IEventHandler, fd: FileDescriptor) -> tresult {
        let Some(handler) = (unsafe { ComRef::from_raw(handler) }) else { return kInvalidArgument };
        if fd < 0 {
            return kInvalidArgument;
        }
        self.watches.borrow_mut().push(Rc::new(Watch { handler: handler.to_com_ptr(), fd }));
        kResultOk
    }

    /// Removes every descriptor registered for `handler`.
    pub(super) unsafe fn unregister_event_handler(&self, handler: *mut IEventHandler) -> tresult {
        // Drop the removed handlers after the borrow ends: releasing one can
        // call back into the plugin, which may call back into us.
        let removed: Vec<_> = {
            let mut watches = self.watches.borrow_mut();
            let (removed, kept) = std::mem::take(&mut *watches).into_iter().partition(|w| w.handler.as_ptr() == handler);
            *watches = kept;
            removed
        };
        if removed.is_empty() { kResultFalse } else { kResultOk }
    }

    pub(super) unsafe fn register_timer(&self, handler: *mut ITimerHandler, milliseconds: TimerInterval) -> tresult {
        let Some(handler) = (unsafe { ComRef::from_raw(handler) }) else { return kInvalidArgument };
        if milliseconds == 0 {
            return kInvalidArgument;
        }
        let interval = Duration::from_millis(milliseconds);
        let timer = Timer { handler: handler.to_com_ptr(), interval, next: Cell::new(Instant::now() + interval) };
        self.timers.borrow_mut().push(Rc::new(timer));
        kResultOk
    }

    pub(super) unsafe fn unregister_timer(&self, handler: *mut ITimerHandler) -> tresult {
        let removed = {
            let mut timers = self.timers.borrow_mut();
            timers.iter().position(|t| t.handler.as_ptr() == handler).map(|i| timers.remove(i))
        };
        if removed.is_some() { kResultOk } else { kResultFalse }
    }

    /// Release every handler. Call after `IPlugView::removed`.
    pub(super) fn clear(&self) {
        let watches = std::mem::take(&mut *self.watches.borrow_mut());
        let timers = std::mem::take(&mut *self.timers.borrow_mut());
        drop((watches, timers));
    }

    /// Call ready descriptors and due timers without blocking. Callbacks may
    /// register or unregister handlers, so iterate over a snapshot and skip
    /// entries removed by an earlier callback.
    pub(super) fn pump(&self) {
        let watches = self.watches.borrow().clone();
        let mut fds: Vec<libc::pollfd> =
            watches.iter().map(|w| libc::pollfd { fd: w.fd, events: libc::POLLIN, revents: 0 }).collect();
        if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 0) } > 0 {
            for (watch, fd) in watches.iter().zip(&fds) {
                if fd.revents & libc::POLLNVAL != 0 {
                    // Closed without unregistering; drop it rather than poll it forever.
                    self.watches.borrow_mut().retain(|w| !Rc::ptr_eq(w, watch));
                } else if fd.revents != 0 && self.watches.borrow().iter().any(|w| Rc::ptr_eq(w, watch)) {
                    unsafe { watch.handler.onFDIsSet(watch.fd) };
                }
            }
        }
        let now = Instant::now();
        let timers = self.timers.borrow().clone();
        for timer in timers {
            if timer.next.get() <= now && self.timers.borrow().iter().any(|t| Rc::ptr_eq(t, &timer)) {
                // Missed periods collapse into one call.
                timer.next.set(now + timer.interval);
                unsafe { timer.handler.onTimer() };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;

    use vst3::{Class, ComWrapper};

    use super::*;

    struct Counter {
        calls: Rc<Cell<usize>>,
        on_call: Box<dyn Fn()>,
    }

    impl Class for Counter {
        type Interfaces = (IEventHandler, ITimerHandler);
    }

    impl IEventHandlerTrait for Counter {
        unsafe fn onFDIsSet(&self, _: FileDescriptor) {
            self.calls.set(self.calls.get() + 1);
            (self.on_call)();
        }
    }

    impl ITimerHandlerTrait for Counter {
        unsafe fn onTimer(&self) {
            self.calls.set(self.calls.get() + 1);
            (self.on_call)();
        }
    }

    fn counter(on_call: impl Fn() + 'static) -> (ComWrapper<Counter>, Rc<Cell<usize>>) {
        let calls = Rc::new(Cell::new(0));
        (ComWrapper::new(Counter { calls: calls.clone(), on_call: Box::new(on_call) }), calls)
    }

    #[test]
    fn readable_descriptor_calls_its_handler_until_unregistered() {
        let run_loop = RunLoop::default();
        let (handler, calls) = counter(|| {});
        let handler = handler.to_com_ptr::<IEventHandler>().unwrap();
        let (read, mut write) = UnixStream::pair().unwrap();
        assert_eq!(unsafe { run_loop.register_event_handler(handler.as_ptr(), read.as_raw_fd()) }, kResultOk);
        run_loop.pump();
        assert_eq!(calls.get(), 0);
        write.write_all(b"x").unwrap();
        run_loop.pump();
        assert_eq!(calls.get(), 1);
        assert_eq!(unsafe { run_loop.unregister_event_handler(handler.as_ptr()) }, kResultOk);
        run_loop.pump();
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn timer_fires_when_due_and_can_unregister_itself() {
        let run_loop = Rc::new(RunLoop::default());
        let pointer = Rc::new(Cell::new(std::ptr::null_mut()));
        let (handler, calls) = counter({
            let run_loop = Rc::downgrade(&run_loop);
            let pointer = pointer.clone();
            move || unsafe {
                run_loop.upgrade().unwrap().unregister_timer(pointer.get());
            }
        });
        let handler = handler.to_com_ptr::<ITimerHandler>().unwrap();
        pointer.set(handler.as_ptr());
        assert_eq!(unsafe { run_loop.register_timer(handler.as_ptr(), 5) }, kResultOk);
        run_loop.pump();
        assert_eq!(calls.get(), 0);
        std::thread::sleep(Duration::from_millis(10));
        run_loop.pump();
        std::thread::sleep(Duration::from_millis(10));
        run_loop.pump();
        assert_eq!(calls.get(), 1);
        assert!(run_loop.timers.borrow().is_empty());
    }

    #[test]
    fn clear_releases_handlers() {
        let run_loop = RunLoop::default();
        let (handler, calls) = counter(|| {});
        let timer = handler.to_com_ptr::<ITimerHandler>().unwrap();
        unsafe { run_loop.register_timer(timer.as_ptr(), 1) };
        drop(handler);
        run_loop.clear();
        std::thread::sleep(Duration::from_millis(5));
        run_loop.pump();
        assert_eq!(calls.get(), 0);
        assert!(run_loop.timers.borrow().is_empty());
    }
}
