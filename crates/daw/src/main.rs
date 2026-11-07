mod app;
mod cli;
mod config;
mod input;
mod keys;
mod menu;
mod midi;
mod native_menu;
mod open_files;
mod palette;
mod panels;
mod processing;
mod project_files;
mod render;
mod session;
mod task;
mod theme;
mod ui;
mod units;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    // `bazel run` starts in the runfiles directory; resolve relative paths from where it was invoked.
    if let Some(dir) = std::env::var_os("BUILD_WORKING_DIRECTORY") {
        let _ = std::env::set_current_dir(dir);
    }
    // Plugin scanning re-runs this executable as a child process per plugin.
    let args: Vec<String> = std::env::args().collect();
    if let Some(code) = daw_plugins::scan::run_child(&args) {
        std::process::exit(code);
    }
    // Rune's headless review tools: `daw --snap out.png --scene demo`.
    if ui::headless_args(&args) {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
        return match ui::run(true) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let cli = cli::Cli::parse();
    if let Some(command) = cli.command {
        return cli::run(command);
    }
    let _ = app::STARTUP_PROJECT.set(cli.project);
    match run_app() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_app() -> Result<(), String> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    open_files::install();
    ui::run(false)
}
