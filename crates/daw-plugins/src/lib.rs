//! VST3 hosting on macOS and Linux, and Audio Units on macOS.

#[cfg(target_os = "macos")]
pub mod au;
pub mod scan;
pub mod vst3;
#[cfg(target_os = "macos")]
mod window;
#[cfg(target_os = "linux")]
#[path = "window_linux.rs"]
mod window;

#[cfg(target_os = "macos")]
pub use window::{KeyPress, set_unhandled_keys};

use daw_engine::Processor;
use daw_model::{PluginFormat, PluginRef};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PluginKind {
    Instrument,
    Effect,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginInfo {
    pub plugin: PluginRef,
    pub kind: PluginKind,
    /// Free-form category from the plugin, e.g. "Fx|Dynamics".
    pub category: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamInfo {
    pub id: u32,
    pub name: String,
    pub units: String,
    /// Discrete step count; 0 for continuous.
    pub steps: u32,
    pub default: f32,
    pub automatable: bool,
}

/// A parameter change made in the plugin's own editor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Touch {
    pub param: u32,
    pub value: f32,
}

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("{0}")]
    Load(String),
    #[error("plugin {0} not found")]
    NotFound(String),
    #[error("{call} failed with {code}")]
    Call { call: &'static str, code: i32 },
}

/// Main-thread side of a loaded plugin: parameters, state, and editor.
pub trait Controller {
    fn params(&self) -> Vec<ParamInfo>;
    fn param_value(&self, id: u32) -> f32;
    /// Update the controller's view of a parameter. The engine gets the same
    /// change separately through its command queue.
    fn set_param(&mut self, id: u32, value: f32);
    fn param_text(&self, id: u32, value: f32) -> String;
    fn save_state(&self) -> Result<Vec<u8>, PluginError>;
    fn has_editor(&self) -> bool;
    /// Show the editor, creating it the first time. Later calls show the same
    /// editor again, because some plugins fail to build a second one.
    fn open_editor(&mut self, title: &str) -> Result<(), PluginError>;
    /// Hide the editor window. The editor stays alive until the plugin is dropped.
    fn hide_editor(&mut self);
    /// Whether the editor window is showing.
    fn editor_open(&self) -> bool;
    /// Parameter edits from the plugin editor since the last call. Call often
    /// from the UI thread: on Linux this also runs the editor's events.
    fn take_touches(&mut self) -> Vec<Touch>;
}

pub struct Loaded {
    pub processor: Box<dyn Processor>,
    pub controller: Box<dyn Controller>,
}

/// Instantiate a plugin, restoring `state` when non-empty. Must run on the main thread.
pub fn load(plugin: &PluginRef, state: &[u8], sample_rate: f64, max_block: usize) -> Result<Loaded, PluginError> {
    match plugin.format {
        PluginFormat::Vst3 => vst3::load(plugin, state, sample_rate, max_block),
        #[cfg(target_os = "macos")]
        PluginFormat::AudioUnit => au::load(plugin, state, sample_rate, max_block),
        #[cfg(not(target_os = "macos"))]
        PluginFormat::AudioUnit => Err(PluginError::Load("Audio Units are only supported on macOS".into())),
    }
}
