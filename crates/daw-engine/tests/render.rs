use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::{Arc, Mutex};

use daw_engine::song::{PlayMode, compile};
use daw_engine::{Command, Engine, EngineHandle, Event, EventKind, Node, Processor, TransportInfo, create};
use daw_model::automation::{Envelope, Point};
use daw_model::time::TICKS_PER_BEAT;
use daw_model::{ClipSource, Note, Project, Target};

struct CountingAllocator;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.with(Cell::get) {
            COUNT.with(|c| c.set(c.get() + 1));
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if COUNTING.with(Cell::get) {
            COUNT.with(|c| c.set(c.get() + 1));
        }
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const SAMPLE_RATE: f64 = 48_000.0;

/// Writes 1.0 at each note-on frame. With `dc` set, outputs the last received
/// parameter value on every frame instead.
struct Probe {
    dc: bool,
    value: f32,
    log: Arc<Mutex<Vec<EventKind>>>,
}

impl Processor for Probe {
    fn process(&mut self, _: &TransportInfo, events: &[Event], left: &mut [f32], right: &mut [f32]) {
        left.fill(0.0);
        for event in events {
            if let EventKind::Param { value, .. } = event.kind {
                self.value = value;
            }
            if let EventKind::NoteOn { .. } = event.kind
                && !self.dc {
                    left[event.offset as usize] = 1.0;
                }
            if let Ok(mut log) = self.log.try_lock()
                && log.len() < log.capacity() {
                    log.push(event.kind);
                }
        }
        if self.dc {
            left.fill(self.value);
        }
        right.copy_from_slice(left);
    }

