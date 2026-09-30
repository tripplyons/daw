//! Audio Unit hosting through `AUAudioUnit`, which covers AUv2 components
//! (bridged by the system) and AUv3 extensions alike.

use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use block2::RcBlock;
use daw_engine::{Event, EventKind, Processor, TransportInfo};
use daw_model::{PluginFormat, PluginRef};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2::{AnyThread, msg_send};
use objc2_app_kit::NSViewController;
use objc2_audio_toolbox::{
    AUAudioUnit, AUEventSampleTimeImmediate, AUHostTransportStateFlags, AUParameter, AUParameterAddress,
    AUParameterAutomationEvent, AUParameterAutomationEventType, AUParameterObserverToken, AUValue, AudioComponentDescription, AudioComponentInstantiationOptions,
    AudioUnitRenderActionFlags,
};
use objc2_avf_audio::{AVAudioFormat, AVAudioUnitComponentManager};
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioTimeStamp, AudioTimeStampFlags};
use objc2_foundation::{
    NSData, NSDictionary, NSError, NSInteger, NSMutableDictionary, NSPropertyListFormat, NSPropertyListSerialization, NSString,
};

use crate::window::EditorWindow;
use crate::{Controller, Loaded, ParamInfo, PluginError, PluginInfo, PluginKind, Touch};

// `requestViewControllerWithCompletionHandler:` is declared in CoreAudioKit.
#[link(name = "CoreAudioKit", kind = "framework")]
unsafe extern "C" {}

const fn four_cc(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}

const INSTRUMENT: u32 = four_cc(b"aumu");
const EFFECT: u32 = four_cc(b"aufx");
const MUSIC_EFFECT: u32 = four_cc(b"aumf");

fn describe(kind: u32) -> AudioComponentDescription {
    AudioComponentDescription {
        componentType: kind,
        componentSubType: 0,
        componentManufacturer: 0,
        componentFlags: 0,
        componentFlagsMask: 0,
    }
}

/// Plugin ids encode the component description as three hex OSTypes, since
/// four-char codes may contain spaces or non-ASCII bytes.
pub fn component_id(description: &AudioComponentDescription) -> String {
    format!(
        "{:08x}-{:08x}-{:08x}",
        description.componentType, description.componentSubType, description.componentManufacturer
    )
}

fn parse_component_id(id: &str) -> Option<AudioComponentDescription> {
    let mut parts = id.split('-').map(|p| u32::from_str_radix(p, 16));
    let description = AudioComponentDescription {
        componentType: parts.next()?.ok()?,
        componentSubType: parts.next()?.ok()?,
        componentManufacturer: parts.next()?.ok()?,
        componentFlags: 0,
        componentFlagsMask: 0,
    };
    parts.next().is_none().then_some(description)
}

/// List installed instrument and effect Audio Units from the system registry.
pub fn list() -> Vec<(PluginInfo, u64)> {
    let manager = unsafe { AVAudioUnitComponentManager::sharedAudioUnitComponentManager() };
    let mut out = Vec::new();
    for (kind, plugin_kind) in
        [(INSTRUMENT, PluginKind::Instrument), (EFFECT, PluginKind::Effect), (MUSIC_EFFECT, PluginKind::Effect)]
    {
        let components = unsafe { manager.componentsMatchingDescription(describe(kind)) };
        for component in components.iter() {
            let description = unsafe { component.audioComponentDescription() };
            let tags: Vec<String> = unsafe { component.allTagNames() }.iter().map(|t| t.to_string()).collect();
            let info = PluginInfo {
                plugin: PluginRef {
                    format: PluginFormat::AudioUnit,
                    id: component_id(&description),
                    path: String::new(),
                    name: unsafe { component.name() }.to_string(),
                    vendor: unsafe { component.manufacturerName() }.to_string(),
                },
                kind: plugin_kind,
                category: tags.join("|"),
            };
            out.push((info, unsafe { component.version() } as u64));
        }
    }
    out
}

/// Spin the main run loop until `done` returns something or the timeout passes.
/// AU instantiation and view requests may complete on the main queue, so
/// blocking the main thread outright would deadlock.
fn wait_for<T>(timeout: Duration, mut done: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(value) = done() {
            return Some(value);
        }
        if Instant::now() >= deadline {
            return None;
        }
        unsafe {
            objc2_core_foundation::CFRunLoop::run_in_mode(
                objc2_core_foundation::kCFRunLoopDefaultMode,
                0.005,
                true,
            )
        };
    }
}

