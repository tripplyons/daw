//! `daw batch`: many edits in one load and save.

use std::collections::HashMap;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use daw_model::Project;
use daw_plugins::scan::{self, Catalog};

use super::edit::{self, Op};
use super::plugins::{Plugins, parse_assignment};

#[derive(Parser)]
#[command(no_binary_name = true, name = "batch line")]
struct Line {
    #[command(subcommand)]
    command: LineCommand,
}

#[derive(Subcommand)]
enum LineCommand {
    #[command(flatten)]
    Edit(Op),
    /// Set plugin parameters, as `daw params` does.
    Params {
        instance: u64,
        #[arg(value_parser = parse_assignment, required = true)]
        set: Vec<(String, f32)>,
    },
    /// Load a factory preset or a state file, as `daw presets` does.
    Presets {
        instance: u64,
        #[arg(long, conflicts_with = "load_file", required_unless_present = "load_file")]
        load: Option<String>,
        #[arg(long)]
        load_file: Option<PathBuf>,
    },
}

/// Run a script against a project and return the text to print. Lines are
/// `daw edit` commands without `edit FILE`, plus `params` and `presets` lines.
/// `NAME = COMMAND` keeps the command's output, and `$NAME` uses it later.
/// Stops at the first failing line; the caller saves only on success.
pub fn run(project: &mut Project, script: &str) -> Result<String, String> {
    let mut variables: HashMap<String, String> = HashMap::new();
    let mut plugins = Plugins::default();
    let mut catalog: Option<Catalog> = None;
    let mut out = Vec::new();
    for (number, text) in script.lines().enumerate() {
        let fail = |e: String| format!("line {}: {e}\n  {}", number + 1, text.trim());
        let mut words = shell_words::split(text).map_err(|e| fail(e.to_string()))?;
        if words.first().is_none_or(|w| w.starts_with('#')) {
            continue;
        }
        let name = match words.get(1).map(String::as_str) {
            Some("=") if is_name(&words[0]) => {
                let name = words.remove(0);
                words.remove(0);
                Some(name)
            }
            _ => None,
        };
        let words = words.iter().map(|w| substitute(w, &variables)).collect::<Result<Vec<_>, _>>().map_err(fail)?;
        let line = Line::try_parse_from(&words).map_err(|e| fail(e.render().to_string().trim().to_string()))?;
        let output = match line.command {
            LineCommand::Edit(op) => {
                edit::apply(project, op, || catalog.get_or_insert_with(|| scan::cached(&scan::cache_path())).clone())
            }
            LineCommand::Params { instance, set } => plugins.set(project, instance, &set),
            LineCommand::Presets { instance, load: Some(preset), .. } => plugins.load_preset(project, instance, &preset),
            LineCommand::Presets { instance, load_file: Some(path), .. } => plugins.load_file(project, instance, &path),
            LineCommand::Presets { .. } => unreachable!("clap requires --load or --load-file"),
        }
        .map_err(fail)?;
        match name {
            Some(name) => {
                out.push(format!("{name} = {output}"));
                variables.insert(name, output);
            }
            None if !output.is_empty() => out.push(output),
            None => {}
        }
    }
    plugins.store(project)?;
    Ok(out.join("\n"))
}

fn is_name(word: &str) -> bool {
    word.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Replace `$NAME` and `${NAME}` with earlier results.
fn substitute(word: &str, variables: &HashMap<String, String>) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = word;
    while let Some(start) = rest.find('$') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let (name, next) = match after.strip_prefix('{') {
            Some(braced) => {
                let end = braced.find('}').ok_or(format!("unclosed ${{ in {word:?}"))?;
                (&braced[..end], &braced[end + 1..])
            }
            None => {
                let end = after.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(after.len());
                (&after[..end], &after[end..])
            }
        };
        if name.is_empty() {
            return Err(format!("stray $ in {word:?}"));
        }
        out.push_str(variables.get(name).ok_or(format!("${name} is not set"))?);
        rest = next;
    }
    out.push_str(rest);
    Ok(out)
}
