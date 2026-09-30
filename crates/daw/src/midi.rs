//! Live note input goes straight to the engine; timestamped copies reach the
//! UI for recording. Note-offs keep the instrument chosen at note-on.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}, mpsc};

use daw_engine::{engine::{LiveNote, Shared}, song::{PlayMode, channel_node}};
use daw_model::{ChannelId, ClipSource, Note, PatternId};
use daw_model::time::Ticks;
use midir::{Ignore, MidiInput, MidiInputConnection};

use crate::app::App;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    pub index: Option<usize>,
    pub name: String,
}
impl std::fmt::Display for Port {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.name) }
}

#[derive(Debug, Clone, Copy)]
pub struct Packet {
    pub midi_channel: u8,
    pub channel: ChannelId,
    pub node: u64,
    pub key: u8,
    pub velocity: f32,
    pub tick: f64,
    pub elapsed: f64,
    pub recording: bool,
}

#[derive(Clone, Copy)]
struct Held {
    packet: Packet,
    pattern: Option<PatternId>,
    start: Ticks,
    loop_end: Option<Ticks>,
}

pub struct State {
    connection: Option<MidiInputConnection<()>>,
    pub ports: Vec<Port>,
    pub selected: Option<Port>,
    pub recording: bool,
    target: Arc<Mutex<(u64, ChannelId)>>,
    armed: Arc<AtomicBool>,
    events: Option<mpsc::Receiver<Packet>>,
    overflow: Arc<AtomicBool>,
    held: HashMap<(u8, u8), Held>,
    take: Option<(PatternId, Ticks)>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            connection: None, ports: vec![], selected: None, recording: false,
            target: Arc::new(Mutex::new((0, ChannelId(0)))),
            armed: Arc::new(AtomicBool::new(false)), events: None,
            overflow: Arc::new(AtomicBool::new(false)), held: HashMap::new(), take: None,
        }
    }
}

impl State {
    pub fn rescan(&mut self) -> Result<(), String> {
        let input = MidiInput::new("DAW MIDI").map_err(|e| e.to_string())?;
        let mut ports = vec![Port { index: None, name: "off".into() }];
        for (index, port) in input.ports().iter().enumerate() {
            ports.push(Port { index: Some(index), name: input.port_name(port).map_err(|e| e.to_string())? });
        }
        self.ports = ports;
        if self.selected.is_none() { self.selected = self.ports.first().cloned(); }
        Ok(())
    }

    pub fn connect(&mut self, port: Port, shared: Arc<Shared>) -> Result<rtrb::Consumer<LiveNote>, String> {
        self.connection = None;
        self.events = None;
        self.selected = None;
        let (mut live, consumer) = rtrb::RingBuffer::new(4096);
        let Some(index) = port.index else { self.selected = Some(port); return Ok(consumer) };
        let mut input = MidiInput::new("DAW MIDI").map_err(|e| e.to_string())?;
        input.ignore(Ignore::SysexAndTime);
        let device = input.ports().get(index).cloned().ok_or("MIDI device disappeared; rescan inputs")?;
        if input.port_name(&device).map_err(|e| e.to_string())? != port.name {
            return Err("MIDI devices changed; rescan inputs".into());
        }
        let (tx, events) = mpsc::sync_channel(4096);
        let target = self.target.clone();
        let armed = self.armed.clone();
        let overflow = self.overflow.clone();
        let mut targets = [[None::<(u64, ChannelId)>; 128]; 16];
        self.connection = Some(input.connect(&device, "DAW input", move |_, bytes, _| {
            let Some((midi_channel, key, velocity)) = decode(bytes) else { return };
            let slot = &mut targets[midi_channel as usize][key as usize];
            let (target_node, target_channel) = if velocity > 0.0 {
                let Ok(target) = target.lock() else { return };
                *slot = Some(*target);
                *target
            } else {
                let Some(target) = slot.take() else { return };
                target
            };
            if target_node == 0 { return; }
            if live.push(LiveNote { node: target_node, key, velocity }).is_err() { overflow.store(true, Ordering::Relaxed); }
            let packet = Packet { midi_channel, node: target_node, channel: target_channel, key, velocity,
                tick: shared.position(), elapsed: shared.record_position(), recording: armed.load(Ordering::Relaxed) && shared.playing() };
            if tx.try_send(packet).is_err() { overflow.store(true, Ordering::Relaxed); }
        }, ()).map_err(|e| e.to_string())?);
        self.selected = Some(port);
        self.events = Some(events);
        Ok(consumer)
    }
}

