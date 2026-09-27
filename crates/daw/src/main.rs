mod app;
mod capture;
mod config;
mod keys;
mod open_files;
mod panels;
mod quit;
mod session;
mod theme;

use app::App;

fn main() -> iced::Result {
    // Plugin scanning re-runs this executable as a child process per plugin.
    let args: Vec<String> = std::env::args().collect();
    if let Some(code) = daw_plugins::scan::run_child(&args) {
        std::process::exit(code);
    }
    if args.get(1).map(String::as_str) == Some("--export") {
        let (Some(project), Some(out)) = (args.get(2), args.get(3)) else {
            eprintln!("usage: daw --export <project> <file.wav>");
            std::process::exit(2);
        };
        std::process::exit(app::export_cli(project, out));
    }
    let project = args.get(1).filter(|a| !a.starts_with('-')).map(std::path::PathBuf::from);
    let _ = app::STARTUP_PROJECT.set(project);
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