fn instantiate(description: AudioComponentDescription) -> Result<Retained<AUAudioUnit>, PluginError> {
    type Slot = Arc<Mutex<Option<Result<Retained<AUAudioUnit>, String>>>>;
    let slot: Slot = Arc::default();
    let sink = slot.clone();
    let handler = RcBlock::new(move |unit: *mut AUAudioUnit, error: *mut NSError| {
        let result = match unsafe { Retained::retain(unit) } {
            Some(unit) => Ok(unit),
            None => Err(unsafe { error.as_ref() }
                .map(|e| e.localizedDescription().to_string())
                .unwrap_or_else(|| "instantiation failed".into())),
        };
        *sink.lock().unwrap() = Some(result);
    });
    unsafe {
        AUAudioUnit::instantiateWithComponentDescription_options_completionHandler(
            description,
            // Empty options: AUv2 loads in-process, AUv3 in its extension process.
            AudioComponentInstantiationOptions::empty(),
            &handler,
        )
    };
    wait_for(Duration::from_secs(20), || slot.lock().unwrap().take())
        .ok_or_else(|| PluginError::Load("timed out instantiating the audio unit".into()))?
        .map_err(PluginError::Load)
}

#[derive(Clone, Copy)]
struct ParamRange {
    address: AUParameterAddress,
    min: AUValue,
    max: AUValue,
}

impl ParamRange {
    fn to_plain(self, value: f32) -> AUValue {
        self.min + value * (self.max - self.min)
    }

    fn to_normalized(self, value: AUValue) -> f32 {
        if self.max > self.min { ((value - self.min) / (self.max - self.min)).clamp(0.0, 1.0) } else { 0.0 }
    }
}

fn parameters(unit: &AUAudioUnit) -> Vec<Retained<AUParameter>> {
    unsafe { unit.parameterTree() }.map(|tree| unsafe { tree.allParameters() }.to_vec()).unwrap_or_default()
}

/// Transport values the AU reads through host blocks during render.
struct TransportCell(UnsafeCell<TransportInfo>);

// Written and read only on the audio thread, inside `process`.
unsafe impl Sync for TransportCell {}
unsafe impl Send for TransportCell {}

/// Input audio for effects, read by the pull-input block during render.
struct InputCell(UnsafeCell<[*const f32; 4]>);

unsafe impl Sync for InputCell {}
unsafe impl Send for InputCell {}

#[repr(C)]
struct StereoBufferList {
    count: u32,
    buffers: [AudioBuffer; 2],
}

type PullInput = dyn Fn(NonNull<AudioUnitRenderActionFlags>, NonNull<AudioTimeStamp>, u32, isize, NonNull<AudioBufferList>) -> i32;
type Render = dyn Fn(
    NonNull<AudioUnitRenderActionFlags>,
    NonNull<AudioTimeStamp>,
    u32,
    isize,
    NonNull<AudioBufferList>,
    objc2_audio_toolbox::AURenderPullInputBlock,
) -> i32;
type ScheduleMidi = dyn Fn(i64, u8, isize, NonNull<u8>);
type ScheduleParam = dyn Fn(i64, u32, AUParameterAddress, AUValue);

struct AuProcessor {
    unit: Retained<AUAudioUnit>,
    // Retained copies: the unit may replace its blocks (for example when the
    // output device changes) and release the old ones while we still render.
    render: Option<RcBlock<Render>>,
    midi: Option<RcBlock<ScheduleMidi>>,
    schedule_param: Option<RcBlock<ScheduleParam>>,
    pull_input: Option<RcBlock<PullInput>>,
    ranges: Arc<Vec<ParamRange>>,
    transport: Arc<TransportCell>,
    input: Arc<InputCell>,
    buffers: [Box<[f32]>; 2],
    silence: Box<[f32]>,
    sample_time: f64,
    max_block: usize,
}

// The unit and blocks are used from the audio thread only after setup.
unsafe impl Send for AuProcessor {}

