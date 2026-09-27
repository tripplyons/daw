//! Plugin discovery with crash isolation and a persistent cache.
//!
//! Each VST3 bundle and each Audio Unit is probed in a child process
//! (`<exe> --scan-vst3 <path>` / `<exe> --scan-au <id>`), so a plugin that
//! crashes or hangs while loading is recorded as failed instead of taking
//! down the app. Results are cached by path and modification time (VST3) or
//! component version (AU); only new or changed plugins are probed again.

use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, UNIX_EPOCH};

use daw_model::PluginFormat;
use serde::{Deserialize, Serialize};

use crate::{PluginInfo, vst3};
#[cfg(target_os = "macos")]
use crate::au;

const MARKER: &str = "@@daw-scan@@";
const TIMEOUT: Duration = Duration::from_secs(45);
const WORKERS: usize = 8;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    pub format: PluginFormat,
    pub name: String,
    pub error: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Catalog {
    pub plugins: Vec<PluginInfo>,
    pub failed: Vec<Failure>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    key: String,
    fingerprint: u64,
    name: String,
    format: PluginFormat,
    result: Result<Vec<PluginInfo>, String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    entries: Vec<Entry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
}

pub fn cache_path() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("daw").join("plugins.ron")
}

/// The standard VST3 folders: system-wide, then the user's.
pub fn vst3_dirs() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    let (system, user) = (&["/Library/Audio/Plug-Ins/VST3"][..], "Library/Audio/Plug-Ins/VST3");
    #[cfg(target_os = "linux")]
    let (system, user) = (&["/usr/lib/vst3", "/usr/local/lib/vst3"][..], ".vst3");
    let mut dirs: Vec<PathBuf> = system.iter().map(PathBuf::from).collect();
    dirs.extend(dirs::home_dir().map(|home| home.join(user)));
    dirs
}

/// All `.vst3` bundles under `root`, searching vendor subfolders but not
/// inside bundles.
pub fn find_vst3_bundles(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_owned()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if path.extension().is_some_and(|e| e == "vst3") {
                found.push(path);
            } else {
                pending.push(path);
            }
        }
    }
    found.sort();
    found
}

fn modified_seconds(path: &Path) -> u64 {
    // Bundle contents change on update; the executable's mtime is the best signal.
    let newest = std::fs::read_dir(vst3::binary_dir(path))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max();
    newest
        .or_else(|| std::fs::metadata(path).ok()?.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn load_cache(path: &Path) -> Cache {
    let Ok(text) = std::fs::read_to_string(path) else { return Cache::default() };
    ron::from_str(&text).unwrap_or_else(|error| {
        log::warn!("ignoring unreadable plugin cache {}: {error}", path.display());
        Cache::default()
    })
}

fn save_cache(path: &Path, cache: &Cache) {
    let result = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))
        .map_err(|e| e.to_string())
        .and_then(|_| ron::ser::to_string(cache).map_err(|e| e.to_string()))
        .and_then(|text| std::fs::write(path, text).map_err(|e| e.to_string()));
    if let Err(error) = result {
        log::warn!("could not write plugin cache {}: {error}", path.display());
    }
}

struct Job {
    key: String,
    fingerprint: u64,
    name: String,
    format: PluginFormat,
    /// For AU the listing already produced the info; the probe only checks it loads.
    listed: Option<PluginInfo>,
}

/// Scan installed plugins, reusing cached results for unchanged ones.
/// `exe` is a binary that handles the `--scan-*` arguments via [`run_child`].
pub fn scan(exe: &Path, cache_file: &Path, progress: impl Fn(Progress) + Sync) -> Catalog {
    let mut cache = load_cache(cache_file);
    let cached: HashMap<String, Entry> = cache.entries.drain(..).map(|e| (e.key.clone(), e)).collect();

    let mut jobs = Vec::new();
    for bundle in vst3_dirs().iter().flat_map(|d| find_vst3_bundles(d)) {
        let name = bundle.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        jobs.push(Job {
            key: bundle.to_string_lossy().into_owned(),
            fingerprint: modified_seconds(&bundle),
            name,
            format: PluginFormat::Vst3,
            listed: None,
        });
    }
    #[cfg(target_os = "macos")]
    for (info, version) in au::list() {
        jobs.push(Job {
            key: info.plugin.id.clone(),
            fingerprint: version,
            name: info.plugin.name.clone(),
            format: PluginFormat::AudioUnit,
            listed: Some(info),
        });
    }

    let mut entries = Vec::new();
    let mut pending = VecDeque::new();
    for job in jobs {
        match cached.get(&job.key) {
            Some(entry) if entry.fingerprint == job.fingerprint => entries.push(entry.clone()),
            _ => pending.push_back(job),
        }
    }
    let total = pending.len();
    log::info!("plugin scan: {} cached, {total} to probe", entries.len());
    let queue = Mutex::new(pending);
    let results = Mutex::new(Vec::new());
    let done = Mutex::new(0);
    progress(Progress { done: 0, total });
    std::thread::scope(|scope| {
        for _ in 0..WORKERS {
            scope.spawn(|| {
                loop {
                    let Some(job) = queue.lock().unwrap().pop_front() else { break };
                    let entry = probe(exe, job);
                    results.lock().unwrap().push(entry);
                    let mut done = done.lock().unwrap();
                    *done += 1;
                    progress(Progress { done: *done, total });
                }
            });
        }
    });
    entries.extend(results.into_inner().unwrap());
    entries.sort_by(|a, b| a.key.cmp(&b.key));

    let mut catalog = Catalog::default();
    for entry in &entries {
        match &entry.result {
            Ok(plugins) => catalog.plugins.extend(plugins.iter().cloned()),
            Err(error) => catalog.failed.push(Failure { format: entry.format, name: entry.name.clone(), error: error.clone() }),
        }
    }
    catalog.plugins.sort_by_key(|p| p.plugin.name.to_lowercase());
    cache.entries = entries;
    save_cache(cache_file, &cache);
    catalog
}

