//! COM objects the host provides to VST3 plugins.

use std::cell::{Cell, UnsafeCell};
use std::collections::HashMap;
use std::ffi::{CStr, c_void};
use std::sync::Mutex;

use vst3::Steinberg::IBStream_::IStreamSeekMode_::{kIBSeekCur, kIBSeekEnd, kIBSeekSet};
use vst3::Steinberg::Vst::{
    Event, IAttributeList, IAttributeListTrait, IComponentHandler, IComponentHandlerTrait, IEventList,
    IEventListTrait, IHostApplication, IHostApplicationTrait, IMessage, IMessageTrait, IParamValueQueue,
    IParamValueQueueTrait, IParameterChanges, IParameterChangesTrait, ParamID, ParamValue, String128, TChar,
};
use vst3::Steinberg::{
    FIDString, IBStream, IBStreamTrait, IPlugFrame, IPlugFrameTrait, IPlugView, IPlugViewTrait, TUID,
    ViewRect, int32, int64, kInvalidArgument, kResultFalse, kResultOk, tresult, uint32,
};
#[cfg(target_os = "linux")]
use vst3::Steinberg::Linux::{FileDescriptor, IEventHandler, IRunLoop, IRunLoopTrait, ITimerHandler, TimerInterval};
use vst3::{Class, ComRef, ComWrapper, Interface};

use crate::Touch;

pub fn write_string128(text: &str, out: &mut String128) {
    let mut length = 0;
    for (src, dst) in text.encode_utf16().zip(out.iter_mut().take(127)) {
        *dst = src as TChar;
        length += 1;
    }
    out[length] = 0;
}

pub fn read_string128(text: &String128) -> String {
    let end = text.iter().position(|&c| c == 0).unwrap_or(text.len());
    String::from_utf16_lossy(&text[..end])
}

fn tuid_is(tuid: &TUID, guid: &vst3::com_scrape_types::Guid) -> bool {
    tuid.iter().zip(guid).all(|(a, b)| a.to_ne_bytes()[0] == *b)
}

pub struct HostApplication;

impl Class for HostApplication {
    type Interfaces = (IHostApplication,);
}

impl IHostApplicationTrait for HostApplication {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        unsafe { write_string128("daw", &mut *name) };
        kResultOk
    }

    unsafe fn createInstance(&self, cid: *mut TUID, iid: *mut TUID, obj: *mut *mut c_void) -> tresult {
        let _ = iid;
        let wanted = unsafe { *cid };
        if tuid_is(&wanted, &IMessage::IID) {
            let wrapper = ComWrapper::new(Message::default());
            if let Some(ptr) = wrapper.to_com_ptr::<IMessage>() {
                unsafe { *obj = ptr.into_raw() as *mut c_void };
                return kResultOk;
            }
        } else if tuid_is(&wanted, &IAttributeList::IID) {
            let wrapper = ComWrapper::new(AttributeList::default());
            if let Some(ptr) = wrapper.to_com_ptr::<IAttributeList>() {
                unsafe { *obj = ptr.into_raw() as *mut c_void };
                return kResultOk;
            }
        }
        unsafe { *obj = std::ptr::null_mut() };
        kResultFalse
    }
}

#[derive(Clone)]
enum Attribute {
    Int(i64),
    Float(f64),
    String(Vec<TChar>),
    Binary(Vec<u8>),
}

#[derive(Default)]
pub struct AttributeList {
    values: Mutex<HashMap<String, Attribute>>,
}

impl Class for AttributeList {
    type Interfaces = (IAttributeList,);
}

fn attr_key(id: *const std::ffi::c_char) -> Option<String> {
    if id.is_null() {
        return None;
    }
    Some(unsafe { CStr::from_ptr(id) }.to_string_lossy().into_owned())
}

impl AttributeList {
    fn set(&self, id: *const std::ffi::c_char, value: Attribute) -> tresult {
        let Some(key) = attr_key(id) else { return kInvalidArgument };
        self.values.lock().unwrap().insert(key, value);
        kResultOk
    }