impl AuProcessor {
    fn range(&self, address: u32) -> Option<ParamRange> {
        let index = self.ranges.binary_search_by_key(&u64::from(address), |r| r.address).ok()?;
        Some(self.ranges[index])
    }
}

impl Processor for AuProcessor {
    fn process(&mut self, transport: &TransportInfo, events: &[Event], left: &mut [f32], right: &mut [f32]) {
        self.process_sidechain(transport, events, left, right, (&[], &[]));
    }

    fn process_sidechain(&mut self, transport: &TransportInfo, events: &[Event], left: &mut [f32], right: &mut [f32], side: (&[f32], &[f32])) {
        let frames = left.len().min(self.max_block);
        unsafe { *self.transport.0.get() = *transport };
        for event in events {
            let time = AUEventSampleTimeImmediate + i64::from(event.offset);
            match event.kind {
                EventKind::NoteOn { key, velocity } => {
                    if let Some(midi) = &self.midi {
                        let bytes = [0x90, key & 0x7f, ((velocity * 127.0).round() as u8).clamp(1, 127)];
                        midi.call((time, 0, 3, NonNull::from(&bytes[0])));
                    }
                }
                EventKind::NoteOff { key } => {
                    if let Some(midi) = &self.midi {
                        let bytes = [0x80, key & 0x7f, 0];
                        midi.call((time, 0, 3, NonNull::from(&bytes[0])));
                    }
                }
                EventKind::Param { id, value } => {
                    if let (Some(range), Some(schedule)) = (self.range(id), &self.schedule_param) {
                        schedule.call((time, 0, range.address, range.to_plain(value)));
                    }
                }
            }
        }

        let side_l = if side.0.len() >= frames { side.0.as_ptr() } else { self.silence.as_ptr() };
        let side_r = if side.1.len() >= frames { side.1.as_ptr() } else { self.silence.as_ptr() };
        unsafe { *self.input.0.get() = [left.as_ptr(), right.as_ptr(), side_l, side_r] };
        let byte_size = (frames * std::mem::size_of::<f32>()) as u32;
        let [out_l, out_r] = &mut self.buffers;
        let mut list = StereoBufferList {
            count: 2,
            buffers: [
                AudioBuffer { mNumberChannels: 1, mDataByteSize: byte_size, mData: out_l.as_mut_ptr() as *mut c_void },
                AudioBuffer { mNumberChannels: 1, mDataByteSize: byte_size, mData: out_r.as_mut_ptr() as *mut c_void },
            ],
        };
        let mut flags = AudioUnitRenderActionFlags(0);
        let mut timestamp: AudioTimeStamp = unsafe { std::mem::zeroed() };
        timestamp.mSampleTime = self.sample_time;
        timestamp.mFlags = AudioTimeStampFlags::SampleTimeValid;
        let pull = self.pull_input.as_ref().map(RcBlock::as_ptr).unwrap_or(std::ptr::null_mut());
        let status = match &self.render {
            Some(render) => render.call((
                NonNull::from(&mut flags),
                NonNull::from(&mut timestamp),
                frames as u32,
                0,
                NonNull::from(&mut list).cast(),
                pull,
            )),
            None => -1,
        };
        self.sample_time += frames as f64;
        if status != 0 {
            left.fill(0.0);
            right.fill(0.0);
            return;
        }
        // The AU may point the buffers at its own memory.
        for (index, out) in [left, right].into_iter().enumerate() {
            let data = list.buffers[index].mData as *const f32;
            if data.is_null() {
                out.fill(0.0);
            } else {
                out[..frames].copy_from_slice(unsafe { std::slice::from_raw_parts(data, frames) });
            }
        }
    }

    fn reset(&mut self) {
        unsafe { self.unit.reset() };
    }
}

fn set_bus_format(unit: &AUAudioUnit, output: bool, sample_rate: f64) -> Result<(), PluginError> {
    let format = unsafe { AVAudioFormat::initStandardFormatWithSampleRate_channels(AVAudioFormat::alloc(), sample_rate, 2) }
        .ok_or_else(|| PluginError::Load("could not create a stereo format".into()))?;
    let busses = unsafe { if output { unit.outputBusses() } else { unit.inputBusses() } };
    for index in 0..unsafe { busses.count() } {
        let bus = unsafe { busses.objectAtIndexedSubscript(index) };
        // `setFormat:error:` takes an AVAudioFormat, which the AudioToolbox bindings omit.
        let result: Result<(), Retained<NSError>> = unsafe { msg_send![&*bus, setFormat: &*format, error: _] };
        // Bridged v2 effects report "no connection" unless the main bus is enabled.
        let _: () = unsafe { msg_send![&*bus, setEnabled: index == 0 || (!output && index == 1)] };
        if let Err(error) = result
            && index == 0 {
                return Err(PluginError::Load(format!("bus format rejected: {}", error.localizedDescription())));
            }
    }
    Ok(())
}

