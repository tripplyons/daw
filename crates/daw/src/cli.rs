//! Command-line interface: open the app, or inspect, edit, and render
//! project files without a window.

mod edit;
mod parse;
mod show;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use daw_model::Project;
use daw_plugins::scan::{self, Catalog};
use daw_plugins::PluginKind;

pub use edit::Op;
use parse::Time;

#[derive(Parser)]
#[command(name = "daw", about = "A tiling DAW. With no command, opens the app.", args_conflicts_with_subcommands = true)]
pub struct Cli {
    /// Project to open in the app.
    pub project: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Create a project with the master, 8 mixer inserts, a synth channel, and one empty pattern.
    New {
        file: PathBuf,
        /// Replace the file if it exists.
        #[arg(long)]
        force: bool,
    },
    /// Print a project overview with ids, or one pattern's notes or automation clip's points.
    Show {
        file: PathBuf,
        #[arg(long, conflicts_with = "automation")]
        pattern: Option<u64>,
        #[arg(long)]
        automation: Option<u64>,
        /// Print the project (or the chosen pattern or clip) as JSON.
        #[arg(long)]
        json: bool,
    },
    /// List installed plugins from the scan cache.
    Plugins {
        /// Scan the plugin folders first, as the app does on startup.
        #[arg(long)]
        scan: bool,
        #[arg(long)]
        json: bool,
    },
    /// Load a plugin instance from a project and list its parameters.
    Params { file: PathBuf, instance: u64 },
    /// Render the song to a 24-bit WAV file, loading its plugins.
    Export { file: PathBuf, out: PathBuf },
    /// Change a project file in place. Commands that create something print its id.
    Edit {
        file: PathBuf,
        #[command(subcommand)]
        op: Op,
    },
}

pub fn run(command: Command) -> ExitCode {
    match execute(command) {
        Ok(output) => {
            if !output.is_empty() {
                println!("{output}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn execute(command: Command) -> Result<String, String> {
    match command {
        Command::New { file, force } => {
            if file.exists() && !force {
                return Err(format!("{} exists; pass --force to replace it", file.display()));
            }
            let mut project = Project::new();
            if let Some(stem) = file.file_stem() {
                project.name = stem.to_string_lossy().into_owned();
            }
            save(&file, &project)?;
            Ok(String::new())
        }
        Command::Show { file, pattern, automation, json } => {
            let project = load(&file)?;
            show::show(&project, pattern, automation, json)
        }
        Command::Plugins { scan, json } => {
            let catalog = if scan {
                let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                scan::scan(&exe, &scan::cache_path(), |_| {})
            } else {
                scan::cached(&scan::cache_path())
            };
            show::plugins(&catalog, json)
        }
        Command::Params { file, instance } => params(&load(&file)?, instance),
        Command::Export { file, out } => {
            // Load here first: the app reports a bad file only in its status bar.
            load(&file)?;
            crate::app::export_cli(&file, &out)?;
            Ok(format!("exported {}", out.display()))
        }
        Command::Edit { file, op } => {
            let mut project = load(&file)?;
            let output = edit::apply(&mut project, op, || scan::cached(&scan::cache_path()))?;
            save(&file, &project)?;
            Ok(output)
        }
    }
}

fn load(path: &Path) -> Result<Project, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    Project::from_ron(&text).map_err(|e| format!("could not parse {}: {e}", path.display()))
}

/// Write through a temporary file so a failed write leaves the old project intact.
fn save(path: &Path, project: &Project) -> Result<(), String> {
    let text = project.to_ron().map_err(|e| e.to_string())?;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    std::fs::write(&temporary, text)
        .and_then(|_| std::fs::rename(&temporary, path))
        .map_err(|e| format!("could not write {}: {e}", path.display()))
}

fn params(project: &Project, instance: u64) -> Result<String, String> {
    let instance = project.plugin(daw_model::InstanceId(instance)).ok_or(format!("no plugin instance {instance}"))?;
    let loaded = daw_plugins::load(&instance.plugin, &instance.state, 48_000.0, daw_engine::MAX_BLOCK)
        .map_err(|e| format!("could not load {}: {e}", instance.plugin.name))?;
    let controller = loaded.controller;
    let lines: Vec<String> = controller
        .params()
        .iter()
        .map(|p| {
            let value = controller.param_value(p.id);
            let steps = if p.steps > 0 { format!("  steps {}", p.steps) } else { String::new() };
            let automatable = if p.automatable { "" } else { "  not automatable" };
            format!("{}  {:?}  value {value:.3} ({}){steps}{automatable}", p.id, p.name, controller.param_text(p.id, value))
        })
        .collect();
    Ok(lines.join("\n"))
}

/// Find an installed plugin by exact id or case-insensitive name.
fn find_plugin(catalog: &Catalog, query: &str, kind: PluginKind) -> Result<daw_model::PluginRef, String> {
    let matches: Vec<_> = match catalog.plugins.iter().find(|p| p.plugin.id == query) {
        Some(exact) => vec![exact],
        None => catalog.plugins.iter().filter(|p| p.plugin.name.eq_ignore_ascii_case(query)).collect(),
    };
    let kind_name = match kind {
        PluginKind::Instrument => "instrument",
        PluginKind::Effect => "effect",
    };
    match matches.as_slice() {
        [] if catalog.plugins.is_empty() => Err("the plugin cache is empty; run `daw plugins --scan`".into()),
        [] => Err(format!("no installed plugin named {query:?}; see `daw plugins`")),
        [found] if found.kind != kind => Err(format!("{} is not an {kind_name}", found.plugin.name)),
        [found] => Ok(found.plugin.clone()),
        _ => {
            let formats: Vec<String> = matches.iter().map(|p| format!("{} {}", p.plugin.format, p.plugin.id)).collect();
            Err(format!("{query:?} matches several plugins; pass an id instead: {}", formats.join(", ")))
        }
    }
}

/// Resolve a time against the project's signature.
fn ticks(project: &Project, time: Time) -> daw_model::time::Ticks {
    time.ticks(project.signature)
}
