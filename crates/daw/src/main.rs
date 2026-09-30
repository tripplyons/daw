mod app;
mod capture;
mod cli;
mod config;
mod keys;
mod menu;
mod open_files;
mod panels;
mod project_files;
mod midi;
mod quit;
mod session;
mod processing;
mod render;
mod theme;

use std::process::ExitCode;

use app::App;
use clap::Parser;

fn main() -> ExitCode {
    // Plugin scanning re-runs this executable as a child process per plugin.
    let args: Vec<String> = std::env::args().collect();
    if let Some(code) = daw_plugins::scan::run_child(&args) {
        std::process::exit(code);
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

fn run_app() -> iced::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    open_files::install();
    iced::application(App::boot, App::update, App::view)
        .title(App::title)
        .theme(|_: &App| theme::theme())
        .subscription(App::subscription)
        .settings(iced::Settings { default_text_size: theme::TEXT_SIZE.into(), ..iced::Settings::default() })
        .window_size(iced::Size::new(1400.0, 860.0))
        // Closing asks to save unsaved changes first; see `Message::CloseRequested`.
        .exit_on_close_request(false)
        .run()
}