fn install_host_blocks(unit: &AUAudioUnit, transport: &Arc<TransportCell>) {
    let context = transport.clone();
    let musical = RcBlock::new(
        move |tempo: *mut f64,
              numerator: *mut f64,
              denominator: *mut isize,
              beat: *mut f64,
              to_next_beat: *mut isize,
              downbeat: *mut f64|
              -> Bool {
            let t = unsafe { &*context.0.get() };
            unsafe {
                if let Some(v) = tempo.as_mut() {
                    *v = t.bpm;
                }
                if let Some(v) = numerator.as_mut() {
                    *v = f64::from(t.numerator);
                }
                if let Some(v) = denominator.as_mut() {
                    *v = t.denominator as isize;
                }
                if let Some(v) = beat.as_mut() {
                    *v = t.beats;
                }
                if let Some(v) = to_next_beat.as_mut() {
                    let frames_per_beat = t.sample_rate * 60.0 / t.bpm.max(1.0);
                    *v = ((1.0 - t.beats.fract()) * frames_per_beat) as isize;
                }
                if let Some(v) = downbeat.as_mut() {
                    *v = t.bar_start;
                }
            }
            Bool::YES
        },
    );
    let state = transport.clone();
    let transport_block = RcBlock::new(
        move |flags: *mut AUHostTransportStateFlags, position: *mut f64, cycle_start: *mut f64, cycle_end: *mut f64| -> Bool {
            let t = unsafe { &*state.0.get() };
            unsafe {
                if let Some(v) = flags.as_mut() {
                    *v = if t.playing { AUHostTransportStateFlags::Moving } else { AUHostTransportStateFlags::empty() };
                }
                if let Some(v) = position.as_mut() {
                    *v = t.frames as f64;
                }
                if let Some(v) = cycle_start.as_mut() {
                    *v = 0.0;
                }
                if let Some(v) = cycle_end.as_mut() {
                    *v = 0.0;
                }
            }
            Bool::YES
        },
    );
    // The properties copy the blocks, so the local RcBlocks can drop.
    unsafe {
        unit.setMusicalContextBlock(RcBlock::as_ptr(&musical));
        unit.setTransportStateBlock(RcBlock::as_ptr(&transport_block));
    }
}

/// Replace the `jucePluginState` entry of a saved JUCE Audio Unit state. That
/// entry holds the plugin's own state format, e.g. a Vital preset's JSON.
pub fn replace_juce_state(state: &[u8], juce: &[u8]) -> Result<Vec<u8>, PluginError> {
    let bad = |what: &str| PluginError::Load(format!("saved AU state {what}"));
    let plist = unsafe {
        NSPropertyListSerialization::propertyListWithData_options_format_error(
            &NSData::with_bytes(state),
            objc2_foundation::NSPropertyListMutabilityOptions::MutableContainers,
            std::ptr::null_mut(),
        )
    }
    .map_err(|e| bad(&format!("could not be decoded: {}", e.localizedDescription())))?;
    let dictionary = plist.downcast::<NSMutableDictionary>().map_err(|_| bad("is not a dictionary"))?;
    let key = NSString::from_str("jucePluginState");
    if dictionary.objectForKey(&key).is_none() {
        return Err(PluginError::Load("this plugin does not store JUCE plugin state".into()));
    }
    unsafe { dictionary.setObject_forKey(&NSData::with_bytes(juce), ProtocolObject::from_ref(&*key)) };
    let data = unsafe {
        NSPropertyListSerialization::dataWithPropertyList_format_options_error(
            &dictionary,
            NSPropertyListFormat::BinaryFormat_v1_0,
            0,
        )
    }
    .map_err(|e| bad(&format!("could not be encoded: {}", e.localizedDescription())))?;
    Ok(data.to_vec())
}

