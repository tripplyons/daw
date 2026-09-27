//! Run a full plugin scan with this binary as the child scanner.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(code) = daw_plugins::scan::run_child(&args) {
        std::process::exit(code);
    }
    let exe = std::env::current_exe().unwrap();
    let cache = std::env::temp_dir().join("daw-scan-example.ron");
    if args.get(1).is_some_and(|a| a == "--fresh") {
        let _ = std::fs::remove_file(&cache);
    }
    let started = std::time::Instant::now();
    let catalog = daw_plugins::scan::scan(&exe, &cache, |p| {
        if p.done % 50 == 0 || p.done == p.total {
            eprintln!("{}/{}", p.done, p.total);
        }
    });
    let count = |format| catalog.plugins.iter().filter(|p| p.plugin.format == format).count();
    println!(
        "{:.1}s: {} VST3, {} AU, {} failed",
        started.elapsed().as_secs_f64(),
        count(daw_model::PluginFormat::Vst3),
        count(daw_model::PluginFormat::AudioUnit),
        catalog.failed.len()
    );
    for failure in &catalog.failed {
        println!("  failed {} {}: {}", failure.format, failure.name, failure.error);
    }
}
