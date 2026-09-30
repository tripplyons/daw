use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use daw_engine::song::{EngineTarget, PlayMode, compile};
use daw_engine::{Command, Engine, EngineHandle, Event, EventKind, Node, Processor, TransportInfo, create};
use daw_model::automation::{Envelope, Point};
use daw_model::time::TICKS_PER_BEAT;
use daw_engine::synth::Sample;
use daw_model::{ClipSource, Note, Project, Source, Target};

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
    assert!(handle.send(Command::Song(Box::new(compile(project, mode, daw_engine::MAX_BLOCK, &HashMap::new())))).is_ok());
    engine.start_offline(0);
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
fn live_gains_pan_and_sends_change_audio_without_allocating() {
    let mut project = unity_project();
    let insert = project.mixer.inserts[1].id;
    project.channels[0].insert = insert;
    project.mixer.set_send(insert, daw_model::Send { to: daw_model::MASTER, level: 1.0, sidechain: false });
    let (mut engine, mut handle, _) = engine_with_probe(&project, PlayMode::Song, true);
    assert!(handle.send(Command::Param { node: project.channels[0].id.0, id: 0, value: 1.0 }).is_ok());
    engine.render_offline(128, |l, r| assert!(l.iter().chain(r).all(|s| (*s - 2.0).abs() < 1e-6)));
    let commands = [
        Command::SendLevel { from: 1, to: 0, value: 0.0 },
        Command::Mix { target: EngineTarget::ChannelVolume(0), value: 0.5 },
        Command::Mix { target: EngineTarget::InsertVolume(1), value: 0.25 },
        Command::Mix { target: EngineTarget::InsertVolume(0), value: 0.25 },
        Command::Mix { target: EngineTarget::InsertPan(1), value: 1.0 },
    ];
    for command in commands { assert!(handle.send(command).is_ok()); }
    let (mut left, mut right) = ([0.0; 128], [0.0; 128]);
    COUNT.with(|c| c.set(0));
    COUNTING.with(|c| c.set(true));
    engine.render(&mut left, &mut right);
    COUNTING.with(|c| c.set(false));
    assert_eq!(COUNT.with(Cell::get), 0);
    assert!(left.iter().all(|s| *s == 0.0));
    assert!(right.iter().all(|s| (*s - 0.125).abs() < 1e-6));
}

#[test]
fn live_mix_updates_resume_flat_automation() {
    for synth in [false, true] {
        let mut project = unity_project();
        let node = project.channels[0].id.0;
        let (target, live, expected) = if synth {
            (Target::SynthCutoff(project.channels[0].id), EngineTarget::Node { node, param: 0 }, 0.5)
        } else {
            (Target::InsertVolume(daw_model::MASTER), EngineTarget::InsertVolume(0), 1.0)
        };
        let automation = project.add_automation("flat", target, 0.5);
        project.add_clip(0, 0, ClipSource::Automation(automation));
        let (mut engine, mut handle, _) = engine_with_probe(&project, PlayMode::Song, true);
        assert!(handle.send(Command::Param { node, id: 0, value: 1.0 }).is_ok());
        engine.render_offline(128, |l, _| assert!(l.iter().all(|s| (*s - expected).abs() < 1e-6)));
        assert!(handle.send(Command::Mix { target: live, value: 0.0 }).is_ok());
        engine.render_offline(128, |l, _| assert!(l.iter().all(|s| (*s - expected).abs() < 1e-6)));
    }
}

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
fn loop_notes_restart_on_the_frame_nearest_the_loop_end() {
    let mut project = unity_project();
    // A bar lasts 11_520_000 / 131 = 87_938.93 frames, so the loop end falls
    // late in a frame.
    project.bpm = 131.0;
    let channel = project.channels[0].id;
    let pattern = project.patterns[0].id;
    project.pattern_mut(pattern).unwrap().toggle_step(channel, 0);
    let (mut engine, _handle, _) = engine_with_probe(&project, PlayMode::Pattern(pattern), false);
    let signal = render(&mut engine, 180_000);
    assert_eq!(onsets(&signal), vec![0, 87_939, 175_878]);
}

