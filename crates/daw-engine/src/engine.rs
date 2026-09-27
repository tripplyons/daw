//! The realtime engine. `Engine::render` runs on the audio thread; the UI talks
//! to it only through `EngineHandle`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use daw_model::automation::tempo_from_normalized;
use daw_model::time::{TICKS_PER_BEAT, Ticks};

use crate::processor::{Event, EventKind, Processor, TransportInfo};
use crate::song::{EngineTarget, InsertPlan, Song};

/// Largest block the engine renders at once; larger host buffers are split.
pub const MAX_BLOCK: usize = 512;
/// Automation is evaluated once per sub-block.
const SUB_BLOCK: usize = 128;
const MAX_NODES: usize = 4096;
const EVENT_CAPACITY: usize = 2048;
pub const MAX_METERS: usize = 256;

pub struct Node {
    pub key: u64,
    processor: Box<dyn Processor>,
    events: Vec<Event>,
    /// Keys with a note-on sent and no note-off yet.
    held: u128,
}

impl Node {
    /// Allocates the event buffer here, off the audio thread.
    pub fn new(key: u64, processor: Box<dyn Processor>) -> Box<Node> {
        Box::new(Node { key, processor, events: Vec::with_capacity(EVENT_CAPACITY), held: 0 })
    }

    fn push(&mut self, event: Event) {
        match event.kind {
            EventKind::NoteOn { key, .. } => self.held |= 1 << key,
            EventKind::NoteOff { key } => self.held &= !(1 << key),
            EventKind::Param { .. } => {}
        }
        // Drop events past capacity rather than allocate on the audio thread.
        if self.events.len() < self.events.capacity() {
            self.events.push(event);
        }
    }

    fn release_all(&mut self, offset: u32) {
        let mut held = self.held;
        while held != 0 {
            let key = held.trailing_zeros() as u8;
            held &= held - 1;
            self.push(Event { offset, kind: EventKind::NoteOff { key } });
        }
    }
}

pub enum Command {
    Song(Box<Song>),
    AddNode(Box<Node>),
    RemoveNode(u64),
    Play,
    Stop,
    Seek(f64),
    Param { node: u64, id: u32, value: f32 },
    /// Live note for previews; velocity 0 is note-off.
    Note { node: u64, key: u8, velocity: f32 },
}

/// Objects the audio thread hands back to be dropped on the UI thread.
pub enum Garbage {
    Song(Box<Song>),
    Node(Box<Node>),
}

/// State the UI reads without locks.
pub struct Shared {
    position: AtomicU64,
    bpm: AtomicU64,
    playing: AtomicBool,
    /// Peak per insert and side, as f32 bits; the UI resets them when read.
    peaks: Vec<AtomicU32>,
}

impl Shared {
    pub fn position(&self) -> f64 {
        f64::from_bits(self.position.load(Ordering::Relaxed))
    }

    pub fn bpm(&self) -> f64 {
        f64::from_bits(self.bpm.load(Ordering::Relaxed))
    }

    pub fn playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }

    /// Peak since the last call, for the insert at `index`.
    pub fn take_peak(&self, index: usize) -> (f32, f32) {
        let take = |i: usize| self.peaks.get(i).map(|p| f32::from_bits(p.swap(0, Ordering::Relaxed))).unwrap_or(0.0);
        (take(index * 2), take(index * 2 + 1))
    }

    fn raise_peak(&self, index: usize, left: f32, right: f32) {
        let raise = |i: usize, value: f32| {
            if let Some(peak) = self.peaks.get(i) {
                let _ = peak.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |bits| {
                    (value > f32::from_bits(bits)).then_some(value.to_bits())
                });
            }
        };
        raise(index * 2, left);
        raise(index * 2 + 1, right);
    }
}

pub struct EngineHandle {
    commands: rtrb::Producer<Command>,
    garbage: rtrb::Consumer<Garbage>,
    pub shared: Arc<Shared>,
    pub sample_rate: f64,
}

impl EngineHandle {
    /// Queue a command. Returns it back when the queue is full.
    pub fn send(&mut self, command: Command) -> Result<(), Command> {
        self.collect_garbage();
        self.commands.push(command).map_err(|rtrb::PushError::Full(c)| c)
    }

    pub fn collect_garbage(&mut self) {
        while self.garbage.pop().is_ok() {}
    }
}