fn restore_state(unit: &AUAudioUnit, state: &[u8]) -> Result<(), PluginError> {
    let data = NSData::with_bytes(state);
    let plist = unsafe {
        NSPropertyListSerialization::propertyListWithData_options_format_error(
            &data,
            objc2_foundation::NSPropertyListMutabilityOptions::Immutable,
            std::ptr::null_mut(),
        )
    };
    let object = plist.map_err(|error| PluginError::Load(format!("could not decode AU state: {}", error.localizedDescription())))?;
    let dictionary = object.downcast::<NSDictionary>().map_err(|_| PluginError::Load("saved AU state is not a dictionary".into()))?;
    let dictionary: &NSDictionary<NSString, AnyObject> = unsafe { &*(Retained::as_ptr(&dictionary) as *const _) };
    unsafe { unit.setFullState(Some(dictionary)) };
    Ok(())
}

pub fn load(plugin: &PluginRef, state: &[u8], sample_rate: f64, max_block: usize) -> Result<Loaded, PluginError> {
    let description = parse_component_id(&plugin.id).ok_or_else(|| PluginError::NotFound(plugin.id.clone()))?;
    let unit = instantiate(description)?;

    let is_effect = description.componentType != INSTRUMENT;
    set_bus_format(&unit, true, sample_rate)?;
    if is_effect {
        set_bus_format(&unit, false, sample_rate)?;
    }
    // The generated binding passes a u32; the runtime signature takes a u64 on
    // current macOS, which objc2's debug checks reject.
    let _: () = unsafe { msg_send![&*unit, setMaximumFramesToRender: max_block as u64] };
    let transport = Arc::new(TransportCell(UnsafeCell::new(TransportInfo {
        sample_rate,
        bpm: 120.0,
        beats: 0.0,
        frames: 0,
        playing: false,
        numerator: 4,
        denominator: 4,
        bar_start: 0.0,
    })));
    install_host_blocks(&unit, &transport);
    unsafe { unit.allocateRenderResourcesAndReturnError() }
        .map_err(|e| PluginError::Load(format!("allocate render resources: {}", e.localizedDescription())))?;
    // Restore after allocating, as hosts like Logic do: some plugins, such as
    // Dexed, reset their patch while allocating.
    if !state.is_empty() {
        restore_state(&unit, state)?;
    }

    let mut ranges: Vec<ParamRange> = parameters(&unit)
        .iter()
        .map(|p| unsafe { ParamRange { address: p.address(), min: p.minValue(), max: p.maxValue() } })
        .filter(|r| r.address <= u64::from(u32::MAX))
        .collect();
    ranges.sort_by_key(|r| r.address);
    let ranges = Arc::new(ranges);

    let input = Arc::new(InputCell(UnsafeCell::new([std::ptr::null(); 4])));
    let pull_input = is_effect.then(|| {
        let input = input.clone();
        RcBlock::new(
            move |_: NonNull<AudioUnitRenderActionFlags>,
                  _: NonNull<AudioTimeStamp>,
                  frames: u32,
                  bus: isize,
                  list: NonNull<AudioBufferList>|
                  -> i32 {
                let sources = unsafe { *input.0.get() };
                let list = list.as_ptr();
                let count = unsafe { (*list).mNumberBuffers } as usize;
                let buffers = unsafe { std::slice::from_raw_parts_mut((*list).mBuffers.as_mut_ptr(), count) };
                for (index, buffer) in buffers.iter_mut().enumerate() {
                    let source = sources[index.min(1) + if bus == 0 { 0 } else { 2 }];
                    if source.is_null() {
                        continue;
                    }
                    if buffer.mData.is_null() {
                        // Borrow our input directly; the AU only reads it.
                        buffer.mData = source as *mut c_void;
                    } else {
                        unsafe { std::ptr::copy_nonoverlapping(source, buffer.mData as *mut f32, frames as usize) };
                    }
                    buffer.mDataByteSize = frames * std::mem::size_of::<f32>() as u32;
                }
                0
            },
        )
    });

    let processor = AuProcessor {
        render: unsafe { RcBlock::copy(unit.renderBlock()) },
        midi: unsafe { RcBlock::copy(unit.scheduleMIDIEventBlock()) },
        schedule_param: unsafe { RcBlock::copy(unit.scheduleParameterBlock()) },
        unit: unit.clone(),
        pull_input,
        silence: vec![0.0; max_block].into_boxed_slice(),
        ranges: ranges.clone(),
        transport,
        input,
        buffers: [vec![0.0; max_block].into_boxed_slice(), vec![0.0; max_block].into_boxed_slice()],
        sample_time: 0.0,
        max_block,
    };

    let touches: Arc<Mutex<Vec<Touch>>> = Arc::default();
    let observer_touches = touches.clone();
    let observer_ranges = ranges.clone();
    let observer = RcBlock::new(move |count: NSInteger, events: NonNull<AUParameterAutomationEvent>| {
        if count <= 0 { return; }
        let mut touches = observer_touches.lock().unwrap();
        for event in unsafe { std::slice::from_raw_parts(events.as_ptr(), count as usize) } {
            let Ok(index) = observer_ranges.binary_search_by_key(&event.address, |r| r.address) else { continue };
            let param = event.address as u32;
            if event.eventType == AUParameterAutomationEventType::Touch { touches.push(Touch::Begin(param)); }
            if touches.len() < 4096 {
                touches.push(Touch::Value { param, value: observer_ranges[index].to_normalized(event.value) });
            }
            if event.eventType == AUParameterAutomationEventType::Release { touches.push(Touch::End(param)); }
        }
    });
    let token = unsafe { unit.parameterTree() }
        .map(|tree| unsafe { tree.tokenByAddingParameterAutomationObserver(RcBlock::as_ptr(&observer)) });

    let controller = AuController { unit, ranges, touches, token, _observer: observer, editor: None };
    Ok(Loaded { processor: Box::new(processor), controller: Box::new(controller) })
}