fn probe(exe: &Path, job: Job) -> Entry {
    let flag = match job.format {
        PluginFormat::Vst3 => "--scan-vst3",
        PluginFormat::AudioUnit => "--scan-au",
    };
    let result = run_probe(exe, flag, &job.key).and_then(|found| match job.listed.clone() {
        Some(listed) => Ok(vec![listed]),
        None if found.is_empty() => Err("no audio processor classes".into()),
        None => Ok(found),
    });
    if let Err(error) = &result {
        log::warn!("plugin {} failed: {error}", job.name);
    }
    Entry { key: job.key, fingerprint: job.fingerprint, name: job.name, format: job.format, result }
}

fn run_probe(exe: &Path, flag: &str, key: &str) -> Result<Vec<PluginInfo>, String> {
    let mut child = Command::new(exe)
        .args([flag, key])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start scanner: {e}"))?;
    // Read stdout on a thread so a chatty plugin cannot fill the pipe and stall.
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {}s", TIMEOUT.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => return Err(error.to_string()),
        }
    };
    let output = reader.join().unwrap_or_default();
    let payload = output.lines().find_map(|line| line.strip_prefix(MARKER));
    match payload {
        Some(payload) => ron::from_str::<Result<Vec<PluginInfo>, String>>(payload).map_err(|e| format!("bad scanner output: {e}"))?,
        None if status.success() => Err("scanner produced no result".into()),
        None => Err(match status.code() {
            Some(code) => format!("scanner exited with code {code}"),
            None => "plugin crashed while loading".into(),
        }),
    }
}

/// Handle scanner arguments in a child process. Returns `None` when the
/// arguments are not a scan request; otherwise the process exit code.
pub fn run_child(args: &[String]) -> Option<i32> {
    let (flag, key) = (args.get(1)?, args.get(2)?);
    let result: Result<Vec<PluginInfo>, String> = match flag.as_str() {
        "--scan-vst3" => probe_vst3(Path::new(key)),
        #[cfg(target_os = "macos")]
        "--scan-au" => probe_au(key),
        _ => return None,
    };
    let text = ron::ser::to_string(&result).unwrap_or_else(|e| format!("Err(\"{e}\")"));
    println!("{MARKER}{text}");
    Some(0)
}

fn probe_vst3(path: &Path) -> Result<Vec<PluginInfo>, String> {
    let plugins = vst3::scan_bundle(path).map_err(|e| e.to_string())?;
    // Instantiate each class so broken plugins are caught here, not in a session.
    for info in &plugins {
        crate::load(&info.plugin, &[], 48_000.0, 512).map_err(|e| format!("{}: {e}", info.plugin.name))?;
    }
    Ok(plugins)
}

#[cfg(target_os = "macos")]
fn probe_au(id: &str) -> Result<Vec<PluginInfo>, String> {
    let (info, _) = au::list().into_iter().find(|(info, _)| info.plugin.id == id).ok_or_else(|| format!("{id} not registered"))?;
    crate::load(&info.plugin, &[], 48_000.0, 512).map_err(|e| e.to_string())?;
    Ok(vec![info])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_bundles_in_vendor_folders_only_once() {
        let root = std::env::temp_dir().join(format!("daw-scan-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["A.vst3/Contents/MacOS", "Vendor/B.vst3/Contents", "Vendor/B.vst3/Contents/Inner.vst3", "Empty"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        let found = find_vst3_bundles(&root);
        let names: Vec<_> = found.iter().map(|p| p.strip_prefix(&root).unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["A.vst3", "Vendor/B.vst3"]);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
