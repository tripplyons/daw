//! VST3 hosting: bundle loading, class enumeration, processing, and editors.

mod host;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod run_loop;

#[cfg(target_os = "linux")]
pub use linux::binary_dir;

use std::collections::HashMap;
use std::ffi::{c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use daw_engine::{Event as EngineEvent, EventKind, Processor, TransportInfo};
use daw_model::{PluginFormat, PluginRef};
#[cfg(target_os = "macos")]
use objc2_core_foundation::{CFBundle, CFRetained, CFString, CFURL};
use vst3::Steinberg::Vst::BusDirections_::{kInput, kOutput};
use vst3::Steinberg::Vst::Event_::EventTypes_::{kNoteOffEvent, kNoteOnEvent};
use vst3::Steinberg::Vst::MediaTypes_::{kAudio, kEvent};
use vst3::Steinberg::Vst::ParameterInfo_::ParameterFlags_::{kCanAutomate, kIsHidden, kIsReadOnly};
use vst3::Steinberg::Vst::ProcessContext_::StatesAndFlags_::{
    kBarPositionValid, kPlaying, kProjectTimeMusicValid, kTempoValid, kTimeSigValid,
};
use vst3::Steinberg::Vst::ProcessModes_::kRealtime;
use vst3::Steinberg::Vst::SymbolicSampleSizes_::kSample32;
use vst3::Steinberg::Vst::{
    AudioBusBuffers, AudioBusBuffers__type0, BusInfo, Event, Event__type0, IAudioProcessor, IAudioProcessorTrait,
    IComponent, IComponentHandler, IComponentTrait, IConnectionPoint, IConnectionPointTrait, IEditController,
    IEditControllerTrait, IEventList, IParameterChanges, NoteOffEvent, NoteOnEvent, ParameterInfo, ProcessContext,
    ProcessData, ProcessSetup, SpeakerArrangement, SpeakerArr,
};
use vst3::Steinberg::{
    FUnknown, IBStream, IPlugView, IPlugViewTrait, IPluginBaseTrait, IPluginFactory, IPluginFactory2,
    IPluginFactory2Trait, IPluginFactoryTrait, PClassInfo, PClassInfo2, TUID, ViewRect, kResultOk, kResultTrue,
};
use vst3::{ComPtr, ComWrapper, Interface};

use crate::window::EditorWindow;
use crate::{Controller, Loaded, ParamInfo, PluginError, PluginInfo, PluginKind, Touch};
use host::{
    ComponentHandler, EventList, HostApplication, MemoryStream, ParameterChanges, PlugFrame, read_string128,
};

/// The window type editors attach to.
#[cfg(target_os = "macos")]
const EDITOR_PLATFORM: vst3::Steinberg::FIDString = vst3::Steinberg::kPlatformTypeNSView;
#[cfg(target_os = "linux")]
const EDITOR_PLATFORM: vst3::Steinberg::FIDString = vst3::Steinberg::kPlatformTypeX11EmbedWindowID;

#[cfg(target_os = "macos")]
type BundleEntry = unsafe extern "C" fn(*mut c_void) -> bool;
type GetPluginFactory = unsafe extern "C" fn() -> *mut IPluginFactory;

/// A loaded VST3 bundle. Modules stay loaded for the life of the process:
/// many plugins crash when their bundle is unloaded and reloaded.
struct Module {
    factory: ComPtr<IPluginFactory>,
    #[cfg(target_os = "macos")]
    _bundle: CFRetained<CFBundle>,
    #[cfg(target_os = "linux")]
    _library: linux::Library,
}

unsafe impl Send for Module {}
unsafe impl Sync for Module {}

fn modules() -> &'static Mutex<HashMap<PathBuf, Arc<Module>>> {
    static MODULES: OnceLock<Mutex<HashMap<PathBuf, Arc<Module>>>> = OnceLock::new();
    MODULES.get_or_init(Default::default)
}