struct AuEditor {
    _controller: Retained<NSViewController>,
    window: EditorWindow,
}

struct AuController {
    unit: Retained<AUAudioUnit>,
    ranges: Arc<Vec<ParamRange>>,
    touches: Arc<Mutex<Vec<Touch>>>,
    token: Option<AUParameterObserverToken>,
    _observer: RcBlock<dyn Fn(NSInteger, NonNull<AUParameterAutomationEvent>)>,
    editor: Option<AuEditor>,
}

impl AuController {
    fn parameter(&self, id: u32) -> Option<Retained<AUParameter>> {
        let tree = unsafe { self.unit.parameterTree() }?;
        unsafe { tree.parameterWithAddress(u64::from(id)) }
    }

    fn range(&self, id: u32) -> Option<ParamRange> {
        let index = self.ranges.binary_search_by_key(&u64::from(id), |r| r.address).ok()?;
        Some(self.ranges[index])
    }
}

impl Controller for AuController {
    fn presets(&self) -> Vec<String> {
        let Some(presets) = (unsafe { self.unit.factoryPresets() }) else { return Vec::new() };
        presets.iter().map(|p| unsafe { p.name() }.to_string()).collect()
    }

    fn load_preset(&mut self, index: usize) -> Result<(), PluginError> {
        let presets = unsafe { self.unit.factoryPresets() }.unwrap_or_default();
        if index >= presets.len() {
            return Err(PluginError::Load(format!("no preset {index}; there are {}", presets.len())));
        }
        unsafe { self.unit.setCurrentPreset(Some(&presets.objectAtIndex(index))) };
        Ok(())
    }

    fn params(&self) -> Vec<ParamInfo> {
        parameters(&self.unit)
            .iter()
            .filter(|p| unsafe { p.address() } <= u64::from(u32::MAX))
            .map(|p| unsafe {
                let range = ParamRange { address: p.address(), min: p.minValue(), max: p.maxValue() };
                let steps = p.valueStrings().map(|s| s.count().saturating_sub(1) as u32).unwrap_or(0);
                // kAudioUnitParameterFlag_IsWritable
                let writable = p.flags().0 & (1 << 31) != 0;
                ParamInfo {
                    id: p.address() as u32,
                    name: p.displayName().to_string(),
                    units: p.unitName().map(|u| u.to_string()).unwrap_or_default(),
                    steps,
                    default: range.to_normalized(p.value()),
                    automatable: writable,
                }
            })
            .collect()
    }

