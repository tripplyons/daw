//! Cmd+Q normally calls `terminate:`, which quits without asking the window.
//! Point the Quit menu item at the main window's `performClose:` instead, so
//! quitting goes through the same close request (and save prompt) as the
//! close button.

/// Returns true once the menu item is rerouted; call again until it is.
#[cfg(target_os = "macos")]
pub fn route_to_close() -> bool {
    use objc2::{MainThreadMarker, sel};
    use objc2_app_kit::NSApplication;

    let Some(mtm) = MainThreadMarker::new() else { return false };
    let app = NSApplication::sharedApplication(mtm);
    // The app window is winit's; plugin editors are plain NSWindows.
    let Some(window) = app.windows().iter().find(|w| w.class().name().to_str().is_ok_and(|n| n.contains("Winit"))) else {
        return false;
    };
    let Some(menu) = app.mainMenu() else { return false };
    for top in menu.itemArray().iter() {
        let Some(submenu) = top.submenu() else { continue };
        for item in submenu.itemArray().iter() {
            if item.action() == Some(sel!(terminate:)) {
                // The window outlives the menu item's use of it: both last until exit.
                unsafe {
                    item.setTarget(Some(&window));
                    item.setAction(Some(sel!(performClose:)));
                }
                return true;
            }
        }
    }
    false
}

#[cfg(not(target_os = "macos"))]
pub fn route_to_close() -> bool {
    true
}
