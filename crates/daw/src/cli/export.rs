//! `daw export`: render a project to WAV without a window, loading its plugins.

use std::path::Path;

use daw_engine::output::{self, BitDepth, Stem};
use daw_model::Project;
use daw_model::time::Ticks;

use crate::processing::Control;
use crate::render::Renderer;
use crate::session::RenderOptions;

/// Render the song to `out` as 24-bit WAV, and each reached mixer insert to
/// `stems` when a folder is given.
pub fn export(project: &Project, out: &Path, range: Option<(Ticks, Ticks)>, stems: Option<&Path>, tail: Option<f64>) -> Result<(), String> {
    let stems = match stems {
        Some(folder) => stem_files(project, folder)?,
        None => Vec::new(),
    };
    let sample_rate = output::default_sample_rate().unwrap_or(48_000.0);
    let mut renderer = Renderer::new(project, Default::default(), Default::default(), sample_rate)?;
    let options = RenderOptions { depth: BitDepth::Int24, range, tail_seconds: tail.unwrap_or(project.render.export_tail_seconds) };
    renderer.worker.run(out, options, &stems, &Control::default()).map(|_| ()).map_err(|e| format!("export failed: {e}"))
}

/// One file per insert that some channel's signal reaches, plus the master.
fn stem_files(project: &Project, folder: &Path) -> Result<Vec<Stem>, String> {
    std::fs::create_dir_all(folder).map_err(|e| format!("could not create {}: {e}", folder.display()))?;
    let reached = |id| project.channels.iter().any(|c| c.insert == id || project.mixer.feeds(c.insert, id));
    Ok(project
        .mixer
        .inserts
        .iter()
        .enumerate()
        .filter(|&(index, insert)| index == 0 || reached(insert.id))
        .map(|(index, insert)| Stem {
            insert: index,
            path: folder.join(format!("{index:02} {}.wav", insert.name.replace(['/', '\\'], "-"))),
        })
        .collect())
}