#[test]
fn the_clock_marks_when_each_run_of_playback_is_heard() {
    let project = unity_project();
    let pattern = project.patterns[0].id;
    let (mut engine, handle, _) = engine_with_probe(&project, PlayMode::Pattern(pattern), false);
    let (mut left, mut right) = (vec![0.0; 512], vec![0.0; 512]);
    let heard = |frame: u64| 1_000_000_000 + frame * 1_000_000_000 / 48_000;
    engine.render_live(&mut left, &mut right, heard(0));
    let start = handle.shared.clock().unwrap();
    assert_eq!((start.tick, start.nanos, start.run), (0.0, heard(0), heard(0)));
    // The one-bar pattern loops two seconds in, in the buffer from frame
    // 95_744.
    for buffer in 1..187 {
        engine.render_live(&mut left, &mut right, heard(buffer * 512));
        assert_eq!(handle.shared.clock().unwrap().run, heard(0), "buffer {buffer}");
    }
    engine.render_live(&mut left, &mut right, heard(187 * 512));
    let wrapped = handle.shared.clock().unwrap();
    assert_eq!((wrapped.tick, wrapped.nanos), (0.0, wrapped.run));
    assert!(wrapped.run.abs_diff(heard(96_000)) <= 1, "{} vs {}", wrapped.run, heard(96_000));
}

