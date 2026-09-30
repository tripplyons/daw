//! Serialize archive writes on a worker. Plugin state is captured on the UI thread.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

use daw_model::Project;
use iced::Task;

use super::{App, Message};

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Save(PathBuf),
    Pack(PathBuf),
    Autosave(PathBuf),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    kind: Kind,
    generation: u64,
    revision: u64,
}

struct Request {
    job: Job,
    project: Project,
}

pub struct Finished {
    job: Job,
    result: Result<(), String>,
}

#[derive(Default)]
pub struct State {
    generation: u64,
    waiting_revision: Option<u64>,
    worker: Option<(Sender<Request>, Receiver<Finished>)>,
    pending: Vec<Job>,
}

impl State {
    pub fn new_project(&mut self) { self.generation += 1; self.cancel_wait(); }

    pub fn wait_for(&mut self, path: Option<&PathBuf>, revision: u64) -> bool {
        let saving = self.pending.iter().any(|j| j.generation == self.generation && j.revision == revision
            && matches!(&j.kind, Kind::Save(p) if Some(p) == path));
        self.waiting_revision = saving.then_some(revision);
        saving
    }

    pub fn cancel_wait(&mut self) { self.waiting_revision = None; }

    pub fn autosaving(&self) -> bool {
        self.pending.iter().any(|j| j.generation == self.generation && matches!(j.kind, Kind::Autosave(_)))
    }

    fn queue(&mut self, kind: Kind, revision: u64, project: Project) -> Result<(), String> {
        let job = Job { kind, generation: self.generation, revision };
        if self.pending.contains(&job) { return Ok(()); }
        if self.worker.is_none() {
            let (send, requests) = mpsc::channel::<Request>();
            let (completed, receive) = mpsc::channel();
            std::thread::Builder::new().name("project saves".into()).spawn(move || {
                for Request { job, project } in requests {
                    let result = match &job.kind {
                        Kind::Save(path) | Kind::Pack(path) => crate::project_files::save(path, &project),
                        Kind::Autosave(folder) => crate::project_files::snapshot(folder, &project).map(|_| ()),
                    };
                    if completed.send(Finished { job, result }).is_err() { break; }
                }
            }).map_err(|e| e.to_string())?;
            self.worker = Some((send, receive));
        }
        self.worker.as_ref().unwrap().0.send(Request { job: job.clone(), project }).map_err(|e| e.to_string())?;
        self.pending.push(job);
        Ok(())
    }

    fn poll(&mut self) -> Vec<Finished> {
        let finished: Vec<_> = self.worker.as_ref().map(|(_, r)| r.try_iter().collect()).unwrap_or_default();
        for result in &finished { self.pending.retain(|j| j != &result.job); }
        finished
    }

    #[cfg(test)]
    pub fn wait(&mut self) -> Option<Finished> {
        if self.pending.is_empty() { return None; }
        let result = self.worker.as_ref().unwrap().1.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        self.pending.retain(|j| j != &result.job);
        Some(result)
    }
}

impl App {
    fn queue_save(&mut self, kind: Kind) {
        self.session.store_states(&mut self.project);
        if let Err(error) = self.saving.queue(kind, self.revision, self.project.clone()) {
            self.pending = None;
            self.saving.cancel_wait();
            self.set_status(format!("save failed: {error}"));
        }
    }

    pub fn autosave(&mut self) {
        if self.saving.autosaving() { return; }
        self.last_autosave = std::time::Instant::now();
        let folder = crate::project_files::backups_dir().join(&self.backup_key);
        self.queue_save(Kind::Autosave(folder));
    }

    pub(super) fn save(&mut self) {
        let _ = self.update(Message::BpmDone);
        let Some(path) = self.path.clone() else { return };
        if let Some(name) = path.file_stem() { self.project.name = name.to_string_lossy().into_owned(); }
        self.set_status(format!("saving {}", path.display()));
        self.queue_save(Kind::Save(path.clone()));
        if self.pending.is_some() { self.saving.wait_for(Some(&path), self.revision); }
    }

    pub(super) fn pack(&mut self, path: PathBuf) {
        let _ = self.update(Message::BpmDone);
        self.set_status(format!("packaging {}", path.display()));
        self.queue_save(Kind::Pack(path));
    }

    pub(super) fn poll_saves(&mut self) -> Task<Message> {
        let tasks: Vec<_> = self.saving.poll().into_iter().map(|r| self.save_finished(r)).collect();
        Task::batch(tasks)
    }

    pub(super) fn save_finished(&mut self, finished: Finished) -> Task<Message> {
        let Finished { job, result } = finished;
        if job.generation != self.saving.generation { return Task::none(); }
        let manual = matches!(&job.kind, Kind::Save(path) if self.path.as_ref() == Some(path));
        let continuing = manual && self.saving.waiting_revision == Some(job.revision);
        if continuing { self.saving.cancel_wait(); }
        if let Err(error) = result {
            let action = match job.kind { Kind::Save(_) => "save", Kind::Pack(_) => "package", Kind::Autosave(_) => "autosave" };
            if continuing { self.pending = None; }
            self.set_status(format!("{action} failed: {error}"));
            return Task::none();
        }
        match job.kind {
            Kind::Save(path) if manual => {
                if self.revision == job.revision { self.dirty = false; }
                self.backup_key = crate::project_files::backup_key(&self.project.name, &path);
                self.set_status(format!("saved {}{}", path.display(), if self.dirty { "; newer changes remain unsaved" } else { "" }));
                if continuing && let Some(next) = self.pending.take()
                    && !self.dirty { return self.proceed(next); }
            }
            Kind::Pack(path) => self.set_status(format!("packaged {}", path.display())),
            Kind::Autosave(_) => self.autosaved_revision = job.revision,
            _ => {}
        }
        Task::none()
    }
}
