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
    /// List a plugin instance's parameters, or set some and save them into the project.
    Params {
        file: PathBuf,
        instance: u64,
        /// PARAM=VALUE, where PARAM is an id or a case-insensitive name and VALUE is normalized 0..1.
        #[arg(value_parser = parse_assignment)]
        set: Vec<(String, f32)>,
        /// List only parameters whose name contains this text, ignoring case.
        #[arg(long, conflicts_with = "set")]
        find: Option<String>,
        /// Show the text the plugin displays across one parameter's range, to find
        /// the normalized value for a setting such as 200 Hz or "High pass".
        #[arg(long, conflicts_with_all = ["set", "find"])]
        describe: Option<String>,
    },
    /// List a plugin instance's factory presets, or load one into the project (Audio Units).
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
        Command::Params { file, instance, describe: Some(param), .. } => describe_param(&load(&file)?, instance, &param),
        Command::Params { file, instance, set, find, .. } if set.is_empty() => params(&load(&file)?, instance, find.as_deref()),
        Command::Params { file, instance, set, .. } => {
            let mut project = load(&file)?;
            let output = set_params(&mut project, instance, &set)?;
            save(&file, &project)?;
            Ok(output)
        }
        Command::Presets { file, instance, find, load: None, load_file: None } => presets(&load(&file)?, instance, find.as_deref()),
        Command::Presets { file, instance, load: preset, load_file, .. } => {
            let mut project = load(&file)?;
            let output = match (preset, load_file) {
                (Some(preset), _) => load_preset(&mut project, instance, &preset)?,
                (None, Some(path)) => load_state_file(&mut project, instance, &path)?,
                (None, None) => unreachable!("handled above"),
            };
            save(&file, &project)?;
            Ok(output)
        }
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

fn params(project: &Project, instance: u64, find: Option<&str>) -> Result<String, String> {
    let (_, loaded) = load_plugin(project, instance)?;
    let controller = loaded.controller;
    let lines: Vec<String> = controller
        .params()
        .iter()
        .filter(|p| find.is_none_or(|f| contains_ignore_case(&p.name, f)))
        .map(|p| param_line(&*controller, p))
        .collect();
    Ok(lines.join("\n"))
}

fn describe_param(project: &Project, instance: u64, query: &str) -> Result<String, String> {
    let (_, loaded) = load_plugin(project, instance)?;
    let params = loaded.controller.params();
    let param = find_param(&params, query)?;
    // Discrete parameters show each step; continuous ones every 0.05.
    let count = if param.steps > 0 && param.steps <= 64 { param.steps } else { 20 };
    let mut lines = vec![format!("{}  {:?}", param.id, param.name)];
    for i in 0..=count {
        let value = i as f32 / count as f32;
        lines.push(format!("  {value:.4}  {}", loaded.controller.param_text(param.id, value)));
    }
    Ok(lines.join("\n"))
}

fn presets(project: &Project, instance: u64, find: Option<&str>) -> Result<String, String> {
    let (plugin, loaded) = load_plugin(project, instance)?;
    let names = loaded.controller.presets();
    if names.is_empty() {
        return Err(format!("{} has no factory presets the host can load", plugin.name));
    }
    let lines: Vec<String> = names
        .iter()
        .enumerate()
        .filter(|(_, name)| find.is_none_or(|f| contains_ignore_case(name, f)))
        .map(|(index, name)| format!("{index}  {name:?}"))
        .collect();
    Ok(lines.join("\n"))
}

fn load_preset(project: &mut Project, instance: u64, preset: &str) -> Result<String, String> {
    let (plugin, mut loaded) = load_plugin(project, instance)?;
    let names = loaded.controller.presets();
    let index = match preset.parse::<usize>() {
        Ok(index) => index,
        Err(_) => {
            let matches: Vec<usize> = (0..names.len()).filter(|&i| names[i].eq_ignore_ascii_case(preset)).collect();
            match matches.as_slice() {
                [index] => *index,
                [] => return Err(format!("{} has no preset named {preset:?}; list them with `daw presets`", plugin.name)),
                _ => return Err(format!("several presets are named {preset:?}; use an index")),
            }
        }
    };
    loaded.controller.load_preset(index).map_err(|e| e.to_string())?;
    let state = loaded.controller.save_state().map_err(|e| format!("could not save {} state: {e}", plugin.name))?;
    if let Some(instance) = project.plugin_mut(daw_model::InstanceId(instance)) {
        instance.state = state;
    }
    Ok(format!("loaded preset {index} {:?}", names.get(index).map_or("", String::as_str)))
}