    fn get(&self, id: *const std::ffi::c_char) -> Option<Attribute> {
        self.values.lock().unwrap().get(&attr_key(id)?).cloned()
    }
}

impl IAttributeListTrait for AttributeList {
    unsafe fn setInt(&self, id: *const std::ffi::c_char, value: int64) -> tresult {
        self.set(id, Attribute::Int(value))
    }

    unsafe fn getInt(&self, id: *const std::ffi::c_char, value: *mut int64) -> tresult {
        match self.get(id) {
            Some(Attribute::Int(v)) => {
                unsafe { *value = v };
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn setFloat(&self, id: *const std::ffi::c_char, value: f64) -> tresult {
        self.set(id, Attribute::Float(value))
    }

    unsafe fn getFloat(&self, id: *const std::ffi::c_char, value: *mut f64) -> tresult {
        match self.get(id) {
            Some(Attribute::Float(v)) => {
                unsafe { *value = v };
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn setString(&self, id: *const std::ffi::c_char, string: *const TChar) -> tresult {
        if string.is_null() {
            return kInvalidArgument;
        }
        let mut text = Vec::new();
        let mut i = 0;
        loop {
            let c = unsafe { *string.add(i) };
            text.push(c);
            if c == 0 {
                break;
            }
            i += 1;
        }
        self.set(id, Attribute::String(text))
    }

    unsafe fn getString(&self, id: *const std::ffi::c_char, string: *mut TChar, size_in_bytes: uint32) -> tresult {
        let Some(Attribute::String(text)) = self.get(id) else { return kResultFalse };
        let capacity = size_in_bytes as usize / std::mem::size_of::<TChar>();
        if capacity == 0 {
            return kInvalidArgument;
        }
        let count = text.len().min(capacity);
        unsafe {
            std::ptr::copy_nonoverlapping(text.as_ptr(), string, count);
            *string.add(count - 1) = 0;
        }
        kResultOk
    }

    unsafe fn setBinary(&self, id: *const std::ffi::c_char, data: *const c_void, size_in_bytes: uint32) -> tresult {
        let bytes = if data.is_null() {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(data as *const u8, size_in_bytes as usize) }.to_vec()
        };
        self.set(id, Attribute::Binary(bytes))
    }

    unsafe fn getBinary(&self, id: *const std::ffi::c_char, data: *mut *const c_void, size_in_bytes: *mut uint32) -> tresult {
        let Some(key) = attr_key(id) else { return kInvalidArgument };
        // Return a pointer into the stored value so it stays valid for the
        // lifetime of the list, as the SDK requires.
        let values = self.values.lock().unwrap();
        let Some(Attribute::Binary(bytes)) = values.get(&key) else { return kResultFalse };
        unsafe {
            *data = bytes.as_ptr() as *const c_void;
            *size_in_bytes = bytes.len() as uint32;
        }
        kResultOk
    }
}

pub struct Message {
    id: Mutex<std::ffi::CString>,
    attributes: ComWrapper<AttributeList>,
}

impl Default for Message {
    fn default() -> Self {
        Self { id: Mutex::new(std::ffi::CString::default()), attributes: ComWrapper::new(AttributeList::default()) }
    }
}

impl Class for Message {
    type Interfaces = (IMessage,);
}

impl IMessageTrait for Message {
    unsafe fn getMessageID(&self) -> FIDString {
        self.id.lock().unwrap().as_ptr()
    }

    unsafe fn setMessageID(&self, id: FIDString) {
        if !id.is_null() {
            *self.id.lock().unwrap() = unsafe { CStr::from_ptr(id) }.to_owned();
        }
    }

    unsafe fn getAttributes(&self) -> *mut IAttributeList {
        // Borrowed pointer: the message owns the list.
        self.attributes.as_com_ref::<IAttributeList>().map(|r| r.as_ptr()).unwrap_or(std::ptr::null_mut())
    }
}

/// Growable in-memory stream for plugin state.
#[derive(Default)]
pub struct MemoryStream {
    data: Mutex<(Vec<u8>, usize)>,
}

impl MemoryStream {
    pub fn with_data(data: Vec<u8>) -> Self {
        Self { data: Mutex::new((data, 0)) }
    }

    pub fn take(&self) -> Vec<u8> {
        std::mem::take(&mut self.data.lock().unwrap().0)
    }
}

impl Class for MemoryStream {
    type Interfaces = (IBStream,);
}

impl IBStreamTrait for MemoryStream {
    unsafe fn read(&self, buffer: *mut c_void, num_bytes: int32, num_bytes_read: *mut int32) -> tresult {
        let mut guard = self.data.lock().unwrap();
        let (data, position) = &mut *guard;
        let count = (num_bytes.max(0) as usize).min(data.len().saturating_sub(*position));
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr().add(*position), buffer as *mut u8, count) };
        *position += count;
        if !num_bytes_read.is_null() {
            unsafe { *num_bytes_read = count as int32 };
        }
        kResultOk
    }

    unsafe fn write(&self, buffer: *mut c_void, num_bytes: int32, num_bytes_written: *mut int32) -> tresult {
        let mut guard = self.data.lock().unwrap();
        let (data, position) = &mut *guard;
        let count = num_bytes.max(0) as usize;
        if data.len() < *position + count {
            data.resize(*position + count, 0);
        }
        unsafe { std::ptr::copy_nonoverlapping(buffer as *const u8, data.as_mut_ptr().add(*position), count) };
        *position += count;
        if !num_bytes_written.is_null() {
            unsafe { *num_bytes_written = count as int32 };
        }
        kResultOk
    }

    unsafe fn seek(&self, pos: int64, mode: int32, result: *mut int64) -> tresult {
        let mut guard = self.data.lock().unwrap();
        let (data, position) = &mut *guard;
        let base = match mode as u32 {
            m if m == kIBSeekSet => 0,
            m if m == kIBSeekCur => *position as i64,
            m if m == kIBSeekEnd => data.len() as i64,
            _ => return kInvalidArgument,
        };
        let target = base + pos;
        if target < 0 {
            return kInvalidArgument;
        }
        *position = target as usize;
        if !result.is_null() {
            unsafe { *result = target };
        }
        kResultOk
    }

    unsafe fn tell(&self, pos: *mut int64) -> tresult {
        if !pos.is_null() {
            unsafe { *pos = self.data.lock().unwrap().1 as int64 };
        }
        kResultOk
    }
}

/// Receives parameter edits from the plugin editor on the main thread.
pub struct ComponentHandler {
    /// Edits forwarded to the audio thread.
    pub to_processor: Mutex<rtrb::Producer<(ParamID, f32)>>,
    pub touches: Mutex<Vec<Touch>>,
    /// Set when the plugin asks for a restart (e.g. latency or param list changed).
    pub restart: Cell<i32>,
}

// Only used from the main thread; the Mutexes guard against misbehaving plugins.
unsafe impl Sync for ComponentHandler {}
unsafe impl Send for ComponentHandler {}

impl Class for ComponentHandler {
    type Interfaces = (IComponentHandler,);
}

impl IComponentHandlerTrait for ComponentHandler {
    unsafe fn beginEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }

    unsafe fn performEdit(&self, id: ParamID, value: ParamValue) -> tresult {
        let _ = self.to_processor.lock().unwrap().push((id, value as f32));
        let mut touches = self.touches.lock().unwrap();
        if touches.len() < 4096 {
            touches.push(Touch { param: id, value: value as f32 });
        }
        kResultOk
    }

    unsafe fn endEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }

    unsafe fn restartComponent(&self, flags: int32) -> tresult {
        self.restart.set(self.restart.get() | flags);
        kResultOk
    }
}

pub type Resizer = Box<dyn FnMut(i32, i32)>;

/// Resizes the editor window when the plugin asks.
pub struct PlugFrame {
    pub resize: Mutex<Option<Resizer>>,
    #[cfg(target_os = "linux")]
    pub run_loop: super::run_loop::RunLoop,
}

#[cfg(target_os = "macos")]
unsafe impl Sync for PlugFrame {}
#[cfg(target_os = "macos")]
unsafe impl Send for PlugFrame {}

impl Class for PlugFrame {
    #[cfg(target_os = "macos")]
    type Interfaces = (IPlugFrame,);
    #[cfg(target_os = "linux")]
    type Interfaces = (IPlugFrame, IRunLoop);
}

impl IPlugFrameTrait for PlugFrame {
    unsafe fn resizeView(&self, view: *mut IPlugView, new_size: *mut ViewRect) -> tresult {
        if view.is_null() || new_size.is_null() {
            return kInvalidArgument;
        }
        let rect = unsafe { *new_size };
        let (width, height) = (rect.right.saturating_sub(rect.left), rect.bottom.saturating_sub(rect.top));
        if width <= 0 || height <= 0 {
            return kInvalidArgument;
        }
        if let Some(resize) = self.resize.lock().unwrap().as_mut() {
            resize(width, height);
        }
        if let Some(view) = unsafe { ComRef::<IPlugView>::from_raw(view) } {
            let mut rect = rect;
            unsafe { view.onSize(&mut rect) };
        }
        kResultOk
    }
}

#[cfg(target_os = "linux")]
impl IRunLoopTrait for PlugFrame {
    unsafe fn registerEventHandler(&self, handler: *mut IEventHandler, fd: FileDescriptor) -> tresult {
        unsafe { self.run_loop.register_event_handler(handler, fd) }
    }