fn host_application() -> &'static ComWrapper<HostApplication> {
    struct Shared(ComWrapper<HostApplication>);
    unsafe impl Send for Shared {}
    unsafe impl Sync for Shared {}
    static HOST: OnceLock<Shared> = OnceLock::new();
    &HOST.get_or_init(|| Shared(ComWrapper::new(HostApplication))).0
}

fn host_context() -> *mut FUnknown {
    host_application()
        .as_com_ref::<vst3::Steinberg::Vst::IHostApplication>()
        .map(|r| r.as_ptr() as *mut FUnknown)
        .unwrap_or(std::ptr::null_mut())
}

/// The folder with the bundle's binaries for this platform.
#[cfg(target_os = "macos")]
pub fn binary_dir(bundle: &Path) -> PathBuf {
    bundle.join("Contents/MacOS")
}

#[cfg(target_os = "macos")]
fn load_module(path: &Path) -> Result<Arc<Module>, PluginError> {
    if let Some(module) = modules().lock().unwrap().get(path) {
        return Ok(module.clone());
    }
    let fail = |message: &str| PluginError::Load(format!("{}: {message}", path.display()));
    let url = CFURL::from_directory_path(path).ok_or_else(|| fail("invalid path"))?;
    let bundle = CFBundle::new(None, Some(&url)).ok_or_else(|| fail("not a bundle"))?;
    let mut error: *mut objc2_core_foundation::CFError = std::ptr::null_mut();
    if !unsafe { bundle.load_executable_and_return_error(&mut error) } {
        let reason = unsafe { error.as_ref() }
            .and_then(|e| e.description())
            .map(|d| d.to_string())
            .unwrap_or_else(|| "could not load the bundle executable".into());
        return Err(fail(&reason));
    }
    let symbol = |name: &'static str| bundle.function_pointer_for_name(Some(&CFString::from_static_str(name)));
    let entry = symbol("bundleEntry");
    if !entry.is_null() {
        let entry: BundleEntry = unsafe { std::mem::transmute(entry) };
        let bundle_ptr = CFRetained::as_ptr(&bundle).as_ptr() as *mut c_void;
        if !unsafe { entry(bundle_ptr) } {
            return Err(fail("bundleEntry returned false"));
        }
    }
    let get_factory = symbol("GetPluginFactory");
    if get_factory.is_null() {
        return Err(fail("no GetPluginFactory export"));
    }
    let get_factory: GetPluginFactory = unsafe { std::mem::transmute(get_factory) };
    let factory = unsafe { ComPtr::from_raw(get_factory()) }.ok_or_else(|| fail("GetPluginFactory returned null"))?;
    let module = Arc::new(Module { factory, _bundle: bundle });
    modules().lock().unwrap().insert(path.to_owned(), module.clone());
    Ok(module)
}

#[cfg(target_os = "linux")]
fn load_module(path: &Path) -> Result<Arc<Module>, PluginError> {
    // Hold the lock while loading so two threads cannot both run ModuleEntry
    // for one library.
    let mut modules = modules().lock().unwrap();
    if let Some(module) = modules.get(path) {
        return Ok(module.clone());
    }
    let fail = |message: &str| PluginError::Load(format!("{}: {message}", path.display()));
    let library = linux::executable_path(path).and_then(|binary| linux::Library::open(&binary)).map_err(|e| fail(&e))?;
    let get_factory = unsafe { library.inner.get::<GetPluginFactory>(b"GetPluginFactory\0") }
        .map_err(|_| fail("no GetPluginFactory export"))?;
    let factory = unsafe { ComPtr::from_raw(get_factory()) }.ok_or_else(|| fail("GetPluginFactory returned null"))?;
    // Fields drop in order, so the factory is released before ModuleExit.
    let module = Arc::new(Module { factory, _library: library });
    modules.insert(path.to_owned(), module.clone());
    Ok(module)
}

fn c_text(bytes: &[c_char]) -> String {
    let end = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
    let bytes: Vec<u8> = bytes[..end].iter().map(|c| c.to_ne_bytes()[0]).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn tuid_hex(tuid: &TUID) -> String {
    tuid.iter().map(|b| format!("{:02X}", b.to_ne_bytes()[0])).collect()
}

fn parse_tuid(hex: &str) -> Option<TUID> {
    if hex.len() != 32 {
        return None;
    }
    let mut out: TUID = [0; 16];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()? as c_char;
    }
    Some(out)
}