#[test]
fn notes_follow_tempo_automation() {
    let mut project = unity_project();
    let channel = project.channels[0].id;
    let pattern = project.patterns[0].id;
    let beat = TICKS_PER_BEAT as u64;
    for start in [0, beat * 3] {
        project.pattern_mut(pattern).unwrap().notes_mut(channel).push(Note { start, length: 120, key: 60, velocity: 1.0 });
    }
    project.add_clip(0, 0, ClipSource::Pattern(pattern));
    let tempo = project.add_automation("tempo", Target::Tempo, daw_model::automation::tempo_to_normalized(60.0));
    let clip = project.add_clip(1, beat, ClipSource::Automation(tempo));
    project.playlist.clips.iter_mut().find(|c| c.id == clip).unwrap().length = beat;
    let (mut engine, _handle, _) = engine_with_probe(&project, PlayMode::Song, false);
    let signal = render(&mut engine, FRAMES_PER_BEAT * 6);
    // One beat at 120 bpm, then two at about 60, held after the tempo clip.
    let second = (project.tempo_map().seconds_at(beat as f64 * 3.0) * SAMPLE_RATE).round() as usize;
    assert!(second.abs_diff(FRAMES_PER_BEAT * 5) < 2, "{second}");
    assert_eq!(onsets(&signal), vec![0, second]);
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
fn seeking_past_the_loop_end_wraps_without_playing_the_skipped_notes() {
    let mut project = unity_project();
    let channel = project.channels[0].id;
    let pattern = project.patterns[0].id;
    project.pattern_mut(pattern).unwrap().toggle_step(channel, 0);
    let (mut engine, mut handle, log) = engine_with_probe(&project, PlayMode::Pattern(pattern), false);
    render(&mut engine, 128);
    assert!(handle.send(Command::Seek(project.signature.ticks_per_bar() as f64 * 8.0)).is_ok());
    log.lock().unwrap().clear();
    let signal = render(&mut engine, FRAMES_PER_BEAT * 2);
    assert_eq!(onsets(&signal), vec![0]);
    let log = log.lock().unwrap();
    assert_eq!(log.iter().filter(|e| matches!(e, EventKind::NoteOn { .. })).count(), 1, "{log:?}");
}

#[test]
fn song_edits_release_sounding_notes_they_remove() {
    let mut project = unity_project();
    let channel = project.channels[0].id;
    let pattern = project.patterns[0].id;
    let beat = TICKS_PER_BEAT as u64;
    project.pattern_mut(pattern).unwrap().notes_mut(channel).push(Note { start: 0, length: beat * 4, key: 70, velocity: 1.0 });
    project.add_clip(0, 0, ClipSource::Pattern(pattern));
    let (mut engine, mut handle, log) = engine_with_probe(&project, PlayMode::Song, false);
    render(&mut engine, FRAMES_PER_BEAT);
    let publish = |handle: &mut EngineHandle, project: &Project| {
        assert!(handle.send(Command::Song(Box::new(compile(project, PlayMode::Song, daw_engine::MAX_BLOCK, &HashMap::new())))).is_ok());
    };
    // An edit that keeps the note lets it sound on.
    publish(&mut handle, &project);
    render(&mut engine, 128);
    assert!(!log.lock().unwrap().contains(&EventKind::NoteOff { key: 70 }));
    project.pattern_mut(pattern).unwrap().notes_mut(channel).clear();
    publish(&mut handle, &project);
    render(&mut engine, 128);
    assert!(log.lock().unwrap().contains(&EventKind::NoteOff { key: 70 }));
}

#[test]
fn starting_playback_restores_automated_values_outside_clips() {
    let mut project = unity_project();
    let channel = project.channels[0].id;
    project.channel_mut(channel).unwrap().source = Source::Synth(daw_model::SynthParams { cutoff: 0.25, ..Default::default() });
    let automation = project.add_automation("cutoff", Target::SynthCutoff(channel), 1.0);
    let beat = TICKS_PER_BEAT as u64;
    let clip = project.add_clip(0, beat, ClipSource::Automation(automation));
    project.playlist.clips.iter_mut().find(|c| c.id == clip).unwrap().length = beat;
    let (mut engine, mut handle, _) = engine_with_probe(&project, PlayMode::Song, true);
    let signal = render(&mut engine, FRAMES_PER_BEAT * 3);
    // The project value before the clip, the clip's value in it, and the
    // clip's last value held after it.
    assert_eq!(signal[FRAMES_PER_BEAT / 2], 0.25);
    assert_eq!(signal[FRAMES_PER_BEAT * 3 / 2], 1.0);
    assert_eq!(signal[FRAMES_PER_BEAT * 5 / 2], 1.0);
    for command in [Command::Stop, Command::Seek(beat as f64 * 3.0), Command::Play] {
        assert!(handle.send(command).is_ok());
    }
    assert_eq!(render(&mut engine, 128)[0], 0.25);
}

#[test]
fn render_does_not_allocate() {
    let mut project = unity_project();
    let channel = project.channels[0].id;
    let pattern = project.patterns[0].id;
    for step in 0..16 {
        project.pattern_mut(pattern).unwrap().toggle_step(channel, step);
    }
    let [source, bus, detector] = [1, 2, 3].map(|i| project.mixer.inserts[i].id);
    assert!(project.mixer.set_send(source, daw_model::Send { to: bus, level: 0.5, sidechain: false }));
    assert!(project.mixer.set_send(source, daw_model::Send { to: detector, level: 0.5, sidechain: true }));
    let (mut engine, mut handle, _) = engine_with_probe(&project, PlayMode::Pattern(pattern), false);
    let (mut midi, input) = rtrb::RingBuffer::new(8);
    handle.send(Command::MidiInput(input)).ok().unwrap();
    let mut left = vec![0.0; 256];
    let mut right = vec![0.0; 256];
    engine.render(&mut left, &mut right);
    COUNTING.with(|c| c.set(true));
    for _ in 0..2000 {
        midi.push(daw_engine::engine::LiveNote { node: channel.0, key: 127, velocity: 0.5 }).unwrap();
        midi.push(daw_engine::engine::LiveNote { node: channel.0, key: 127, velocity: 0.0 }).unwrap();
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
    assert!(handle.send(Command::Song(Box::new(compile(&project, PlayMode::Pattern(pattern), 512, &HashMap::new())))).is_ok());
    engine.start_offline(0);
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

/// An effect with a long tail: adds a slowly decaying copy of its loudest input.
struct Ring {
    level: f32,
}

impl Processor for Ring {
    fn process(&mut self, _: &TransportInfo, _: &[Event], left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.level = (self.level * 0.9999).max(l.abs());
            *l += self.level;
            *r += self.level;
        }
    }

    fn reset(&mut self) {
        self.level = 0.0;
    }
}

/// An engine playing a note on step 0 into a `Ring` on the master.
fn engine_with_ring() -> (Engine, EngineHandle) {
    let mut project = unity_project();
    let channel = project.channels[0].id;
    let pattern = project.patterns[0].id;
    project.pattern_mut(pattern).unwrap().toggle_step(channel, 0);
    let effect = project.add_plugin(daw_model::PluginRef {
        format: daw_model::PluginFormat::Vst3,
        id: "ring".into(),
        path: String::new(),
        name: "ring".into(),
        vendor: String::new(),
    });
    project.mixer.inserts[0].effects.push(effect);
    let (mut engine, mut handle, _) = engine_with_probe(&project, PlayMode::Pattern(pattern), false);
    assert!(handle.send(Command::AddNode(Node::new(effect.0, Box::new(Ring { level: 0.0 })))).is_ok());
    let playing = render(&mut engine, 1024);
    assert!(playing[1000] > 0.5, "the tail rings while playing");
    (engine, handle)
}

#[test]
fn stop_cuts_effect_tails() {
    let (mut engine, mut handle) = engine_with_ring();
    assert!(handle.send(Command::Stop).is_ok());
    let stopped = render(&mut engine, 1024);
    assert!(stopped.iter().all(|s| *s == 0.0), "tail after stop: {}", stopped[0]);
}

#[test]
fn seek_cuts_effect_tails_and_keeps_playing() {
    let (mut engine, mut handle) = engine_with_ring();
    assert!(handle.send(Command::Seek(1.0)).is_ok());
    let moved = render(&mut engine, 1024);
    assert!(moved.iter().all(|s| *s == 0.0), "tail after seek: {}", moved[0]);

    assert!(handle.send(Command::Seek(0.0)).is_ok());
    let replayed = render(&mut engine, 1024);
    assert!(replayed[1000] > 0.5, "playback continues from the new position");
}

/// An engine playing `project` in song mode, where every audio channel's
/// file is a rising ramp: frame `i` of the file holds `i / 48000`.
fn engine_with_ramp(project: &Project) -> Engine {
    let ramp: Vec<f32> = (0..48_000).map(|i| i as f32 / 48_000.0).collect();
    let sample = Arc::new(Sample { sample_rate: 24_000.0, left: ramp.clone(), right: ramp });
    let samples: HashMap<String, Arc<Sample>> =
        project.channels.iter().filter_map(|c| match &c.source {
            Source::Audio { path } => Some((path.clone(), sample.clone())),
            _ => None,
        }).collect();
    let (mut engine, mut handle) = create(SAMPLE_RATE);
    assert!(handle.send(Command::Song(Box::new(compile(project, PlayMode::Song, 512, &samples)))).is_ok());
    engine.start_offline(0);
    engine
}

fn audio_project() -> (Project, daw_model::ChannelId) {
    let mut project = unity_project();
    let channel = project.add_channel("take", Source::Audio { path: "ramp.wav".into() });
    project.channel_mut(channel).unwrap().volume = 1.0;
    (project, channel)
}

#[test]
fn audio_clips_play_their_part_of_the_file_at_its_own_rate() {
    let (mut project, channel) = audio_project();
    let beat = TICKS_PER_BEAT as u64;
    // One beat of the file, starting a quarter second (half a beat) in.
    let clip = project.add_audio_clip(0, beat, channel, beat);
    project.playlist.clips.iter_mut().find(|c| c.id == clip).unwrap().offset = beat / 2;
    let mut engine = engine_with_ramp(&project);
    let signal = render(&mut engine, FRAMES_PER_BEAT * 3);
    assert!(signal[..FRAMES_PER_BEAT].iter().all(|s| *s == 0.0), "silent before the clip");
    // The edge fade takes the clip's last frames to silence; rounding in the
    // position may leave one nearly silent frame at the end.
    assert!(signal[FRAMES_PER_BEAT * 2].abs() < 1e-3, "faded out: {}", signal[FRAMES_PER_BEAT * 2]);
    assert!(signal[FRAMES_PER_BEAT * 2 + 1..].iter().all(|s| *s == 0.0), "silent after the clip");
    // The file runs at 24 kHz, so each output frame advances half a file frame.
    for j in [1000, 10_000, 20_000] {
        let expected = (6000.0 + j as f32 / 2.0) / 48_000.0;
        let actual = signal[FRAMES_PER_BEAT + j];
        assert!((actual - expected).abs() < 1e-4, "frame {j}: {actual} vs {expected}");
    }
}

#[test]
fn audio_clips_restart_at_the_loop_start() {
    let (mut project, channel) = audio_project();
    let beat = TICKS_PER_BEAT as u64;
    project.add_audio_clip(0, 0, channel, beat * 2);
    project.playlist.loop_range = Some((0, beat));
    let mut engine = engine_with_ramp(&project);
    let signal = render(&mut engine, FRAMES_PER_BEAT * 2);
    for j in [500, 5000, 20_000] {
        assert_eq!(signal[FRAMES_PER_BEAT + j], signal[j], "frame {j} after the wrap");
    }
}

#[test]
fn audio_wraps_on_the_frame_nearest_the_loop_end() {
    let (mut project, channel) = audio_project();
    // A beat lasts 2_880_000 / 127 = 22_677.17 frames, so the loop end falls
    // early in a frame.
    project.bpm = 127.0;
    let beat = TICKS_PER_BEAT as u64;
    project.add_audio_clip(0, 0, channel, beat * 2);
    project.playlist.loop_range = Some((0, beat));
    let mut engine = engine_with_ramp(&project);
    let signal = render(&mut engine, 23_000);
    // The ramp fades out into the loop end, then restarts faded in from
    // silence.
    assert!(signal[22_600] > 0.2, "before the wrap: {}", signal[22_600]);
    assert!(signal[22_676] < 0.01, "faded out: {}", signal[22_676]);
    assert_eq!(signal[22_677], 0.0, "at the wrap");
}

#[test]
fn audio_fades_in_where_playback_starts_inside_a_clip() {
    let (mut project, channel) = audio_project();
    let beat = TICKS_PER_BEAT as u64;
    project.add_audio_clip(0, 0, channel, beat * 2);
    let mut engine = engine_with_ramp(&project);
    engine.start_offline(beat / 2);
    let signal = render(&mut engine, 1000);
    assert_eq!(signal[0], 0.0);
    // Half a beat is a quarter second, 6000 frames into the 24 kHz file.
    let expected = (6000.0 + 500.0 / 2.0) / 48_000.0;
    assert!((signal[500] - expected).abs() < 1e-4, "{} vs {expected}", signal[500]);
}

#[test]
fn audio_clips_keep_their_own_rate_through_tempo_changes() {
    let (mut project, channel) = audio_project();
    let beat = TICKS_PER_BEAT as u64;
    project.add_audio_clip(0, 0, channel, beat * 4);
    let tempo = project.add_automation("tempo", Target::Tempo, daw_model::automation::tempo_to_normalized(60.0));
    let clip = project.add_clip(1, beat, ClipSource::Automation(tempo));
    project.playlist.clips.iter_mut().find(|c| c.id == clip).unwrap().length = beat;
    let mut engine = engine_with_ramp(&project);
    let signal = render(&mut engine, FRAMES_PER_BEAT * 3);
    // The tempo halves after the first beat, at frame 24_000, and holds
    // after the tempo clip, but the file still plays at half a file frame per
    // output frame.
    for frame in [1000, 23_990, 24_010, 40_000, 71_000] {
        let expected = frame as f32 / 2.0 / 48_000.0;
        assert!((signal[frame] - expected).abs() < 1e-4, "frame {frame}: {} vs {expected}", signal[frame]);
    }
}

#[test]
fn muted_tracks_silence_audio_clips() {
    let (mut project, channel) = audio_project();
    project.add_audio_clip(0, 0, channel, TICKS_PER_BEAT as u64);
    project.playlist.tracks[0].mute = true;
    let mut engine = engine_with_ramp(&project);
    assert!(render(&mut engine, FRAMES_PER_BEAT).iter().all(|s| *s == 0.0));
}

struct Detector(Arc<std::sync::atomic::AtomicU32>);
impl Processor for Detector {
    fn process(&mut self, _: &TransportInfo, _: &[Event], _: &mut [f32], _: &mut [f32]) {}
    fn process_sidechain(&mut self, _: &TransportInfo, _: &[Event], _: &mut [f32], _: &mut [f32], side: (&[f32], &[f32])) {
        for &sample in side.0 { self.0.fetch_max(sample.to_bits(), std::sync::atomic::Ordering::Relaxed); }
    }
    fn reset(&mut self) {}
}

#[test]
fn parallel_sends_sum_but_sidechains_only_reach_the_detector() {
    for destination in [1, 0] {
        let mut project = unity_project();
        let pattern = project.patterns[0].id;
        let channel = project.channels[0].id;
        project.pattern_mut(pattern).unwrap().toggle_step(channel, 0);
        let [bus, source] = [destination, 2].map(|i| project.mixer.inserts[i].id);
        project.channels[0].insert = source;
        let effect = project.add_plugin(daw_model::PluginRef {
            format: daw_model::PluginFormat::Vst3, id: "detector".into(), path: String::new(), name: "detector".into(), vendor: String::new(),
        });
        project.mixer.insert_mut(bus).unwrap().effects.push(effect);
        for sidechain in [false, true] {
            assert!(project.mixer.set_send(source, daw_model::Send { to: bus, level: 0.5, sidechain }));
            let (mut engine, mut handle, _) = engine_with_probe(&project, PlayMode::Pattern(pattern), false);
            let peak = Arc::new(std::sync::atomic::AtomicU32::new(0));
            handle.send(Command::AddNode(Node::new(effect.0, Box::new(Detector(peak.clone()))))).ok().unwrap();
            let signal = render(&mut engine, 512);
            assert_eq!(signal[0], if sidechain { 1.0 } else { 1.5 });
            assert_eq!(f32::from_bits(peak.load(std::sync::atomic::Ordering::Relaxed)), if sidechain { 0.5 } else { 0.0 });
        }
    }
}

#[test]
fn live_midi_queue_plays_without_transport_and_releases_on_disconnect() {
    let project = unity_project();
    let pattern = project.patterns[0].id;
    let (mut engine, mut handle, log) = engine_with_probe(&project, PlayMode::Pattern(pattern), false);
    handle.send(Command::Stop).ok().unwrap();
    let (mut sender, receiver) = rtrb::RingBuffer::new(8);
    handle.send(Command::MidiInput(receiver)).ok().unwrap();
    sender.push(daw_engine::engine::LiveNote { node: project.channels[0].id.0, key: 64, velocity: 0.8 }).unwrap();
    assert_eq!(render(&mut engine, 512)[0], 1.0);
    let (_, replacement) = rtrb::RingBuffer::new(8);
    handle.send(Command::MidiInput(replacement)).ok().unwrap();
    render(&mut engine, 512);
    assert!(log.lock().unwrap().contains(&EventKind::NoteOff { key: 64 }));
}

#[test]
fn muted_individual_clips_do_not_play() {
    let (mut project, channel) = audio_project();
    project.add_audio_clip(0, 0, channel, TICKS_PER_BEAT as u64);
    project.playlist.clips[0].muted = true;
    let mut engine = engine_with_ramp(&project);
    assert!(render(&mut engine, FRAMES_PER_BEAT).iter().all(|s| *s == 0.0));
}