    unsafe fn unregisterEventHandler(&self, handler: *mut IEventHandler) -> tresult {
        unsafe { self.run_loop.unregister_event_handler(handler) }
    }

    unsafe fn registerTimer(&self, handler: *mut ITimerHandler, milliseconds: TimerInterval) -> tresult {
        unsafe { self.run_loop.register_timer(handler, milliseconds) }
    }

    unsafe fn unregisterTimer(&self, handler: *mut ITimerHandler) -> tresult {
        unsafe { self.run_loop.unregister_timer(handler) }
    }
}

const MAX_EVENTS: usize = 1024;

/// Fixed-capacity event list, filled and read on the audio thread.
pub struct EventList {
    events: UnsafeCell<Vec<Event>>,
}

unsafe impl Sync for EventList {}
unsafe impl Send for EventList {}

impl EventList {
    pub fn new() -> Self {
        Self { events: UnsafeCell::new(Vec::with_capacity(MAX_EVENTS)) }
    }

    /// Safety: only the audio thread touches the list, and never while the plugin holds it.
    pub unsafe fn clear(&self) {
        unsafe { (*self.events.get()).clear() };
    }

    pub unsafe fn push(&self, event: Event) {
        let events = unsafe { &mut *self.events.get() };
        if events.len() < events.capacity() {
            events.push(event);
        }
    }
}

impl Class for EventList {
    type Interfaces = (IEventList,);
}

impl IEventListTrait for EventList {
    unsafe fn getEventCount(&self) -> int32 {
        unsafe { (*self.events.get()).len() as int32 }
    }

