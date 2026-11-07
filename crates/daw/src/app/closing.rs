//! Close the current project for a new, opened, or recovered one, or to
//! quit, asking to save unsaved changes first.

use std::path::PathBuf;
use std::time::Instant;

use daw_engine::song::PlayMode;
use daw_model::Project;
use crate::task::Task;

use super::{App, Message};

/// What replaces the current project once its unsaved changes are saved or
/// discarded.
#[derive(Debug, Clone, PartialEq)]
pub enum Pending {
    Close,
    New,
    Open(PathBuf),
    Recover(PathBuf),
}

impl Pending {
    fn verb(&self) -> String {
        match self {
            Pending::Close => "closing".into(),
            Pending::New => "starting a new project".into(),
            Pending::Open(path) => format!("opening \"{}\"", path.file_name().unwrap_or_default().to_string_lossy()),
            Pending::Recover(_) => "recovering a backup".into(),
        }
    }
}

impl App {
    /// Run `next` now, or first ask to save unsaved changes. Ignored while
    /// the prompt is already showing.
    pub(super) fn guard(&mut self, next: Pending) -> Task {
        let _ = self.update(Message::BpmDone);
        self.poll_midi();
        self.finish_midi();
        if self.pending.is_some() {
            return Task::none();
        }
        if self.saving.wait_for(self.path.as_ref(), self.revision) {
            self.pending = Some(next);
            return Task::none();
        }
        if !self.dirty {
            return self.proceed(next);
        }
        let prompt = self.ask_to_save(&next);
        self.pending = Some(next);
        prompt
    }

    pub(super) fn proceed(&mut self, next: Pending) -> Task {
        match next {
            Pending::Close => return Task::exit(),
            Pending::New => self.new_project(),
            Pending::Open(path) => self.open(path),
            Pending::Recover(path) => {
                self.open(path.clone());
                if self.path.as_ref() == Some(&path) {
                    self.path = None;
                    self.dirty = true;
                    self.revision = self.revision.wrapping_add(1);
                    self.set_status("backup recovered; Save As to keep the recovered project");
                }
            }
        }
        Task::none()
    }

    pub(super) fn new_project(&mut self) {
        self.replace_project(Project::new(), None, crate::project_files::stamp());
    }

    /// Close the current project, with its jobs, recordings, and history,
    /// and start editing `project`.
    fn replace_project(&mut self, project: Project, path: Option<PathBuf>, backup_key: String) {
        self.saving.new_project();
        self.rendering.new_project();
        self.suspend_midi_input();
        if self.midi.recording { self.toggle_midi_recording(); }
        self.backup_key = backup_key;
        self.last_autosave = Instant::now();
        self.stop_audio_recording();
        self.session.clear();
        self.project = project;
        self.path = path;
        self.undo.clear();
        self.redo.clear();
        self.dirty = false;
        self.song_start = 0.0;
        self.pattern_start = 0.0;
        self.selected_pattern = self.project.patterns[0].id;
        self.mode = PlayMode::Song;
        self.validate_selection();
        self.refresh();
        self.reset_midi_input();
    }

    /// Show the unsaved changes prompt; its buttons send `SaveChoice`.
    fn ask_to_save(&mut self, _next: &Pending) -> Task {
        self.asking = true;
        Task::none()
    }

    /// The prompt's title and question, while it shows.
    pub(super) fn pending_question(&self) -> Option<(String, String)> {
        let next = self.pending.as_ref().filter(|_| self.asking)?;
        let name = self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
        let name = name.unwrap_or_else(|| self.project.name.clone());
        Some((
            format!("Save changes to \"{name}\"?"),
            format!("Your changes will be lost if you don't save them before {}.", next.verb()),
        ))
    }

    /// Whether a close is waiting on the prompt or a save.
    pub fn closing_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn open(&mut self, path: PathBuf) {
        let project = match crate::project_files::load(&path) {
            Ok(project) => project,
            Err(error) => {
                self.set_status(format!("open failed: {error}"));
                return;
            }
        };
        let backup_key = crate::project_files::backup_key(&project.name, &path);
        self.replace_project(project, Some(path), backup_key);
        let failures = self.session.load_errors.len();
        self.set_status(if let Some(error) = &self.session.preparation_error {
            format!("opened; audio preparation failed: {error}")
        } else if failures == 0 {
            "opened".to_string()
        } else {
            format!("opened; {failures} plugins failed to load")
        });
    }
}