fn load_state_file(project: &mut Project, instance: u64, path: &Path) -> Result<String, String> {
    let juce = std::fs::read(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let (plugin, loaded) = load_plugin(project, instance)?;
    let current = loaded.controller.save_state().map_err(|e| format!("could not save {} state: {e}", plugin.name))?;
    drop(loaded);
    let state = daw_plugins::replace_juce_state(&plugin, &current, &juce).map_err(|e| e.to_string())?;
    // Load the new state and save it again, so the plugin checks it and the
    // project stores the plugin's own encoding.
    let loaded = daw_plugins::load(&plugin, &state, 48_000.0, daw_engine::MAX_BLOCK)
        .map_err(|e| format!("{} rejected {}: {e}", plugin.name, path.display()))?;
    let state = loaded.controller.save_state().map_err(|e| format!("could not save {} state: {e}", plugin.name))?;
    if let Some(instance) = project.plugin_mut(daw_model::InstanceId(instance)) {
        instance.state = state;
    }
    Ok(format!("loaded {}", path.display()))
}

fn contains_ignore_case(text: &str, part: &str) -> bool {
    text.to_lowercase().contains(&part.to_lowercase())
}

/// Set parameters and store the plugin's new state in the project. Values go
/// through the processor as well as the controller, because VST3 saves the
/// processor's copy.
fn set_params(project: &mut Project, instance: u64, set: &[(String, f32)]) -> Result<String, String> {
    let (plugin, mut loaded) = load_plugin(project, instance)?;
    let params = loaded.controller.params();
    let mut events = Vec::new();
    for (name, value) in set {
        let param = find_param(&params, name)?;
        events.push(daw_engine::Event { offset: 0, kind: daw_engine::EventKind::Param { id: param.id, value: *value } });
        loaded.controller.set_param(param.id, *value);
    }
    let transport = daw_engine::TransportInfo {
        sample_rate: 48_000.0,
        bpm: project.bpm,
        beats: 0.0,
        frames: 0,
        playing: false,
        numerator: project.signature.numerator,
        denominator: project.signature.denominator,
        bar_start: 0.0,
    };
    let (mut left, mut right) = (vec![0.0; 64], vec![0.0; 64]);
    loaded.processor.process(&transport, &events, &mut left, &mut right);
    let state = loaded.controller.save_state().map_err(|e| format!("could not save {} state: {e}", plugin.name))?;
    if let Some(instance) = project.plugin_mut(daw_model::InstanceId(instance)) {
        instance.state = state;
    }
    let lines: Vec<String> = set
        .iter()
        .filter_map(|(name, _)| find_param(&params, name).ok())
        .map(|p| param_line(&*loaded.controller, p))
        .collect();
    Ok(lines.join("\n"))
}

fn load_plugin(project: &Project, instance: u64) -> Result<(daw_model::PluginRef, daw_plugins::Loaded), String> {
    let instance = project.plugin(daw_model::InstanceId(instance)).ok_or(format!("no plugin instance {instance}"))?;
    let loaded = daw_plugins::load(&instance.plugin, &instance.state, 48_000.0, daw_engine::MAX_BLOCK)
        .map_err(|e| format!("could not load {}: {e}", instance.plugin.name))?;
    Ok((instance.plugin.clone(), loaded))
}

fn param_line(controller: &dyn daw_plugins::Controller, p: &daw_plugins::ParamInfo) -> String {
    let value = controller.param_value(p.id);
    let steps = if p.steps > 0 { format!("  steps {}", p.steps) } else { String::new() };
    let automatable = if p.automatable { "" } else { "  not automatable" };
    format!("{}  {:?}  value {value:.3} ({}){steps}{automatable}", p.id, p.name, controller.param_text(p.id, value))
}

fn find_param<'a>(params: &'a [daw_plugins::ParamInfo], query: &str) -> Result<&'a daw_plugins::ParamInfo, String> {
    if let Some(param) = query.parse::<u32>().ok().and_then(|id| params.iter().find(|p| p.id == id)) {
        return Ok(param);
    }
    let matches: Vec<_> = params.iter().filter(|p| p.name.eq_ignore_ascii_case(query)).collect();
    match matches.as_slice() {
        [param] => Ok(param),
        [] => Err(format!("no parameter {query:?}; list them with `daw params FILE INSTANCE`")),
        _ => Err(format!("several parameters are named {query:?}; use an id")),
    }
}

fn parse_assignment(text: &str) -> Result<(String, f32), String> {
    let (name, value) = text.rsplit_once('=').ok_or(format!("expected PARAM=VALUE, got {text:?}"))?;
    Ok((name.to_string(), parse::unit(value)?))
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
