//! Cancellable audio preparation. Results are accepted only by the current job.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use daw_engine::audio::{self, Cache};
use daw_model::Project;

#[derive(Default)]
struct Shared { cancelled: AtomicBool, progress: AtomicU32 }

#[derive(Clone, Default)]
pub struct Control(Arc<Shared>);

impl Control {
    pub fn cancel(&self) { self.0.cancelled.store(true, Ordering::Relaxed); }
    pub fn cancelled(&self) -> bool { self.0.cancelled.load(Ordering::Relaxed) }
    pub fn progress(&self) -> f32 { f32::from_bits(self.0.progress.load(Ordering::Relaxed)) }
    pub fn update(&self, fraction: f32) -> bool {
        self.0.progress.store(fraction.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        !self.cancelled()
    }
}

pub type Peaks = HashMap<String, Arc<Vec<f32>>>;

pub struct Finished { pub cache: Cache, pub peaks: Peaks, pub result: Result<(), String> }

pub struct Preparation {
    pub needed: HashSet<String>,
    pub control: Control,
    receive: Receiver<Finished>,
}

impl Preparation {
    pub fn start(project: Project, mut cache: Cache, mut peaks: Peaks) -> Result<Self, String> {
        let control = Control::default();
        let worker_control = control.clone();
        let needed = Cache::required(&project);
        let (send, receive) = mpsc::channel();
        std::thread::Builder::new().name("audio preparation".into()).spawn(move || {
            let result = audio::prepare_with_progress(&project, &mut cache, |p| worker_control.update(p * 0.8))
                .and_then(|()| prepare_peaks(&cache, &mut peaks, &worker_control));
            cache.trim(&project);
            peaks.retain(|key, _| cache.contains_key(key));
            let _ = send.send(Finished { cache, peaks, result });
        }).map_err(|e| e.to_string())?;
        Ok(Self { needed, control, receive })
    }

    pub fn poll(&self) -> Result<Option<Finished>, String> {
        match self.receive.try_recv() {
            Ok(finished) => Ok(Some(finished)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err("audio worker stopped before completion".into()),
        }
    }
}

impl Drop for Preparation { fn drop(&mut self) { self.control.cancel(); } }

fn prepare_peaks(cache: &Cache, peaks: &mut Peaks, control: &Control) -> Result<(), String> {
    for (index, (key, sample)) in cache.iter().enumerate() {
        if peaks.contains_key(key) { continue; }
        let mut summary = Vec::with_capacity(sample.left.len().div_ceil(crate::session::PEAK_FRAMES));
        for (chunk, (l, r)) in sample.left.chunks(crate::session::PEAK_FRAMES).zip(sample.right.chunks(crate::session::PEAK_FRAMES)).enumerate() {
            if chunk % 256 == 0 {
                let fraction = chunk as f32 * crate::session::PEAK_FRAMES as f32 / sample.left.len().max(1) as f32;
                if !control.update(0.8 + 0.2 * (index as f32 + fraction) / cache.len().max(1) as f32) { return Err("cancelled".into()); }
            }
            summary.push(l.iter().chain(r).fold(0.0f32, |m, s| m.max(s.abs())));
        }
        peaks.insert(key.clone(), Arc::new(summary));
    }
    if control.update(1.0) { Ok(()) } else { Err("cancelled".into()) }
}
