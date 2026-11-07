//! Work that an update asks the UI shell to do after it returns: file
//! dialogs, background threads, focusing a field, or quitting. Building or
//! dropping a task has no side effects, so tests can discard them.

use std::path::PathBuf;
use std::sync::mpsc::Sender;

use crate::app::Message;

/// A file dialog request. `then` turns the chosen path, or `None` when
/// cancelled, into the message that handles it.
pub struct Dialog {
    pub save: bool,
    pub title: String,
    pub filter: String,
    pub extensions: Vec<String>,
    pub folder: Option<PathBuf>,
    pub file_name: Option<String>,
    pub then: fn(Option<PathBuf>) -> Message,
}

pub enum Effect {
    /// Handle this message next.
    Message(Message),
    Dialog(Dialog),
    /// Run on a new thread; it sends messages back to the app.
    Thread(Box<dyn FnOnce(Sender<Message>) + Send>),
    /// Move keyboard focus to the text field with this key and select its text.
    Focus(&'static str),
    /// Scroll the horizontal scroll view with this key by this many points.
    ScrollX(&'static str, f32),
    Exit,
}

#[must_use]
#[derive(Default)]
pub struct Task(pub Vec<Effect>);

impl Task {
    pub fn none() -> Task {
        Task(Vec::new())
    }

    pub fn done(message: impl Into<Message>) -> Task {
        Task(vec![Effect::Message(message.into())])
    }

    pub fn batch(tasks: impl IntoIterator<Item = Task>) -> Task {
        Task(tasks.into_iter().flat_map(|t| t.0).collect())
    }

    pub fn exit() -> Task {
        Task(vec![Effect::Exit])
    }

    pub fn focus(key: &'static str) -> Task {
        Task(vec![Effect::Focus(key)])
    }

    pub fn scroll_x(key: &'static str, by: f32) -> Task {
        Task(vec![Effect::ScrollX(key, by)])
    }

    pub fn thread(work: impl FnOnce(Sender<Message>) + Send + 'static) -> Task {
        Task(vec![Effect::Thread(Box::new(work))])
    }

    /// Ask for an existing file, starting in `folder` when given.
    pub fn open(filter: &str, extensions: &[&str], folder: Option<PathBuf>, then: fn(Option<PathBuf>) -> Message) -> Task {
        Task(vec![Effect::Dialog(Dialog {
            save: false,
            title: format!("Open {filter}"),
            filter: filter.to_string(),
            extensions: extensions.iter().map(|e| e.to_string()).collect(),
            folder,
            file_name: None,
            then,
        })])
    }

    /// Ask where to write a new file, suggesting `file_name`.
    pub fn save(filter: &str, extensions: &[&str], file_name: &str, then: fn(Option<PathBuf>) -> Message) -> Task {
        Task(vec![Effect::Dialog(Dialog {
            save: true,
            title: format!("Save {filter}"),
            filter: filter.to_string(),
            extensions: extensions.iter().map(|e| e.to_string()).collect(),
            folder: None,
            file_name: Some(file_name.to_string()),
            then,
        })])
    }

}

impl std::fmt::Debug for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Task({} effects)", self.0.len())
    }
}