const AUDIO_MODULE_CLASS: &str = "Audio Module Class";

/// Enumerate the audio processor classes in a bundle.
pub fn scan_bundle(path: &Path) -> Result<Vec<PluginInfo>, PluginError> {
    let module = load_module(path)?;
    let factory = &module.factory;
    let factory2 = factory.cast::<IPluginFactory2>();
    let vendor = unsafe {
        let mut info = std::mem::zeroed();
        if factory.getFactoryInfo(&mut info) == kResultOk { c_text(&info.vendor) } else { String::new() }
    };
    let mut plugins = Vec::new();
    for index in 0..unsafe { factory.countClasses() } {
        let (cid, category, name, sub_categories, class_vendor) = match &factory2 {
            Some(factory2) => {
                let mut info: PClassInfo2 = unsafe { std::mem::zeroed() };
                if unsafe { factory2.getClassInfo2(index, &mut info) } != kResultOk {
                    continue;
                }
                (info.cid, c_text(&info.category), c_text(&info.name), c_text(&info.subCategories), c_text(&info.vendor))
            }
            None => {
                let mut info: PClassInfo = unsafe { std::mem::zeroed() };
                if unsafe { factory.getClassInfo(index, &mut info) } != kResultOk {
                    continue;
                }
                (info.cid, c_text(&info.category), c_text(&info.name), String::new(), String::new())
            }
        };
        if category != AUDIO_MODULE_CLASS {
            continue;
        }
        let kind = if sub_categories.split('|').any(|c| c == "Instrument") { PluginKind::Instrument } else { PluginKind::Effect };
        plugins.push(PluginInfo {
            plugin: PluginRef {
                format: PluginFormat::Vst3,
                id: tuid_hex(&cid),
                path: path.to_string_lossy().into_owned(),
                name,
                vendor: if class_vendor.is_empty() { vendor.clone() } else { class_vendor },
            },
            kind,
            category: sub_categories,
        });
    }
    Ok(plugins)
}

/// Component, processor, and controller for one plugin instance. Torn down
/// when both the processor and controller sides are dropped.
struct Instance {
    // Rust drops fields in declaration order after Drop::drop. Release child
    // interfaces before their owning controller/component (DPF requires this).
    connections: Option<(ComPtr<IConnectionPoint>, ComPtr<IConnectionPoint>)>,
    processor: ComPtr<IAudioProcessor>,
    controller: Option<ComPtr<IEditController>>,
    component: ComPtr<IComponent>,
    /// True when the controller is a separate object that needs its own terminate.
    separate_controller: bool,
    _module: Arc<Module>,
}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            self.processor.setProcessing(0);
            self.component.setActive(0);
            if let Some((a, b)) = &self.connections {
                a.disconnect(b.as_ptr());
                b.disconnect(a.as_ptr());
            }
            if let Some(controller) = &self.controller {
                controller.setComponentHandler(std::ptr::null_mut());
                if self.separate_controller {
                    controller.terminate();
                }
            }
            self.component.terminate();
        }
    }
}

fn check(call: &'static str, code: i32) -> Result<(), PluginError> {
    if code == kResultOk || code == kResultTrue { Ok(()) } else { Err(PluginError::Call { call, code }) }
}

fn stream(data: Vec<u8>) -> ComWrapper<MemoryStream> {
    ComWrapper::new(MemoryStream::with_data(data))
}

fn stream_ptr(stream: &ComWrapper<MemoryStream>) -> *mut IBStream {
    stream.as_com_ref::<IBStream>().map(|r| r.as_ptr()).unwrap_or(std::ptr::null_mut())
}

/// State layout: u32 little-endian component length, component bytes, controller bytes.
fn split_state(state: &[u8]) -> Option<(&[u8], &[u8])> {
    let length = u32::from_le_bytes(state.get(..4)?.try_into().ok()?) as usize;
    let component = state.get(4..4 + length)?;
    Some((component, &state[4 + length..]))
}

