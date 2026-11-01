//! Fake scanner executables and injected discovery keep tests off installed plugins.

use super::*;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!("daw-probe-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn scanner(&self, body: &str) -> PathBuf {
        let path = self.0.join("scanner");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    fn successful_scanner(&self) -> PathBuf {
        let result: Result<Vec<PluginInfo>, String> = Ok(vec![info("probed")]);
        let payload = ron::ser::to_string(&result).unwrap();
        self.scanner(&format!("printf '.\\n' >> \"$0.calls\"\nprintf '%s\\n' '{MARKER}{payload}'"))
    }
    fn calls(&self) -> usize {
        std::fs::read_to_string(self.0.join("scanner.calls")).unwrap_or_default().lines().count()
    }
    fn cache(&self) -> PathBuf { self.0.join("cache.ron") }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

fn info(name: &str) -> PluginInfo {
    PluginInfo {
        plugin: daw_model::PluginRef { format: PluginFormat::Vst3, id: name.into(), path: format!("/fake/{name}.vst3"), name: name.into(), vendor: "test".into() },
        kind: crate::PluginKind::Instrument, category: "Instrument".into(),
    }
}
fn entry(key: &str, fingerprint: u64, failed: bool) -> Entry {
    Entry { key: key.into(), fingerprint, name: key.into(), format: PluginFormat::Vst3,
        result: if failed { Err("old failure".into()) } else { Ok(vec![info(key)]) } }
}
fn job(key: &str, fingerprint: u64) -> Job {
    Job { key: key.into(), fingerprint, name: key.into(), format: PluginFormat::Vst3, listed: None }
}

#[test]
fn startup_reuses_unchanged_successes_and_failures_and_discards_removed_plugins() {
    let fixture = Fixture::new();
    let exe = fixture.successful_scanner();
    save_cache(&fixture.cache(), &Cache { entries: vec![
        entry("good", 1, false), entry("bad", 1, true), entry("updated", 0, false),
        entry("updated-failure", 0, true), entry("removed", 1, false),
    ] });
    let progress = Mutex::new(Vec::new());
    let catalog = scan_jobs(&exe, &fixture.cache(), ScanMode::Changed, vec![
        job("good", 1), job("bad", 1), job("updated", 1), job("updated-failure", 1), job("new", 1),
    ], |p| progress.lock().unwrap().push(p));
    assert_eq!(fixture.calls(), 3);
    assert_eq!(catalog.plugins.len(), 4);
    assert_eq!(catalog.failed.len(), 1);
    assert_eq!(catalog.failed[0].name, "bad");
    assert_eq!(cached(&fixture.cache()), catalog);
    assert!(!load_cache(&fixture.cache()).entries.iter().any(|entry| entry.key == "removed"));
    let progress = progress.into_inner().unwrap();
    assert_eq!(progress.first(), Some(&Progress { done: 0, total: 3 }));
    assert_eq!(progress.last(), Some(&Progress { done: 3, total: 3 }));
}

#[test]
fn explicit_rescan_retries_failed_plugins_and_keeps_unchanged_successes() {
    let fixture = Fixture::new();
    let exe = fixture.successful_scanner();
    save_cache(&fixture.cache(), &Cache { entries: vec![entry("good", 1, false), entry("bad", 1, true)] });
    let catalog = scan_jobs(&exe, &fixture.cache(), ScanMode::RetryFailed,
        vec![job("good", 1), job("bad", 1)], |_| {});
    assert_eq!(fixture.calls(), 1);
    assert_eq!(catalog.plugins.len(), 2);
    assert!(catalog.failed.is_empty());
    assert_eq!(cached(&fixture.cache()), catalog);
    let startup = scan_jobs(&exe, &fixture.cache(), ScanMode::Changed,
        vec![job("good", 1), job("bad", 1)], |p| assert_eq!(p, Progress { done: 0, total: 0 }));
    assert_eq!(fixture.calls(), 1);
    assert_eq!(startup, catalog);
}

#[test]
fn failed_rescan_retains_exit_code_and_stderr_in_cache() {
    let fixture = Fixture::new();
    let exe = fixture.scanner("printf \"error: Unrecognized option: 'scan-vst3'\\n\" >&2\nexit 101");
    save_cache(&fixture.cache(), &Cache { entries: vec![entry("bad", 1, true)] });
    let catalog = scan_jobs(&exe, &fixture.cache(), ScanMode::RetryFailed, vec![job("bad", 1)], |_| {});
    assert!(catalog.plugins.is_empty());
    assert_eq!(catalog.failed.len(), 1);
    assert!(catalog.failed[0].error.contains("scanner exited with code 101"));
    assert!(catalog.failed[0].error.contains("Unrecognized option: 'scan-vst3'"));
    assert_eq!(cached(&fixture.cache()), catalog);
}

#[test]
fn chatty_plugins_cannot_block_either_output_pipe_or_hide_the_final_result() {
    let fixture = Fixture::new();
    let result: Result<Vec<PluginInfo>, String> = Ok(vec![info("chatty")]);
    let payload = ron::ser::to_string(&result).unwrap();
    let exe = fixture.scanner(&format!("i=0\nwhile [ \"$i\" -lt 2500 ]; do\n printf '%0100d' 0\n printf '%0100d' 0 >&2\n i=$((i + 1))\ndone\nprintf '\\n%s\\n' '{MARKER}{payload}'"));
    assert_eq!(run_probe(&exe, "--scan-vst3", "fake"), result);
}

#[test]
fn large_catalogs_are_not_truncated_with_plugin_diagnostics() {
    let fixture = Fixture::new();
    let result: Result<Vec<PluginInfo>, String> = Ok((0..200).map(|index| info(&format!("class-{index}"))).collect());
    let payload = ron::ser::to_string(&result).unwrap();
    assert!(payload.len() > OUTPUT_LIMIT);
    let exe = fixture.scanner(&format!("printf '%s\\n' '{MARKER}{payload}'"));
    assert_eq!(run_probe(&exe, "--scan-vst3", "fake"), result);
}

#[test]
fn missing_and_invalid_results_are_reported() {
    let fixture = Fixture::new();
    let exe = fixture.scanner("printf 'load warning\\n' >&2");
    let error = run_probe(&exe, "--scan-vst3", "fake").unwrap_err();
    assert!(error.contains("scanner produced no result"));
    assert!(error.contains("load warning"));
    let exe = fixture.scanner(&format!("printf '%s\\n' '{MARKER}invalid'"));
    assert!(run_probe(&exe, "--scan-vst3", "fake").unwrap_err().contains("bad scanner output"));
}

#[test]
fn output_capture_is_bounded_and_handles_non_utf8_plugin_logs() {
    let mut output = vec![0xff; OUTPUT_LIMIT * 4];
    output.extend_from_slice(b"final diagnostic");
    let captured = read_output(output.as_slice());
    assert!(captured.ends_with("final diagnostic"));
    assert!(captured.chars().count() <= OUTPUT_LIMIT);
}
