# DAW

A digital audio workstation (DAW) written in Rust with [iced](https://iced.rs). It hosts VST3 and Audio Unit plugins, arranges its panels as tiles the way an i3 or sway window manager does, and has FL Studio style patterns, a playlist, a mixer, and an automation editor.

It runs on macOS and Linux. Audio Units are macOS only. On Linux, plugin editor windows use X11, or XWayland on Wayland desktops.

## Requirements

- Rust 1.88 or newer (edition 2024), installed with [rustup](https://rustup.rs)
- macOS: Xcode Command Line Tools (`xcode-select --install`)
- Linux: the ALSA, X11, xkbcommon, and Wayland development packages. On Debian or Ubuntu:

```sh
sudo apt-get install build-essential pkg-config libasound2-dev libx11-dev libxkbcommon-dev libwayland-dev
```

On Linux, audio goes through ALSA, which PipeWire and PulseAudio also accept. File dialogs use the XDG desktop portal, or `zenity` when no portal is running. Without an output device the app still edits projects and exports WAV files.

## Run

```sh
git clone https://github.com/tripplyons/daw.git
cd daw
cargo run --release
```

The first build takes a few minutes. Debug builds also work (`cargo run`); the workspace compiles dependencies with optimizations so audio keeps up in either mode.

## Build a .app (macOS)

```sh
scripts/bundle-app.sh
open target/release/DAW.app
```

The script builds a release binary and wraps it in `target/release/DAW.app` with an ad-hoc code signature, which is enough to run it on your own Mac. Pass `--install` to also copy it to `/Applications`; it refuses to replace a different app already at that path.

The bundle declares the `.dawproj` file type, so double-clicking a project in Finder (or dropping one on the Dock icon) opens it in DAW. After `--install`, only the copy in `/Applications` handles these files.

## Plugins

On startup the app scans the standard plugin folders:

- macOS: `/Library/Audio/Plug-Ins/VST3`, `~/Library/Audio/Plug-Ins/VST3`, and every Audio Unit registered with macOS
- Linux: `/usr/lib/vst3`, `/usr/local/lib/vst3`, and `~/.vst3`

A Linux VST3 bundle needs a binary for your CPU, such as `Contents/x86_64-linux/` or `Contents/aarch64-linux/`. A project with Audio Units still opens on Linux; those plugins show a load error.

Each plugin is scanned in a separate process, so a plugin that crashes or hangs during the scan is listed as failed instead of closing the app. Results are cached in `~/Library/Application Support/daw/plugins.ron` on macOS and `~/.local/share/daw/plugins.ron` on Linux; later scans only probe new or updated plugins. The browser panel's "rescan" button scans again.

Click a plugin in the browser panel to use it: an instrument gets a new channel in the channel rack, and an effect goes on the selected mixer insert.

## Files

- Projects are saved as `.dawproj` files. Closing, starting a new project, or opening another one asks to save unsaved changes first.
- Key bindings are saved to `~/.config/daw/config.json`, or to `$XDG_CONFIG_HOME/daw/config.json` when that variable is an absolute path. Edit them on the settings page (Cmd+comma).

## Command line

With no command, `daw [project]` opens the app. Subcommands read and change project files without a window:

```sh
daw new song.dawproj
daw show song.dawproj                      # overview with ids
daw edit song.dawproj note add 10 9 E4 1beat 2step
daw edit song.dawproj clip add pattern:10 0 4bar
daw plugins                                # installed plugins from the scan cache
daw export song.dawproj song.wav
```

`daw --help` lists every command.

### Agent skill

The `daw-project` skill in `.claude/skills/daw-project` teaches coding agents to use these commands: the project model, value formats, and a worked example. Claude Code loads it automatically inside this repository. To use it in other folders, put `daw` on your PATH and install the skill for your user. From the repository root:

```sh
cargo install --path crates/daw
mkdir -p ~/.claude/skills
ln -s "$PWD/.claude/skills/daw-project" ~/.claude/skills/daw-project
```

The symlink keeps the skill in step with the repository; copy the folder instead if you want a fixed version. Other agents that read `SKILL.md` folders can use the same folder, for example by linking it into `~/.agents/skills`.

## Default keys

Alt is the tiling modifier. Keys match by physical position, so they work on any keyboard layout. On Linux, Ctrl replaces Cmd everywhere in this README. If your window manager already uses an Alt shortcut, rebind it there or in the settings page.

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

On macOS, bound keys also work while a plugin window is in front, as long as the plugin does not use the key itself.

## Scrolling

The piano roll, playlist, and automation editor share one scheme. Zooming keeps the spot under the cursor in place.

| Scroll with | Action |
| --- | --- |
| nothing | Scroll up and down (keys, tracks, or values) |
| Shift, or a sideways trackpad swipe | Scroll sideways in time |
| Cmd | Zoom time |
| Alt | Zoom height: key height, track height, or the automation value range |
| Alt+Shift over a note | Change the note's velocity |

The mixer strips scroll sideways with a plain scroll wheel.

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