fn decode(bytes: &[u8]) -> Option<(u8, u8, f32)> {
    if bytes.len() != 3 || bytes[1] > 127 || bytes[2] > 127 { return None; }
    let velocity = match bytes[0] & 0xf0 { 0x90 => f32::from(bytes[2]) / 127.0, 0x80 => 0.0, _ => return None };
    Some((bytes[0] & 0x0f, bytes[1], velocity))
}

impl App {
    pub fn midi_target(&self) {
        let selected = self.selected_channel.and_then(|id| self.project.channel(id));
        if let Ok(mut target) = self.midi.target.lock() {
            *target = selected.map_or((0, ChannelId(0)), |c| (channel_node(&c.source, c.id), c.id));
        }
    }

    pub fn poll_midi(&mut self) {
        let packets: Vec<_> = self.midi.events.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        for packet in packets { self.midi_packet(packet); }
        if self.midi.overflow.swap(false, Ordering::Relaxed) {
            self.finish_midi();
            self.session.release_notes();
            self.set_status("MIDI input queue filled; notes released");
        }
        self.midi_target();
    }

    pub fn midi_packet(&mut self, packet: Packet) {
        let identity = (packet.midi_channel, packet.key);
        if let Some(held) = self.midi.held.remove(&identity) { self.finish_midi_note(held, packet.elapsed); }
        if packet.velocity <= 0.0 { return; }
        let mut held = Held { packet, pattern: None, start: packet.tick.max(0.0).round() as Ticks, loop_end: None };
        if packet.recording && self.midi.recording && self.project.channel(packet.channel).is_some() {
            let (pattern, base) = match self.mode {
                PlayMode::Pattern(id) => (id, 0),
                PlayMode::Song => match self.midi.take {
                    Some(take) => take,
                    None => {
                        self.checkpoint();
                        let id = self.project.add_pattern();
                        self.project.pattern_mut(id).unwrap().name = "MIDI take".into();
                        let base = self.project.playlist.loop_range.map_or(self.song_start.max(0.0).round() as Ticks, |r| r.0);
                        let end = self.project.playlist.loop_range.map_or(base + self.project.signature.ticks_per_bar(), |r| r.1);
                        self.project.set_pattern_length(id, end - base);
                        let track = self.project.free_track(base, end);
                        self.project.add_clip(track, base, ClipSource::Pattern(id));
                        self.midi.take = Some((id, base));
                        self.edited();
                        (id, base)
                    }
                },
            };
            held.pattern = Some(pattern);
            held.start = held.start.saturating_sub(base);
            held.loop_end = match self.mode {
                PlayMode::Pattern(_) => self.project.pattern(pattern).map(|p| p.length),
                PlayMode::Song => self.project.playlist.loop_range.map(|(a, b)| b - a),
            };
        }
        self.midi.held.insert(identity, held);
    }

