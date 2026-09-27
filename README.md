# DAW

A digital audio workstation (DAW) written in Rust with [iced](https://iced.rs). It hosts VST3 and Audio Unit plugins, arranges its panels as tiles the way an i3 or sway window manager does, and has FL Studio style patterns, a playlist, a mixer, and an automation editor.

It runs on macOS only, because plugin hosting and editor windows use AppKit and Audio Unit APIs.

## Requirements

- macOS on Apple silicon or Intel
- Xcode Command Line Tools: `xcode-select --install`
- Rust 1.88 or newer (edition 2024), installed with [rustup](https://rustup.rs)

## Run

```sh
git clone https://github.com/tripplyons/daw.git
cd daw
cargo run --release
```

The first build takes a few minutes. Debug builds also work (`cargo run`); the workspace compiles dependencies with optimizations so audio keeps up in either mode.

## Plugins

On startup the app scans the standard plugin folders:

- `/Library/Audio/Plug-Ins/VST3` and `~/Library/Audio/Plug-Ins/VST3`
- every Audio Unit registered with macOS

Each plugin is scanned in a separate process, so a plugin that crashes or hangs during the scan is listed as failed instead of closing the app. Results are cached in `~/Library/Application Support/daw/plugins.ron`; later scans only probe new or updated plugins. The browser panel's "rescan" button scans again.

Click a plugin in the browser panel to use it: an instrument gets a new channel in the channel rack, and an effect goes on the selected mixer insert.

## Files

- Projects are saved as `.dawproj` files.
- Key bindings are saved to `~/.config/daw/config.json`, or to `$XDG_CONFIG_HOME/daw/config.json` when that variable is an absolute path. Edit them on the settings page (Cmd+comma).

## Default keys

Alt is the tiling modifier. Keys match by physical position, so they work on any keyboard layout.

| Keys | Action |
| --- | --- |
| Alt+H/J/K/L or Alt+arrows | Move focus between tiles |
| Alt+Shift+H/J/K/L | Move the focused tile |
| Ctrl+Alt+H/J/K/L | Resize the focused tile |
| Alt+Enter | Split the focused tile |
| Alt+V / Alt+B | Next split stacked / side by side |
| Alt+Space | Change the focused tile's panel |
| Alt+F | Focus mode (one tile fills the window) |
| Alt+Q | Close the focused tile |
| Alt+1 to Alt+9 | Switch workspace |
| Space | Play or pause |
| Escape | Stop and rewind |
| Alt+S | Switch between pattern and song mode |
| Alt+A | Bind mode: the next control you touch gets an automation clip |
| Alt+R | Record automation |
| Delete | Delete the selection in the focused panel |
| Cmd+Z / Cmd+Shift+Z | Undo / redo |
| Cmd+N / Cmd+O / Cmd+S / Cmd+Shift+S | New / open / save / save as |
| Cmd+E | Export WAV |
| Cmd+comma | Settings |

Bound keys also work while a plugin window is in front, as long as the plugin does not use the key itself.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
```

The workspace has four crates:

- `daw-model`: the project data (patterns, playlist, mixer, automation, layout)
- `daw-engine`: real-time audio rendering and the built-in synth
- `daw-plugins`: VST3 and Audio Unit scanning, loading, and editor windows
- `daw`: the iced app

## License

MIT. See [LICENSE](LICENSE).