pub struct Engine {
    sample_rate: f64,
    commands: rtrb::Consumer<Command>,
    garbage: rtrb::Producer<Garbage>,
    shared: Arc<Shared>,
    song: Option<Box<Song>>,
    // Boxed so nodes move between threads through the queues without copying.
    #[allow(clippy::vec_box)]
    nodes: Vec<Box<Node>>,
    playing: bool,
    /// Transport position in ticks.
    position: f64,
    /// Frames since song start, for plugins that want a sample position.
    frames: i64,
    scratch: [Box<[f32]>; 2],
}

pub fn create(sample_rate: f64) -> (Engine, EngineHandle) {
    let (command_tx, command_rx) = rtrb::RingBuffer::new(4096);
    let (garbage_tx, garbage_rx) = rtrb::RingBuffer::new(4096);
    let shared = Arc::new(Shared {
        position: AtomicU64::new(0f64.to_bits()),
        bpm: AtomicU64::new(0f64.to_bits()),
        playing: AtomicBool::new(false),
        peaks: (0..MAX_METERS * 2).map(|_| AtomicU32::new(0)).collect(),
    });
    let engine = Engine {
        sample_rate,
        commands: command_rx,
        garbage: garbage_tx,
        shared: shared.clone(),
        song: None,
        nodes: Vec::with_capacity(MAX_NODES),
        playing: false,
        position: 0.0,
        frames: 0,
        scratch: [vec![0.0; MAX_BLOCK].into_boxed_slice(), vec![0.0; MAX_BLOCK].into_boxed_slice()],
    };
    let handle = EngineHandle { commands: command_tx, garbage: garbage_rx, shared, sample_rate };
    (engine, handle)
}

fn apply_pan(pan: f32, gain: f32) -> (f32, f32) {
    (gain * (1.0 - pan).min(1.0), gain * (1.0 + pan).min(1.0))
}

/// An insert and the insert it sends to, borrowed together. Compiled songs
/// never route an insert into itself.
fn pair_mut(inserts: &mut [InsertPlan], index: usize) -> (&mut InsertPlan, &mut InsertPlan) {
    let output = inserts[index].output;
    if index < output {
        let (low, high) = inserts.split_at_mut(output);
        (&mut low[index], &mut high[0])
    } else {
        let (low, high) = inserts.split_at_mut(index);
        (&mut high[0], &mut low[output])
    }
}

