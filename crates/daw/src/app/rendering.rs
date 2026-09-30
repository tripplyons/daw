//! Render snapshots in the background and apply consolidation only if still current.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::JoinHandle;

use daw_engine::output::BitDepth;
use daw_model::Clip;
use daw_model::time::Ticks;

use super::App;
use crate::processing::Control;
use crate::render::{Renderer, Worker};
use crate::session::RenderOptions;

pub enum Kind {
    Export,
    Consolidate { clips: Vec<Clip>, start: Ticks, length: Ticks },
}

struct File { path: PathBuf, temporary: PathBuf }

impl File {
    fn publish(&self) -> Result<(), String> { std::fs::rename(&self.temporary, &self.path).map_err(|e| e.to_string()) }
}

impl Drop for File { fn drop(&mut self) { let _ = std::fs::remove_file(&self.temporary); } }

struct Job {
    // Declared first so the thread stops before the temporary file is removed.
    thread: Thread,
    kind: Kind,
    file: File,
    generation: u64,
    revision: u64,
}

/// The render thread, and the native controllers its plugins need until it ends.
struct Thread {
    control: Control,
    receive: Receiver<(Worker, Result<bool, String>)>,
    handle: Option<JoinHandle<()>>,
    _controllers: Vec<Box<dyn daw_plugins::Controller>>,
}

impl Drop for Thread {
    fn drop(&mut self) {
        self.control.cancel();
        if let Some(handle) = self.handle.take() { let _ = handle.join(); }
        // Destroy returned processors here, before dropping native controllers.
        while let Ok(result) = self.receive.try_recv() { drop(result); }
    }
}

#[derive(Default)]
pub struct State { generation: u64, job: Option<Job> }

impl State {
    pub fn busy(&self) -> bool { self.job.is_some() }
    pub fn new_project(&mut self) { self.generation += 1; self.cancel(); }
    pub fn cancel(&self) { if let Some(job) = &self.job { job.thread.control.cancel(); } }
    pub fn progress(&self) -> Option<(&str, f32)> {
        let job = self.job.as_ref().filter(|job| job.generation == self.generation)?;
        let label = if matches!(job.kind, Kind::Export) { "export" } else { "consolidate" };
        Some((label, job.thread.control.progress()))
    }

    pub fn start(&mut self, renderer: Renderer, kind: Kind, path: PathBuf, options: RenderOptions, revision: u64) -> Result<(), String> {
        if self.busy() { return Err("an audio render is already running".into()); }
        let temporary = path.with_extension(format!("{}.wav.tmp", crate::project_files::stamp()));
        let file = File { path, temporary: temporary.clone() };
        let control = Control::default();
        let worker_control = control.clone();
        let (send, receive) = mpsc::channel();
        let Renderer { mut worker, controllers } = renderer;
        let handle = std::thread::Builder::new().name("audio render".into()).spawn(move || {
            let result = worker.run(&temporary, options, &[], &worker_control);
            let _ = send.send((worker, result));
        }).map_err(|e| e.to_string())?;
        let thread = Thread { control, receive, handle: Some(handle), _controllers: controllers };
        self.job = Some(Job { thread, kind, file, generation: self.generation, revision });
        Ok(())
    }
}

impl App {
    /// Start rendering the whole song to `path` as 24-bit WAV.
    pub(super) fn export(&mut self, path: PathBuf) -> Result<(), String> {
        if self.rendering.busy() { return Err("an audio render is already running".into()); }
        self.session.store_states(&mut self.project);
        let options = RenderOptions { depth: BitDepth::Int24, range: None, tail_seconds: self.project.render.export_tail_seconds };
        let renderer = self.session.renderer(&self.project)?;
        self.rendering.start(renderer, Kind::Export, path, options, self.revision)
    }

    pub(super) fn poll_renders(&mut self) {
        let Some(job) = &self.rendering.job else { return };
        let result = match job.thread.receive.try_recv() {
            Ok((worker, result)) => { drop(worker); result }
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("audio render worker stopped before completion".into()),
        };
        let Some(Job { thread, kind, file, generation, revision }) = self.rendering.job.take() else { return };
        if generation != self.rendering.generation { return; }
        if thread.control.cancelled() || matches!(result, Ok(false)) { self.set_status("audio render cancelled"); return; }
        if let Err(error) = result { self.set_status(format!("audio render failed: {error}")); return; }
        if matches!(kind, Kind::Consolidate { .. }) && revision != self.revision {
            self.set_status("project changed during consolidation; consolidate the selection again");
            return;
        }
        if let Err(error) = file.publish() { self.set_status(format!("could not publish audio: {error}")); return; }
        match kind {
            Kind::Export => self.set_status(format!("exported {}", file.path.display())),
            Kind::Consolidate { clips, start, length } => self.place_consolidation(&file.path, &clips, start, length),
        }
    }
}
