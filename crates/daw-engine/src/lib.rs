//! Realtime audio engine: transport, scheduling, mixing, and built-in instruments.

pub mod engine;
pub mod input;
pub mod output;
pub mod processor;
pub mod song;
pub mod synth;

pub use engine::{Command, Engine, EngineHandle, MAX_BLOCK, Node, create};
pub use processor::{Event, EventKind, Processor, TransportInfo};
pub use song::{PlayMode, Song, compile};