pub fn load(plugin: &PluginRef, state: &[u8], sample_rate: f64, max_block: usize) -> Result<Loaded, PluginError> {
    let module = load_module(Path::new(&plugin.path))?;
    let cid = parse_tuid(&plugin.id).ok_or_else(|| PluginError::Load(format!("bad class id {}", plugin.id)))?;
    let create = |class: &TUID, iid: &vst3::com_scrape_types::Guid| -> *mut c_void {
        let mut object = std::ptr::null_mut();
        unsafe { module.factory.createInstance(class.as_ptr(), iid.as_ptr() as *const c_char, &mut object) };
        object
    };

    let component = unsafe { ComPtr::<IComponent>::from_raw(create(&cid, &IComponent::IID) as *mut IComponent) }
        .ok_or_else(|| PluginError::Load(format!("{} did not create a component", plugin.name)))?;
    check("IComponent::initialize", unsafe { component.initialize(host_context()) })?;
    let processor = component
        .cast::<IAudioProcessor>()
        .ok_or_else(|| PluginError::Load(format!("{} has no audio processor", plugin.name)))?;

    let mut separate_controller = false;
    let controller = match component.cast::<IEditController>() {
        Some(controller) => Some(controller),
        None => {
            let mut controller_id: TUID = [0; 16];
            let found = unsafe { component.getControllerClassId(&mut controller_id) } == kResultOk;
            let controller = found
                .then(|| unsafe {
                    ComPtr::<IEditController>::from_raw(create(&controller_id, &IEditController::IID) as *mut IEditController)
                })
                .flatten();
            if let Some(controller) = &controller {
                check("IEditController::initialize", unsafe { controller.initialize(host_context()) })?;
                separate_controller = true;
            }
            controller
        }
    };
    let connections = match (&controller, separate_controller) {
        (Some(controller), true) => {
            match (component.cast::<IConnectionPoint>(), controller.cast::<IConnectionPoint>()) {
                (Some(a), Some(b)) => unsafe {
                    a.connect(b.as_ptr());
                    b.connect(a.as_ptr());
                    Some((a, b))
                },
                _ => None,
            }
        }
        _ => None,
    };

    let instance = Arc::new(Instance {
        component,
        processor,
        controller,
        separate_controller,
        connections,
        _module: module.clone(),
    });

    if let Some((component_state, controller_state)) = split_state(state) {
        let component_stream = stream(component_state.to_vec());
        unsafe { instance.component.setState(stream_ptr(&component_stream)) };
        if let Some(controller) = &instance.controller {
            unsafe {
                controller.setComponentState(stream_ptr(&stream(component_state.to_vec())));
                if !controller_state.is_empty() {
                    controller.setState(stream_ptr(&stream(controller_state.to_vec())));
                }
            }
        }
    } else if let Some(controller) = &instance.controller {
        // Sync the controller with the component's default state.
        let component_stream = stream(Vec::new());
        if unsafe { instance.component.getState(stream_ptr(&component_stream)) } == kResultOk {
            let data = component_stream.take();
            unsafe { controller.setComponentState(stream_ptr(&stream(data))) };
        }
    }

    let (to_processor, from_editor) = rtrb::RingBuffer::new(4096);
    let handler = ComWrapper::new(ComponentHandler {
        to_processor: Mutex::new(to_processor),
        touches: Mutex::new(Vec::new()),
        restart: Default::default(),
    });
    if let Some(controller) = &instance.controller {
        let handler_ptr = handler.as_com_ref::<IComponentHandler>().map(|r| r.as_ptr()).unwrap_or(std::ptr::null_mut());
        unsafe { controller.setComponentHandler(handler_ptr) };
    }

    let processor = Vst3Processor::new(instance.clone(), from_editor, sample_rate, max_block)?;
    let controller = Vst3Controller { instance, handler, editor: None };
    Ok(Loaded { processor: Box::new(processor), controller: Box::new(controller) })
}

struct Bus {
    buffers: Vec<Box<[f32]>>,
    pointers: Vec<*mut f32>,
}

