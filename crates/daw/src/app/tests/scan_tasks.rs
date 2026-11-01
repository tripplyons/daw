use super::*;

#[test]
fn discarded_scan_tasks_do_not_start_workers_or_call_the_scanner() {
    let (sender, receiver) = std::sync::mpsc::channel();
    let task = scan_task_with(move |_| {
        sender.send(()).unwrap();
        Catalog::default()
    });
    drop(task);
    // Dropping a deferred task drops the closure and its sender. An eagerly
    // spawned worker would instead send a value (or keep the channel open).
    assert_eq!(receiver.recv_timeout(Duration::from_secs(1)), Err(std::sync::mpsc::RecvTimeoutError::Disconnected));
}

#[test]
fn scan_results_update_browser_state_after_explicit_rescan() {
    let mut app = app();
    app.scan = None;
    let task = app.update(Message::Rescan);
    assert_eq!(app.scan, Some(Progress { done: 0, total: 0 }));
    drop(task);
    // ScanDone, not constructing the task, updates the browser catalog.
    let catalog = Catalog::default();
    let _ = app.update(Message::ScanDone(catalog.clone()));
    assert!(app.scan.is_none());
    assert_eq!(app.catalog, catalog);
}