    fn reset(&mut self) {}
}

/// A project whose gains are all unity so output equals the probe's signal.
fn unity_project() -> Project {
    let mut project = Project::new();
    project.bpm = 120.0;
    for insert in &mut project.mixer.inserts {
        insert.volume = 1.0;
    }
    project.channels[0].volume = 1.0;
    project
}

fn engine_with_probe(project: &Project, mode: PlayMode, dc: bool) -> (Engine, EngineHandle, Arc<Mutex<Vec<EventKind>>>) {
    let (mut engine, mut handle) = create(SAMPLE_RATE);
    let log = Arc::new(Mutex::new(Vec::with_capacity(4096)));
    let key = project.channels[0].id.0;
    let probe = Probe { dc, value: 0.0, log: log.clone() };
    assert!(handle.send(Command::AddNode(Node::new(key, Box::new(probe)))).is_ok());
    assert!(handle.send(Command::Song(Box::new(compile(project, mode, daw_engine::MAX_BLOCK)))).is_ok());
    engine.start_offline();
    (engine, handle, log)
}

fn render(engine: &mut Engine, frames: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(frames);
    engine.render_offline(frames, |left, _| out.extend_from_slice(left));
    out
}

fn onsets(signal: &[f32]) -> Vec<usize> {
    signal.iter().enumerate().filter(|(_, s)| **s > 0.5).map(|(i, _)| i).collect()
}

const FRAMES_PER_BEAT: usize = 24_000;

#[test]
fn notes_land_on_expected_frames() {
    let mut project = unity_project();
    let channel = project.channels[0].id;
    let pattern = project.patterns[0].id;
    let notes = project.pattern_mut(pattern).unwrap().notes_mut(channel);
    for (start, key) in [(0, 60), (TICKS_PER_BEAT as u64, 62), (TICKS_PER_BEAT as u64 * 5 / 2, 64)] {
        notes.push(Note { start, length: 120, key, velocity: 1.0 });
    }
    let (mut engine, _handle, _) = engine_with_probe(&project, PlayMode::Pattern(pattern), false);
    let signal = render(&mut engine, FRAMES_PER_BEAT * 3);
    assert_eq!(onsets(&signal), vec![0, FRAMES_PER_BEAT, FRAMES_PER_BEAT * 5 / 2]);
}

#[test]
fn pattern_mode_loops_at_pattern_length() {
    let mut project = unity_project();
    let channel = project.channels[0].id;
    let pattern = project.patterns[0].id;
    project.pattern_mut(pattern).unwrap().toggle_step(channel, 1);
    let (mut engine, _handle, _) = engine_with_probe(&project, PlayMode::Pattern(pattern), false);
    let bar = FRAMES_PER_BEAT * 4;
    let signal = render(&mut engine, bar * 3);
    let step = FRAMES_PER_BEAT / 4;
    assert_eq!(onsets(&signal), vec![step, bar + step, 2 * bar + step]);
}

#[test]
fn song_mode_places_clips_and_trims_notes() {
    let mut project = unity_project();
    let channel = project.channels[0].id;
    let pattern = project.patterns[0].id;
    project.pattern_mut(pattern).unwrap().toggle_step(channel, 0);
    project.pattern_mut(pattern).unwrap().toggle_step(channel, 8);
    project.add_clip(0, TICKS_PER_BEAT as u64 * 4, ClipSource::Pattern(pattern));
    // Second clip plays only the first half of the pattern.
    let clip = project.add_clip(1, TICKS_PER_BEAT as u64 * 8, ClipSource::Pattern(pattern));
    project.playlist.clips.iter_mut().find(|c| c.id == clip).unwrap().length = TICKS_PER_BEAT as u64;
    let (mut engine, _handle, _) = engine_with_probe(&project, PlayMode::Song, false);
    let signal = render(&mut engine, FRAMES_PER_BEAT * 12);
    let b = FRAMES_PER_BEAT;
    assert_eq!(onsets(&signal), vec![4 * b, 6 * b, 8 * b]);
}

#[test]
fn automation_reaches_expected_values() {
    let mut project = unity_project();
    let channel = project.channels[0].id;
    let automation = project.add_automation("cutoff", Target::SynthCutoff(channel), 0.0);
    let bar = project.signature.ticks_per_bar();
    project.automation_clip_mut(automation).unwrap().envelope =
        Envelope { points: vec![Point::new(0, 0.0), Point::new(bar, 1.0)] };
    project.automation_clip_mut(automation).unwrap().length = bar;
    project.add_clip(0, 0, ClipSource::Automation(automation));
    project.playlist.clips[0].length = bar;
    let (mut engine, _handle, _) = engine_with_probe(&project, PlayMode::Song, true);
    let signal = render(&mut engine, FRAMES_PER_BEAT * 4);
    for (frame, expected) in [(FRAMES_PER_BEAT, 0.25), (FRAMES_PER_BEAT * 2, 0.5), (FRAMES_PER_BEAT * 3 + 100, 0.75)] {
        // Automation updates once per 128-frame sub-block.
        let tolerance = 128.0 / (FRAMES_PER_BEAT * 4) as f32 + 1e-3;
        assert!((signal[frame] - expected).abs() <= tolerance, "frame {frame}: {} vs {expected}", signal[frame]);
    }
}

#[test]
fn loop_wrap_releases_held_notes() {
    let mut project = unity_project();
    let channel = project.channels[0].id;
    let pattern = project.patterns[0].id;
    // A note running past the pattern end must be released at the loop point.
    project.pattern_mut(pattern).unwrap().notes_mut(channel).push(Note {
        start: TICKS_PER_BEAT as u64 * 3,
        length: TICKS_PER_BEAT as u64 * 4,
        key: 70,
        velocity: 1.0,
    });
    let (mut engine, _handle, log) = engine_with_probe(&project, PlayMode::Pattern(pattern), false);
    render(&mut engine, FRAMES_PER_BEAT * 5);
    let log = log.lock().unwrap();
    assert!(log.contains(&EventKind::NoteOff { key: 70 }), "{log:?}");
}

#[test]
fn render_does_not_allocate() {
    let mut project = unity_project();
    let channel = project.channels[0].id;
    let pattern = project.patterns[0].id;
    for step in 0..16 {
        project.pattern_mut(pattern).unwrap().toggle_step(channel, step);
    }
    let (mut engine, _handle, _) = engine_with_probe(&project, PlayMode::Pattern(pattern), false);
    let mut left = vec![0.0; 256];
    let mut right = vec![0.0; 256];
    engine.render(&mut left, &mut right);
    COUNTING.with(|c| c.set(true));
    for _ in 0..2000 {
        engine.render(&mut left, &mut right);
    }
    COUNTING.with(|c| c.set(false));
    assert_eq!(COUNT.with(Cell::get), 0);
}

#[test]
fn builtin_synth_makes_sound() {
    let mut project = unity_project();
    let channel = project.channels[0].id;
    let pattern = project.patterns[0].id;
    project.pattern_mut(pattern).unwrap().toggle_step(channel, 0);
    let (mut engine, mut handle) = create(SAMPLE_RATE);
    let synth = daw_engine::synth::Synth::new(Default::default(), SAMPLE_RATE);
    assert!(handle.send(Command::AddNode(Node::new(channel.0, Box::new(synth)))).is_ok());
    assert!(handle.send(Command::Song(Box::new(compile(&project, PlayMode::Pattern(pattern), 512)))).is_ok());
    engine.start_offline();
    let signal = render(&mut engine, FRAMES_PER_BEAT);
    let peak = signal.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.05, "peak {peak}");
}

#[test]
fn inserts_send_through_their_outputs() {
    let mut project = unity_project();
    let channel = project.channels[0].id;
    let pattern = project.patterns[0].id;
    project.pattern_mut(pattern).unwrap().toggle_step(channel, 0);
    let [source, bus] = [1, 2].map(|i| project.mixer.inserts[i].id);
    project.channels[0].insert = source;
    assert!(project.mixer.set_output(source, bus));
    project.mixer.insert_mut(bus).unwrap().volume = 0.5;
    let peak = |project: &Project| {
        let (mut engine, _handle, _) = engine_with_probe(project, PlayMode::Pattern(pattern), false);
        render(&mut engine, FRAMES_PER_BEAT).iter().fold(0.0f32, |m, s| m.max(*s))
    };
    assert_eq!(peak(&project), 0.5, "the bus gain applies");

    // Soloing the source keeps the bus it feeds.
    project.mixer.insert_mut(source).unwrap().solo = true;
    assert_eq!(peak(&project), 0.5);
    project.mixer.insert_mut(bus).unwrap().mute = true;
    assert_eq!(peak(&project), 0.0);
}