impl Bus {
    fn new(channels: usize, max_block: usize) -> Self {
        let mut buffers: Vec<Box<[f32]>> = (0..channels).map(|_| vec![0.0; max_block].into_boxed_slice()).collect();
        let pointers = buffers.iter_mut().map(|b| b.as_mut_ptr()).collect();
        Self { buffers, pointers }
    }
}

struct Vst3Processor {
    instance: Arc<Instance>,
    events: ComWrapper<EventList>,
    changes: ComWrapper<ParameterChanges>,
    output_changes: ComWrapper<ParameterChanges>,
    from_editor: rtrb::Consumer<(u32, f32)>,
    inputs: Vec<Bus>,
    outputs: Vec<Bus>,
    input_headers: Vec<AudioBusBuffers>,
    output_headers: Vec<AudioBusBuffers>,
    context: ProcessContext,
    max_block: usize,
}

// The COM pointers and buffer pointers are only used from the audio thread
// once the processor has been handed to the engine.
unsafe impl Send for Vst3Processor {}

fn bus_channels(component: &ComPtr<IComponent>, direction: i32) -> Vec<(usize, bool)> {
    let count = unsafe { component.getBusCount(kAudio as i32, direction) };
    (0..count)
        .map(|index| {
            let mut info: BusInfo = unsafe { std::mem::zeroed() };
            unsafe { component.getBusInfo(kAudio as i32, direction, index, &mut info) };
            (info.channelCount.max(0) as usize, index == 0)
        })
        .collect()
}

impl Vst3Processor {
    fn new(
        instance: Arc<Instance>,
        from_editor: rtrb::Consumer<(u32, f32)>,
        sample_rate: f64,
        max_block: usize,
    ) -> Result<Self, PluginError> {
        let component = &instance.component;
        let processor = &instance.processor;

        // Ask for stereo on every audio bus; plugins may refuse and keep their own layout.
        let input_buses = bus_channels(component, kInput as i32);
        let output_buses = bus_channels(component, kOutput as i32);
        let mut input_arrangement: Vec<SpeakerArrangement> = input_buses.iter().map(|_| SpeakerArr::kStereo).collect();
        let mut output_arrangement: Vec<SpeakerArrangement> = output_buses.iter().map(|_| SpeakerArr::kStereo).collect();
        unsafe {
            processor.setBusArrangements(
                input_arrangement.as_mut_ptr(),
                input_arrangement.len() as i32,
                output_arrangement.as_mut_ptr(),
                output_arrangement.len() as i32,
            );
        }
        // Re-read channel counts after negotiation.
        let input_buses = bus_channels(component, kInput as i32);
        let output_buses = bus_channels(component, kOutput as i32);
        unsafe {
            for (index, (_, main)) in input_buses.iter().enumerate() {
                component.activateBus(kAudio as i32, kInput as i32, index as i32, u8::from(*main || index == 1));
            }
            for (index, (_, main)) in output_buses.iter().enumerate() {
                component.activateBus(kAudio as i32, kOutput as i32, index as i32, u8::from(*main));
            }
            if component.getBusCount(kEvent as i32, kInput as i32) > 0 {
                component.activateBus(kEvent as i32, kInput as i32, 0, 1);
            }
        }

        let mut setup = ProcessSetup {
            processMode: kRealtime as i32,
            symbolicSampleSize: kSample32 as i32,
            maxSamplesPerBlock: max_block as i32,
            sampleRate: sample_rate,
        };
        check("IAudioProcessor::setupProcessing", unsafe { processor.setupProcessing(&mut setup) })?;
        check("IComponent::setActive", unsafe { component.setActive(1) })?;
        unsafe { processor.setProcessing(1) };

        let mut inputs: Vec<Bus> = input_buses.iter().map(|(c, _)| Bus::new(*c, max_block)).collect();
        let mut outputs: Vec<Bus> = output_buses.iter().map(|(c, _)| Bus::new(*c, max_block)).collect();
        let header = |bus: &mut Bus| AudioBusBuffers {
            numChannels: bus.pointers.len() as i32,
            silenceFlags: 0,
            __field0: AudioBusBuffers__type0 { channelBuffers32: bus.pointers.as_mut_ptr() },
        };
        let input_headers = inputs.iter_mut().map(header).collect();
        let output_headers = outputs.iter_mut().map(header).collect();

        Ok(Self {
            instance,
            events: ComWrapper::new(EventList::new()),
            changes: ComWrapper::new(ParameterChanges::new()),
            output_changes: ComWrapper::new(ParameterChanges::new()),
            from_editor,
            inputs,
            outputs,
            input_headers,
            output_headers,
            context: unsafe { std::mem::zeroed() },
            max_block,
        })
    }