    unsafe fn getEvent(&self, index: int32, e: *mut Event) -> tresult {
        match unsafe { (&*self.events.get()).get(index as usize) } {
            Some(event) => {
                unsafe { *e = *event };
                kResultOk
            }
            None => kInvalidArgument,
        }
    }

    unsafe fn addEvent(&self, e: *mut Event) -> tresult {
        unsafe { self.push(*e) };
        kResultOk
    }
}

const MAX_POINTS: usize = 64;
const MAX_QUEUES: usize = 256;

pub struct ParamQueue {
    id: Cell<ParamID>,
    points: UnsafeCell<Vec<(int32, ParamValue)>>,
}

unsafe impl Sync for ParamQueue {}
unsafe impl Send for ParamQueue {}

impl Class for ParamQueue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for ParamQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.id.get()
    }

    unsafe fn getPointCount(&self) -> int32 {
        unsafe { (*self.points.get()).len() as int32 }
    }

    unsafe fn getPoint(&self, index: int32, offset: *mut int32, value: *mut ParamValue) -> tresult {
        match unsafe { (&*self.points.get()).get(index as usize) } {
            Some(&(o, v)) => {
                unsafe {
                    *offset = o;
                    *value = v;
                }
                kResultOk
            }
            None => kInvalidArgument,
        }
    }

    unsafe fn addPoint(&self, offset: int32, value: ParamValue, index: *mut int32) -> tresult {
        let points = unsafe { &mut *self.points.get() };
        let position = points.partition_point(|&(o, _)| o < offset);
        if points.get(position).is_some_and(|&(o, _)| o == offset) {
            // UI edits accumulate at offset zero between audio callbacks.
            // VST3 queues have one value per sample: the latest edit wins.
            points[position].1 = value;
        } else {
            if points.len() >= points.capacity() {
                return kResultFalse;
            }
            points.insert(position, (offset, value));
        }
        if !index.is_null() {
            unsafe { *index = position as int32 };
        }
        kResultOk
    }
}