fn peak(buffer: &[f32]) -> f32 {
    buffer.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

impl Engine {
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    fn discard(&mut self, garbage: Garbage) {
        if let Err(rtrb::PushError::Full(garbage)) = self.garbage.push(garbage) {
            // Leaking beats freeing on the audio thread; only happens if the UI stalls.
            std::mem::forget(garbage);
        }
    }

    fn node_mut(&mut self, key: u64) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.key == key).map(|n| &mut **n)
    }

    fn handle_commands(&mut self) {
        while let Ok(command) = self.commands.pop() {
            match command {
                Command::Song(song) => {
                    if let Some(old) = self.song.replace(song) {
                        self.discard(Garbage::Song(old));
                    }
                }
                Command::AddNode(node) => {
                    if let Some(index) = self.nodes.iter().position(|n| n.key == node.key) {
                        let old = std::mem::replace(&mut self.nodes[index], node);
                        self.discard(Garbage::Node(old));
                    } else if self.nodes.len() < self.nodes.capacity() {
                        self.nodes.push(node);
                    } else {
                        self.discard(Garbage::Node(node));
                    }
                }
                Command::RemoveNode(key) => {
                    if let Some(index) = self.nodes.iter().position(|n| n.key == key) {
                        let node = self.nodes.swap_remove(index);
                        self.discard(Garbage::Node(node));
                    }
                }
                Command::Play => self.playing = true,
                Command::Stop => {
                    self.playing = false;
                    self.release_all(0);
                }
                Command::Seek(tick) => {
                    self.position = tick.max(0.0);
                    self.frames = self.ticks_to_frames(self.position);
                    self.release_all(0);
                }
                Command::Param { node, id, value } => {
                    if let Some(node) = self.node_mut(node) {
                        node.push(Event { offset: 0, kind: EventKind::Param { id, value } });
                    }
                }
                Command::Note { node, key, velocity } => {
                    if let Some(node) = self.node_mut(node) {
                        let kind = if velocity > 0.0 { EventKind::NoteOn { key, velocity } } else { EventKind::NoteOff { key } };
                        node.push(Event { offset: 0, kind });
                    }
                }
            }
        }
    }

    fn release_all(&mut self, offset: u32) {
        for node in &mut self.nodes {
            node.release_all(offset);
        }
    }

    fn bpm(&self) -> f64 {
        self.song.as_ref().map(|s| s.bpm).unwrap_or(120.0)
    }

    fn ticks_per_frame(&self) -> f64 {
        self.bpm() / 60.0 * f64::from(TICKS_PER_BEAT) / self.sample_rate
    }

    fn ticks_to_frames(&self, ticks: f64) -> i64 {
        (ticks / self.ticks_per_frame()) as i64
    }

    /// Render stereo output. Buffers may be any length; they are split internally.
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.handle_commands();
        let mut start = 0;
        while start < left.len() {
            let end = (start + SUB_BLOCK).min(left.len());
            self.render_block(&mut left[start..end], &mut right[start..end]);
            start = end;
        }
        self.shared.position.store(self.position.to_bits(), Ordering::Relaxed);
        self.shared.bpm.store(self.bpm().to_bits(), Ordering::Relaxed);
        self.shared.playing.store(self.playing, Ordering::Relaxed);
    }

    fn render_block(&mut self, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len();
        if self.playing {
            self.apply_automation();
            self.schedule_notes(frames);
        }
        let transport = self.transport();
        let Some(mut song) = self.song.take() else {
            left.fill(0.0);
            right.fill(0.0);
            for node in &mut self.nodes {
                node.events.clear();
            }
            return;
        };

        for insert in &mut song.inserts {
            insert.left[..frames].fill(0.0);
            insert.right[..frames].fill(0.0);
        }
        let [scratch_l, scratch_r] = &mut self.scratch;
        let (scratch_l, scratch_r) = (&mut scratch_l[..frames], &mut scratch_r[..frames]);
        for channel in &song.channels {
            let Some(node) = self.nodes.iter_mut().find(|n| n.key == channel.node) else { continue };
            node.processor.process(&transport, &node.events, scratch_l, scratch_r);
            node.events.clear();
            if channel.mute {
                continue;
            }
            let (gl, gr) = apply_pan(channel.pan, channel.volume);
            let Some(insert) = song.inserts.get_mut(channel.insert) else { continue };
            for i in 0..frames {
                insert.left[i] += scratch_l[i] * gl;
                insert.right[i] += scratch_r[i] * gr;
            }
        }

        for &index in &song.order {
            let (insert, output) = pair_mut(&mut song.inserts, index);
            let (l, r) = (&mut insert.left[..frames], &mut insert.right[..frames]);
            for key in &insert.effects {
                if let Some(node) = self.nodes.iter_mut().find(|n| n.key == *key) {
                    node.processor.process(&transport, &node.events, l, r);
                    node.events.clear();
                }
            }
            let (gl, gr) = if insert.silent { (0.0, 0.0) } else { apply_pan(insert.pan, insert.volume) };
            for i in 0..frames {
                l[i] *= gl;
                r[i] *= gr;
                output.left[i] += l[i];
                output.right[i] += r[i];
            }
            self.shared.raise_peak(index, peak(l), peak(r));
        }
        let master = &mut song.inserts[0];
        let (l, r) = (&mut master.left[..frames], &mut master.right[..frames]);
        for key in &master.effects {
            if let Some(node) = self.nodes.iter_mut().find(|n| n.key == *key) {
                node.processor.process(&transport, &node.events, l, r);
                node.events.clear();
            }
        }
        let (gl, gr) = if master.silent { (0.0, 0.0) } else { apply_pan(master.pan, master.volume) };
        for i in 0..frames {
            left[i] = l[i] * gl;
            right[i] = r[i] * gr;
        }
        self.shared.raise_peak(0, peak(left), peak(right));
        // Nodes not routed anywhere still drop their events each block.
        for node in &mut self.nodes {
            node.events.clear();
        }
        self.song = Some(song);

        if self.playing {
            self.advance(frames);
        }
    }

    fn transport(&self) -> TransportInfo {
        let signature = self.song.as_ref().map(|s| s.signature).unwrap_or_default();
        let beats = self.position / f64::from(TICKS_PER_BEAT);
        let bar_beats = 4.0 * f64::from(signature.numerator) / f64::from(signature.denominator);
        TransportInfo {
            sample_rate: self.sample_rate,
            bpm: self.bpm(),
            beats,
            frames: self.frames,
            playing: self.playing,
            numerator: signature.numerator,
            denominator: signature.denominator,
            bar_start: (beats / bar_beats).floor() * bar_beats,
        }
    }

    fn apply_automation(&mut self) {
        let Some(mut song) = self.song.take() else { return };
        let position = self.position;
        for plan in &mut song.automation {
            let Some(value) = plan.value_at(position) else { continue };
            if value == plan.last {
                continue;
            }
            plan.last = value;
            match plan.target {
                EngineTarget::Node { node, param } => {
                    if let Some(node) = self.node_mut(node) {
                        node.push(Event { offset: 0, kind: EventKind::Param { id: param, value } });
                    }
                }
                EngineTarget::InsertVolume(i) => {
                    if let Some(insert) = song.inserts.get_mut(i) {
                        insert.volume = value * 2.0;
                    }
                }
                EngineTarget::InsertPan(i) => {
                    if let Some(insert) = song.inserts.get_mut(i) {
                        insert.pan = value * 2.0 - 1.0;
                    }
                }
                EngineTarget::ChannelVolume(i) => {
                    if let Some(channel) = song.channels.get_mut(i) {
                        channel.volume = value;
                    }
                }
                EngineTarget::ChannelPan(i) => {
                    if let Some(channel) = song.channels.get_mut(i) {
                        channel.pan = value * 2.0 - 1.0;
                    }
                }
                EngineTarget::Tempo => song.bpm = tempo_from_normalized(value),
            }
        }
        self.song = Some(song);
    }

    /// Queue note events that start in this block, wrapping at the loop end.
    fn schedule_notes(&mut self, frames: usize) {
        let per_frame = self.ticks_per_frame();
        let start = self.position;
        let end = start + per_frame * frames as f64;
        let loop_range = self.song.as_ref().and_then(|s| s.loop_range);
        match loop_range {
            Some((loop_start, loop_end)) if end > loop_end as f64 && loop_end > loop_start => {
                let split = ((loop_end as f64 - start) / per_frame).clamp(0.0, frames as f64) as u32;
                self.collect_notes(start, loop_end as f64, 0, per_frame);
                self.release_all(split);
                self.collect_notes(loop_start as f64, loop_start as f64 + (end - loop_end as f64), split, per_frame);
            }
            _ => self.collect_notes(start, end, 0, per_frame),
        }
    }

    /// Queue events in `from..to`, assigning each to its nearest frame. The
    /// window is shifted back half a frame so accumulated float error in the
    /// position cannot push an on-grid event into the previous frame, while
    /// consecutive windows still tile without gaps or overlap.
    fn collect_notes(&mut self, from: f64, to: f64, offset: u32, per_frame: f64) {
        let Some(song) = self.song.take() else { return };
        let (low, high) = (from - per_frame / 2.0, to - per_frame / 2.0);
        let last_frame = ((to - from) / per_frame).round().max(1.0) as u32 - 1;
        for channel in &song.channels {
            let first = channel.events.partition_point(|e| (e.tick as f64) < low);
            let Some(node) = self.nodes.iter_mut().find(|n| n.key == channel.node) else { continue };
            for event in channel.events[first..].iter().take_while(|e| (e.tick as f64) < high) {
                let frame = offset + (((event.tick as f64 - from) / per_frame).round().max(0.0) as u32).min(last_frame);
                let kind = if event.velocity > 0.0 {
                    EventKind::NoteOn { key: event.key, velocity: event.velocity }
                } else {
                    EventKind::NoteOff { key: event.key }
                };
                node.push(Event { offset: frame, kind });
            }
        }
        self.song = Some(song);
    }

    fn advance(&mut self, frames: usize) {
        self.position += self.ticks_per_frame() * frames as f64;
        self.frames += frames as i64;
        if let Some((loop_start, loop_end)) = self.song.as_ref().and_then(|s| s.loop_range)
            && self.position >= loop_end as f64
            && loop_end > loop_start
        {
            self.position = loop_start as f64 + (self.position - loop_end as f64);
            self.frames = self.ticks_to_frames(self.position);
        }
    }

    /// Render `frames` of audio offline, from the current position.
    pub fn render_offline(&mut self, frames: usize, mut sink: impl FnMut(&[f32], &[f32])) {
        let mut left = vec![0.0; MAX_BLOCK];
        let mut right = vec![0.0; MAX_BLOCK];
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(MAX_BLOCK);
            self.render(&mut left[..n], &mut right[..n]);
            sink(&left[..n], &right[..n]);
            done += n;
        }
    }

    /// Set up for an offline render from the song start, bypassing the queue.
    pub fn start_offline(&mut self) {
        self.handle_commands();
        self.release_all(0);
        self.position = 0.0;
        self.frames = 0;
        self.playing = true;
        for node in &mut self.nodes {
            node.processor.reset();
        }
    }

    pub fn stop_offline(&mut self) {
        self.playing = false;
        self.release_all(0);
    }

    pub fn position(&self) -> Ticks {
        self.position as Ticks
    }
}