    fn fill_context(&mut self, transport: &TransportInfo) {
        let context = &mut self.context;
        context.state = kTempoValid | kTimeSigValid | kProjectTimeMusicValid | kBarPositionValid;
        if transport.playing {
            context.state |= kPlaying;
        }
        context.sampleRate = transport.sample_rate;
        context.projectTimeSamples = transport.frames;
        context.continousTimeSamples = transport.frames;
        context.projectTimeMusic = transport.beats;
        context.barPositionMusic = transport.bar_start;
        context.tempo = transport.bpm;
        context.timeSigNumerator = transport.numerator as i32;
        context.timeSigDenominator = transport.denominator as i32;
    }
}

impl Processor for Vst3Processor {
    fn process(&mut self, transport: &TransportInfo, events: &[EngineEvent], left: &mut [f32], right: &mut [f32]) {
        self.process_sidechain(transport, events, left, right, (&[], &[]));
    }

    fn process_sidechain(&mut self, transport: &TransportInfo, events: &[EngineEvent], left: &mut [f32], right: &mut [f32], side: (&[f32], &[f32])) {
        let frames = left.len().min(self.max_block);
        self.fill_context(transport);
        unsafe { self.events.clear() };
        self.changes.clear();
        self.output_changes.clear();
        while let Ok((id, value)) = self.from_editor.pop() {
            self.changes.add(id, 0, f64::from(value));
        }
        for event in events {
            let offset = event.offset as i32;
            let (kind, body) = match event.kind {
                EventKind::NoteOn { key, velocity } => (
                    kNoteOnEvent,
                    Event__type0 {
                        noteOn: NoteOnEvent {
                            channel: 0,
                            pitch: i16::from(key),
                            tuning: 0.0,
                            velocity,
                            length: 0,
                            noteId: -1,
                        },
                    },
                ),
                EventKind::NoteOff { key } => (
                    kNoteOffEvent,
                    Event__type0 {
                        noteOff: NoteOffEvent { channel: 0, pitch: i16::from(key), velocity: 0.0, noteId: -1, tuning: 0.0 },
                    },
                ),
                EventKind::Param { id, value } => {
                    self.changes.add(id, offset, f64::from(value));
                    continue;
                }
            };
            unsafe {
                self.events.push(Event {
                    busIndex: 0,
                    sampleOffset: offset,
                    ppqPosition: 0.0,
                    flags: 0,
                    r#type: kind as u16,
                    __field0: body,
                })
            };
        }

        if let Some(bus) = self.inputs.first_mut() {
            for (channel, buffer) in bus.buffers.iter_mut().enumerate() {
                let source: &[f32] = if channel % 2 == 0 { left } else { right };
                buffer[..frames].copy_from_slice(&source[..frames]);
            }
        }
        for (index, bus) in self.inputs.iter_mut().enumerate().skip(1) {
            for (channel, buffer) in bus.buffers.iter_mut().enumerate() {
                let source = if channel % 2 == 0 { side.0 } else { side.1 };
                if index == 1 && source.len() >= frames { buffer[..frames].copy_from_slice(&source[..frames]); }
                else { buffer[..frames].fill(0.0); }
            }
        }
        for bus in &mut self.outputs {
            bus.buffers.iter_mut().for_each(|b| b[..frames].fill(0.0));
        }

        let mut data = ProcessData {
            processMode: kRealtime as i32,
            symbolicSampleSize: kSample32 as i32,
            numSamples: frames as i32,
            numInputs: self.input_headers.len() as i32,
            numOutputs: self.output_headers.len() as i32,
            inputs: self.input_headers.as_mut_ptr(),
            outputs: self.output_headers.as_mut_ptr(),
            inputParameterChanges: self.changes.as_com_ref::<IParameterChanges>().unwrap().as_ptr(),
            outputParameterChanges: self.output_changes.as_com_ref::<IParameterChanges>().unwrap().as_ptr(),
            inputEvents: self.events.as_com_ref::<IEventList>().unwrap().as_ptr(),
            outputEvents: std::ptr::null_mut(),
            processContext: &mut self.context,
        };
        unsafe { self.instance.processor.process(&mut data) };

        match self.outputs.first() {
            Some(bus) if !bus.buffers.is_empty() => {
                left[..frames].copy_from_slice(&bus.buffers[0][..frames]);
                right[..frames].copy_from_slice(&bus.buffers[bus.buffers.len().min(2) - 1][..frames]);
            }
            _ => {
                left.fill(0.0);
                right.fill(0.0);
            }
        }
    }

