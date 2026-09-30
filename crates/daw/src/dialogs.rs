//! Native file dialogs, each resolving to the chosen path or `None` when
//! cancelled.

use std::path::PathBuf;

/// Ask for an existing file, starting in `folder` when given.
pub async fn open(filter: &str, extensions: &[&str], folder: Option<PathBuf>) -> Option<PathBuf> {
    let mut dialog = rfd::AsyncFileDialog::new().add_filter(filter, extensions);
    if let Some(folder) = folder {
        dialog = dialog.set_directory(folder);
    }
    dialog.pick_file().await.map(|f| f.path().to_owned())
}

/// Ask where to write a new file, suggesting `file_name`.
pub async fn save(filter: &str, extensions: &[&str], file_name: &str) -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .add_filter(filter, extensions)
        .set_file_name(file_name)
        .save_file()
        .await
        .map(|f| f.path().to_owned())
}