/// Fixed pool of parameter queues. Queues are allocated up front so adding
/// changes on the audio thread never allocates.
pub struct ParameterChanges {
    queues: Vec<ComWrapper<ParamQueue>>,
    pointers: Vec<*mut IParamValueQueue>,
    used: Cell<usize>,
}

unsafe impl Sync for ParameterChanges {}
unsafe impl Send for ParameterChanges {}

impl ParameterChanges {
    pub fn new() -> Self {
        let queues: Vec<_> = (0..MAX_QUEUES)
            .map(|_| ComWrapper::new(ParamQueue { id: Cell::new(0), points: UnsafeCell::new(Vec::with_capacity(MAX_POINTS)) }))
            .collect();
        let pointers = queues.iter().map(|q| q.as_com_ref::<IParamValueQueue>().unwrap().as_ptr()).collect();
        Self { queues, pointers, used: Cell::new(0) }
    }

    pub fn clear(&self) {
        for queue in &self.queues[..self.used.get()] {
            unsafe { (*queue.points.get()).clear() };
        }
        self.used.set(0);
    }

    pub fn add(&self, id: ParamID, offset: i32, value: f64) {
        let queue = self.queue_for(id);
        if let Some(queue) = queue {
            unsafe { self.queues[queue].addPoint(offset, value, std::ptr::null_mut()) };
        }
    }

    fn queue_for(&self, id: ParamID) -> Option<usize> {
        let used = self.used.get();
        if let Some(index) = self.queues[..used].iter().position(|q| q.id.get() == id) {
            return Some(index);
        }
        if used >= self.queues.len() {
            return None;
        }
        self.queues[used].id.set(id);
        self.used.set(used + 1);
        Some(used)
    }
}

impl Class for ParameterChanges {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for ParameterChanges {
    unsafe fn getParameterCount(&self) -> int32 {
        self.used.get() as int32
    }

    unsafe fn getParameterData(&self, index: int32) -> *mut IParamValueQueue {
        if (index as usize) < self.used.get() { self.pointers[index as usize] } else { std::ptr::null_mut() }
    }

    unsafe fn addParameterData(&self, id: *const ParamID, index: *mut int32) -> *mut IParamValueQueue {
        let Some(queue) = self.queue_for(unsafe { *id }) else { return std::ptr::null_mut() };
        if !index.is_null() {
            unsafe { *index = queue as int32 };
        }
        self.pointers[queue]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_edit_at_same_sample_replaces_earlier_value() {
        let queue = ParamQueue { id: Cell::new(6), points: UnsafeCell::new(Vec::with_capacity(2)) };
        let mut index = -1;
        unsafe {
            assert_eq!(queue.addPoint(10, 0.8, &mut index), kResultOk);
            assert_eq!(queue.addPoint(0, 0.5, &mut index), kResultOk);
            // Replacing a point must work even at capacity and must not allocate.
            assert_eq!(queue.addPoint(0, 0.25, &mut index), kResultOk);
            assert_eq!(index, 0);
            assert_eq!(queue.getPointCount(), 2);
            assert_eq!(&*queue.points.get(), &[(0, 0.25), (10, 0.8)]);
            assert_eq!((&*queue.points.get()).capacity(), 2);
            assert_eq!(queue.addPoint(20, 1.0, &mut index), kResultFalse);
        }
    }
}