    fn param_value(&self, id: u32) -> f32 {
        match (self.parameter(id), self.range(id)) {
            (Some(p), Some(range)) => range.to_normalized(unsafe { p.value() }),
            _ => 0.0,
        }
    }

    fn set_param(&mut self, id: u32, value: f32) {
        if let (Some(p), Some(range)) = (self.parameter(id), self.range(id)) {
            // Pass our observer token so our own edits are not reported as touches.
            match self.token {
                Some(token) => unsafe { p.setValue_originator(range.to_plain(value), token) },
                None => unsafe { p.setValue(range.to_plain(value)) },
            }
        }
    }

    fn param_text(&self, id: u32, value: f32) -> String {
        match (self.parameter(id), self.range(id)) {
            (Some(p), Some(range)) => {
                let plain = range.to_plain(value);
                // Many AUs return nil here, which the generated binding treats as impossible.
                let text: Option<Retained<NSString>> = unsafe { msg_send![&*p, stringFromValue: &plain as *const AUValue] };
                let text = text.map(|t| t.to_string()).filter(|t| !t.is_empty()).unwrap_or_else(|| format!("{plain:.2}"));
                let units = unsafe { p.unitName() }.map(|u| u.to_string()).unwrap_or_default();
                if units.is_empty() { text } else { format!("{text} {units}") }
            }
            _ => format!("{value:.3}"),
        }
    }

    fn save_state(&self) -> Result<Vec<u8>, PluginError> {
        let Some(state) = (unsafe { self.unit.fullState() }) else { return Ok(Vec::new()) };
        let data = unsafe {
            NSPropertyListSerialization::dataWithPropertyList_format_options_error(
                &state,
                NSPropertyListFormat::BinaryFormat_v1_0,
                0,
            )
        }
        .map_err(|e| PluginError::Load(format!("encode AU state: {}", e.localizedDescription())))?;
        Ok(data.to_vec())
    }

    fn restore_state(&mut self, state: &[u8]) -> Result<(), PluginError> {
        restore_state(&self.unit, state)?;
        self.touches.lock().unwrap().clear();
        Ok(())
    }

    fn has_editor(&self) -> bool {
        unsafe { self.unit.providesUserInterface() }
    }

    fn open_editor(&mut self, title: &str) -> Result<(), PluginError> {
        if let Some(editor) = &self.editor {
            editor.window.show();
            return Ok(());
        }
        type Slot = Arc<Mutex<Option<Option<Retained<NSViewController>>>>>;
        let slot: Slot = Arc::default();
        let sink = slot.clone();
        let handler = RcBlock::new(move |controller: *mut NSViewController| {
            *sink.lock().unwrap() = Some(unsafe { Retained::retain(controller) });
        });
        let _: () = unsafe { msg_send![&*self.unit, requestViewControllerWithCompletionHandler: &*handler] };
        let controller = wait_for(Duration::from_secs(10), || slot.lock().unwrap().take())
            .flatten()
            .ok_or_else(|| PluginError::Load(format!("{title} did not provide an editor")))?;
        let view = controller.view();
        let preferred = controller.preferredContentSize();
        let size = if preferred.width > 0.0 && preferred.height > 0.0 { preferred } else { view.frame().size };
        let window = EditorWindow::new(title, size.width, size.height);
        window.set_content_view(&view);
        window.show();
        self.editor = Some(AuEditor { _controller: controller, window });
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
        std::mem::take(&mut *self.touches.lock().unwrap())
    }
}

impl Drop for AuController {
    fn drop(&mut self) {
        if let Some(editor) = self.editor.take() {
            editor.window.close();
        }
        if let (Some(token), Some(tree)) = (self.token, unsafe { self.unit.parameterTree() }) {
            unsafe { tree.removeParameterObserver(token) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_id_round_trip() {
        let description = AudioComponentDescription {
            componentType: four_cc(b"aufx"),
            componentSubType: four_cc(b"Tpe2"),
            componentManufacturer: four_cc(b"AirW"),
            componentFlags: 0,
            componentFlagsMask: 0,
        };
        let parsed = parse_component_id(&component_id(&description)).unwrap();
        assert_eq!(parsed.componentSubType, description.componentSubType);
        assert_eq!(parsed.componentManufacturer, description.componentManufacturer);
        assert!(parse_component_id("zz").is_none());
    }
}