    fn reset(&mut self) {
        // Toggling processing is the VST3 way to flush voices and tails.
        unsafe {
            self.instance.processor.setProcessing(0);
            self.instance.processor.setProcessing(1);
        }
    }
}

struct Editor {
    view: ComPtr<IPlugView>,
    /// Kept alive while the view holds it. Linux also pumps its run loop.
    #[cfg_attr(target_os = "macos", expect(dead_code))]
    frame: ComWrapper<PlugFrame>,
    window: EditorWindow,
}

struct Vst3Controller {
    instance: Arc<Instance>,
    handler: ComWrapper<ComponentHandler>,
    editor: Option<Editor>,
}

impl Vst3Controller {
    fn controller(&self) -> Option<&ComPtr<IEditController>> {
        self.instance.controller.as_ref()
    }
}

impl Controller for Vst3Controller {
    fn params(&self) -> Vec<ParamInfo> {
        let Some(controller) = self.controller() else { return Vec::new() };
        let count = unsafe { controller.getParameterCount() };
        (0..count)
            .filter_map(|index| {
                let mut info: ParameterInfo = unsafe { std::mem::zeroed() };
                if unsafe { controller.getParameterInfo(index, &mut info) } != kResultOk {
                    return None;
                }
                if info.flags & kIsHidden != 0 {
                    return None;
                }
                Some(ParamInfo {
                    id: info.id,
                    name: read_string128(&info.title),
                    units: read_string128(&info.units),
                    steps: info.stepCount.max(0) as u32,
                    default: info.defaultNormalizedValue as f32,
                    automatable: info.flags & kCanAutomate != 0 && info.flags & kIsReadOnly == 0,
                })
            })
            .collect()
    }

    fn param_value(&self, id: u32) -> f32 {
        self.controller().map(|c| unsafe { c.getParamNormalized(id) } as f32).unwrap_or(0.0)
    }

    fn set_param(&mut self, id: u32, value: f32) {
        if let Some(controller) = self.controller() {
            unsafe { controller.setParamNormalized(id, f64::from(value)) };
        }
    }

    fn param_text(&self, id: u32, value: f32) -> String {
        let Some(controller) = self.controller() else { return format!("{value:.3}") };
        let mut text = [0; 128];
        if unsafe { controller.getParamStringByValue(id, f64::from(value), &mut text) } == kResultOk {
            read_string128(&text)
        } else {
            format!("{value:.3}")
        }
    }

    fn save_state(&self) -> Result<Vec<u8>, PluginError> {
        let component_stream = stream(Vec::new());
        check("IComponent::getState", unsafe { self.instance.component.getState(stream_ptr(&component_stream)) })?;
        let component_state = component_stream.take();
        let mut state = (component_state.len() as u32).to_le_bytes().to_vec();
        state.extend_from_slice(&component_state);
        if let Some(controller) = self.controller() {
            let controller_stream = stream(Vec::new());
            if unsafe { controller.getState(stream_ptr(&controller_stream)) } == kResultOk {
                state.extend_from_slice(&controller_stream.take());
            }
        }
        Ok(state)
    }