    fn finish_midi_note(&mut self, held: Held, elapsed: f64) {
        let Some(pattern) = held.pattern.filter(|id| self.project.pattern(*id).is_some()) else { return };
        let length = ((elapsed - held.packet.elapsed).max(0.0).round() as Ticks).max(1);
        // Overdubbing a held key across several loops fills one loop instead
        // of layering duplicate notes of the same pitch on every pass.
        let length = held.loop_end.map_or(length, |boundary| length.min(boundary));
        self.checkpoint();
        let note = Note { start: held.start, length, key: held.packet.key, velocity: held.packet.velocity };
        let notes = self.project.pattern_mut(pattern).unwrap().notes_mut(held.packet.channel);
        if let Some(boundary) = held.loop_end.filter(|&l| note.end() > l) {
            notes.push(Note { length: boundary.saturating_sub(note.start).max(1), ..note });
            notes.push(Note { start: 0, length: note.end() - boundary, ..note });
        } else { notes.push(note); }
        notes.sort_by_key(|n| n.start);
        if held.loop_end.is_none() {
            let length = self.project.bars_to(note.end());
            if length > self.project.pattern(pattern).unwrap().length { self.project.set_pattern_length(pattern, length); }
        }
        self.edited();
    }

    pub fn finish_midi(&mut self) {
        let elapsed = self.session.shared().record_position();
        let held = std::mem::take(&mut self.midi.held);
        for held in held.into_values() {
            self.session.note(held.packet.node, held.packet.key, 0.0);
            self.finish_midi_note(held, elapsed);
        }
        self.midi.take = None;
    }

    /// Replacing the input queue discards notes addressed to the old project.
    pub fn suspend_midi_input(&mut self) {
        self.midi.connection = None;
        self.poll_midi();
        self.finish_midi();
    }

    pub fn reset_midi_input(&mut self) {
        if let Some(port) = self.midi.selected.clone() {
            match self.midi.connect(port, self.session.midi_shared()) {
                Ok(input) => self.session.attach_midi(input),
                Err(error) => self.set_status(format!("MIDI input failed: {error}")),
            }
        }
    }

    pub fn toggle_midi_recording(&mut self) {
        self.poll_midi();
        self.finish_midi();
        self.midi.recording = !self.midi.recording;
        self.midi.armed.store(self.midi.recording, Ordering::Relaxed);
        self.set_status(if self.midi.recording { "MIDI recording armed: play to record" } else { "MIDI recording off" });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_messages_validate_bytes_and_zero_velocity_is_note_off() {
        assert_eq!(decode(&[0x92, 60, 127]), Some((2, 60, 1.0)));
        assert_eq!(decode(&[0x92, 60, 0]), Some((2, 60, 0.0)));
        assert_eq!(decode(&[0x82, 60, 64]), Some((2, 60, 0.0)));
        for bytes in [&[0x90, 60][..], &[0x90, 128, 1], &[0x90, 60, 128], &[0xb0, 60, 64]] {
            assert!(decode(bytes).is_none());
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn virtual_midi_port_delivers_live_notes_and_retains_note_on_target() {
        use midir::os::unix::VirtualOutput;
        let name = format!("DAW test {}", crate::project_files::stamp());
        let mut output = midir::MidiOutput::new("DAW test output").unwrap().create_virtual(&name).unwrap();
        let mut input = State::default();
        input.rescan().unwrap();
        let port = input.ports.iter().find(|p| p.name == name).cloned().expect("virtual source is listed");
        *input.target.lock().unwrap() = (10, ChannelId(9));
        let (_, engine) = daw_engine::create(48_000.0);
        let mut live = input.connect(port, engine.shared).unwrap();
        output.send(&[0x90, 64, 127]).unwrap();
        let on = input.events.as_ref().unwrap().recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert_eq!((on.node, on.channel, on.key, on.velocity), (10, ChannelId(9), 64, 1.0));
        let note = live.pop().unwrap();
        assert_eq!((note.node, note.key, note.velocity), (10, 64, 1.0));
        *input.target.lock().unwrap() = (12, ChannelId(11));
        output.send(&[0x80, 64, 0]).unwrap();
        let off = input.events.as_ref().unwrap().recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert_eq!((off.node, off.channel, off.velocity), (10, ChannelId(9), 0.0));
        let note = live.pop().unwrap();
        assert_eq!((note.node, note.key, note.velocity), (10, 64, 0.0));
    }
}
