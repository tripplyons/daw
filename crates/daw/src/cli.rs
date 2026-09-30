//! Command-line interface: open the app, or inspect, edit, and render
//! project files without a window.

mod analyze;
mod batch;
mod edit;
mod parse;
mod plugins;
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
use plugins::Plugins;

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
    /// Bundle the project and referenced WAV files into a portable .dawzip archive.
    Pack { file: PathBuf, out: PathBuf },
    /// Extract a portable project into a new folder.
    Unpack { archive: PathBuf, folder: PathBuf },
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
    /// List a plugin's parameters, or set some and save them into the project.
    /// INSTANCE is a plugin instance id or the id of a plugin channel.
    Params {
        file: PathBuf,
        instance: u64,
        /// PARAM=VALUE, where PARAM is an id or a case-insensitive name and VALUE is normalized 0..1.
        #[arg(value_parser = plugins::parse_assignment)]
        set: Vec<(String, f32)>,
        /// List only parameters whose name contains this text, ignoring case.
        #[arg(long, conflicts_with = "set")]
        find: Option<String>,
        /// Show the text the plugin displays across one parameter's range, to find
        /// the normalized value for a setting such as 200 Hz or "High pass".
        #[arg(long, conflicts_with_all = ["set", "find"])]
        describe: Option<String>,
    },
    /// List a plugin's factory presets, or load one into the project (Audio Units).
    /// INSTANCE is a plugin instance id or the id of a plugin channel.
    Presets {
        file: PathBuf,
        instance: u64,
        /// List only presets whose name contains this text, ignoring case.
        #[arg(long, conflicts_with = "load")]
        find: Option<String>,
        /// Preset to load, by index or case-insensitive name. Replaces the plugin's settings.
        #[arg(long, conflicts_with = "load_file")]
        load: Option<String>,
        /// Load a JUCE plugin's own state or preset file, such as a Vital .vital
        /// preset, into its Audio Unit. Replaces the plugin's settings.
        #[arg(long)]
        load_file: Option<PathBuf>,
    },
    /// Render the song to a 24-bit WAV file, loading its plugins.
    Export {
        file: PathBuf,
        out: PathBuf,
        /// Render only this span, e.g. 64bar..72bar, plus the chosen tail. Notes that
        /// start before it are not heard.
        #[arg(long, value_parser = parse::range)]
        range: Option<(Time, Time)>,
        /// Also write each mixer insert's output to its own WAV in this folder,
        /// from the same render.
        #[arg(long)]
        stems: Option<PathBuf>,
        /// Effect tail in seconds (0 to 120). Defaults to the project's export tail.
        #[arg(long, value_parser = |t: &str| parse::bounded(t, 0.0, 120.0))]
        tail: Option<f64>,
    },
    /// Print peak, RMS, stereo width, and octave-band levels of WAV files, such
    /// as an export and its stems.
    Analyze {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Also print a row per section of this many bars; needs --bpm.
        #[arg(long, requires = "bpm")]
        bars: Option<f64>,
        #[arg(long)]
        bpm: Option<f64>,
    },
    /// Change a project file in place. Commands that create something print its id.
    Edit {
        file: PathBuf,
        #[command(subcommand)]
        op: Op,
    },
    /// Apply a script of edits with one load and save; the file changes only
    /// if every line succeeds.
    ///
    /// Each line is what follows `daw edit FILE`, or `params INSTANCE NAME=VALUE...`,
    /// or `presets INSTANCE --load NAME | --load-file PATH`. Plugins load once
    /// per batch. `NAME = COMMAND` keeps the command's output (such as a new id)
    /// and `$NAME` uses it on later lines. Words are split like a shell's, and
    /// `#` starts a comment line. Example:
    ///
    ///   lead = channel add lead --plugin Vital
    ///   presets $lead --load-file "Pads/Warm.vital"
    ///   note add 10 $lead C4 0 1beat
    Batch {
        file: PathBuf,
        /// Script file; reads standard input when omitted.
        script: Option<PathBuf>,
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
        Command::Pack { file, out } => {
            crate::project_files::pack(&out, &load(&file)?)?;
            Ok(format!("packaged {}", out.display()))
        }
        Command::Unpack { archive, folder } => {
            let path = crate::project_files::unpack(&archive, &folder)?;
            Ok(path.display().to_string())
        }
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
        Command::Params { file, instance, describe: Some(param), .. } => {
            Plugins::default().describe(&load(&file)?, instance, &param)
        }
        Command::Params { file, instance, set, find, .. } if set.is_empty() => {
            Plugins::default().list(&load(&file)?, instance, find.as_deref())
        }
        Command::Params { file, instance, set, .. } => {
            let mut project = load(&file)?;
            let mut plugins = Plugins::default();
            let output = plugins.set(&project, instance, &set)?;
            plugins.store(&mut project)?;
            save(&file, &project)?;
            Ok(output)
        }
        Command::Presets { file, instance, find, load: None, load_file: None } => {
            Plugins::default().presets(&load(&file)?, instance, find.as_deref())
        }
        Command::Presets { file, instance, load: preset, load_file, .. } => {
            let mut project = load(&file)?;
            let mut plugins = Plugins::default();
            let output = match (preset, load_file) {
                (Some(preset), _) => plugins.load_preset(&project, instance, &preset)?,
                (None, Some(path)) => plugins.load_file(&project, instance, &path)?,
                (None, None) => unreachable!("handled above"),
            };
            plugins.store(&mut project)?;
            save(&file, &project)?;
            Ok(output)
        }
        Command::Export { file, out, range, stems, tail } => {
            // Load here first: the app reports a bad file only in its status bar.
            let project = load(&file)?;
            let range = match range {
                Some((start, end)) if ticks(&project, end) <= ticks(&project, start) => {
                    return Err("the range must end after it starts".into());
                }
                Some((start, end)) => Some((ticks(&project, start), ticks(&project, end))),
                None => None,
            };
            crate::app::export_cli(&file, &out, range, stems.as_deref(), tail)?;
            Ok(format!("exported {}", out.display()))
        }
        Command::Analyze { files, bars, bpm } => {
            let section = bars.zip(bpm).map(|(bars, bpm)| bars * 240.0 / bpm);
            analyze::analyze(&files, section)
        }
        Command::Edit { file, op } => {
            let mut project = load(&file)?;
            let output = edit::apply(&mut project, op, || scan::cached(&scan::cache_path()))?;
            save(&file, &project)?;
            Ok(output)
        }
        Command::Batch { file, script } => {
            let script = match script {
                Some(path) => std::fs::read_to_string(&path).map_err(|e| format!("could not read {}: {e}", path.display()))?,
                None => std::io::read_to_string(std::io::stdin()).map_err(|e| format!("could not read standard input: {e}"))?,
            };
            let mut project = load(&file)?;
            let output = batch::run(&mut project, &script)?;
            save(&file, &project)?;
            Ok(output)
        }
    }
}

fn load(path: &Path) -> Result<Project, String> {
    crate::project_files::load(path)
}

/// Save the project and its audio in one archive through a temporary file.
fn save(path: &Path, project: &Project) -> Result<(), String> {
    crate::project_files::save(path, project)
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
