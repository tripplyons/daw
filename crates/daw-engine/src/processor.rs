//! The interface every sound source and effect implements, built-in or plugin.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EventKind {
    NoteOn { key: u8, velocity: f32 },
    NoteOff { key: u8 },
    /// Normalized 0..1 parameter value.
    Param { id: u32, value: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Event {
    /// Frame offset within the block.
    pub offset: u32,
    pub kind: EventKind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransportInfo {
    pub sample_rate: f64,
    pub bpm: f64,
    /// Position at the block start, in quarter notes.
    pub beats: f64,
    /// Position at the block start, in frames since the song start.
    pub frames: i64,
    pub playing: bool,
    pub numerator: u32,
    pub denominator: u32,
    /// Start of the bar containing `beats`, in quarter notes.
    pub bar_start: f64,
}

/// Runs on the audio thread. `process` must not allocate, lock, or block.
pub trait Processor: Send {
    /// Instruments overwrite the buffers; effects read them and write in place.
    /// Events are sorted by offset and fall inside the block.
    fn process(&mut self, transport: &TransportInfo, events: &[Event], left: &mut [f32], right: &mut [f32]);

    /// Silence all voices and tails, e.g. after a seek.
    fn reset(&mut self);
}