    fn has_editor(&self) -> bool {
        self.controller().is_some()
    }

    fn open_editor(&mut self, title: &str) -> Result<(), PluginError> {
        if let Some(editor) = &self.editor {
            editor.window.show();
            return Ok(());
        }
        let controller = self.controller().ok_or_else(|| PluginError::Load("no edit controller".into()))?;
        let view = unsafe { controller.createView(c"editor".as_ptr()) };
        let view = unsafe { ComPtr::<IPlugView>::from_raw(view) }
            .ok_or_else(|| PluginError::Load(format!("{title} has no editor")))?;
        if unsafe { view.isPlatformTypeSupported(EDITOR_PLATFORM) } != kResultTrue {
            return Err(PluginError::Load(format!("{title} has no editor for this window system")));
        }
        let mut rect = ViewRect { left: 0, top: 0, right: 400, bottom: 300 };
        unsafe { view.getSize(&mut rect) };
        let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
        #[cfg(target_os = "macos")]
        let window = EditorWindow::new(title, f64::from(width), f64::from(height));
        #[cfg(target_os = "linux")]
        let window = EditorWindow::new(title, width, height, unsafe { view.canResize() } == kResultTrue)?;
        let frame = ComWrapper::new(PlugFrame {
            resize: Mutex::new(Some(Box::new(window.resizer()))),
            #[cfg(target_os = "linux")]
            run_loop: Default::default(),
        });
        unsafe {
            view.setFrame(frame.as_com_ref::<vst3::Steinberg::IPlugFrame>().unwrap().as_ptr());
            if let Err(error) = check("IPlugView::attached", view.attached(window.content_view(), EDITOR_PLATFORM)) {
                view.setFrame(std::ptr::null_mut());
                #[cfg(target_os = "linux")]
                frame.run_loop.clear();
                window.close();
                return Err(error);
            }
        }
        window.show();
        self.editor = Some(Editor { view, frame, window });
        Ok(())
    }

    fn hide_editor(&mut self) {
        if let Some(editor) = &self.editor {
            editor.window.hide();
        }
    }

    fn editor_open(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| e.window.is_visible())
    }

    fn take_touches(&mut self) -> Vec<Touch> {
        // Linux has no shared event loop, so drive the editor from here.
        #[cfg(target_os = "linux")]
        if let Some(editor) = &self.editor {
            editor.frame.run_loop.pump();
            // The user resized the window: let the plugin adjust the size, then follow it.
            if let Some((width, height)) = editor.window.poll() {
                let mut rect = ViewRect { left: 0, top: 0, right: width, bottom: height };
                unsafe { editor.view.checkSizeConstraint(&mut rect) };
                let fitted = (rect.right - rect.left, rect.bottom - rect.top);
                if fitted != (width, height) {
                    editor.window.resize(fitted.0, fitted.1);
                }
                unsafe { editor.view.onSize(&mut rect) };
            }
        }
        std::mem::take(&mut *self.handler.touches.lock().unwrap())
    }
}

impl Drop for Vst3Controller {
    fn drop(&mut self) {
        if let Some(editor) = self.editor.take() {
            unsafe {
                editor.view.removed();
                editor.view.setFrame(std::ptr::null_mut());
            }
            #[cfg(target_os = "linux")]
            editor.frame.run_loop.clear();
            editor.window.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tuid_hex_round_trip() {
        let tuid: TUID = [1, (-2i8) as c_char, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        assert_eq!(parse_tuid(&tuid_hex(&tuid)), Some(tuid));
    }

    #[test]
    fn state_split() {
        let mut state = 3u32.to_le_bytes().to_vec();
        state.extend_from_slice(&[1, 2, 3, 4, 5]);
        assert_eq!(split_state(&state), Some((&[1u8, 2, 3][..], &[4u8, 5][..])));
        assert_eq!(split_state(&[]), None);
    }
}
