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

The bundle declares the `.dawproj` file type, so double-clicking a project in Finder (or dropping one on the Dock icon) opens it in DAW. After `--install`, only the copy in `/Applications` handles these files. It also carries the microphone usage text that macOS shows before audio recording.

## Plugins

On startup the app scans the standard plugin folders:

- macOS: `/Library/Audio/Plug-Ins/VST3`, `~/Library/Audio/Plug-Ins/VST3`, and every Audio Unit registered with macOS
- Linux: `/usr/lib/vst3`, `/usr/local/lib/vst3`, and `~/.vst3`

A Linux VST3 bundle needs a binary for your CPU, such as `Contents/x86_64-linux/` or `Contents/aarch64-linux/`. A project with Audio Units still opens on Linux; those plugins show a load error.

Each plugin is scanned in a separate process, so a plugin that crashes or hangs during the scan is listed as failed instead of closing the app. Results are cached in `~/Library/Application Support/daw/plugins.ron` on macOS and `~/.local/share/daw/plugins.ron` on Linux; later scans only probe new or updated plugins. The browser panel's "rescan" button scans again.

Click a plugin in the browser panel to use it: an instrument gets a new channel in the channel rack, and an effect goes on the selected mixer insert.

## Audio clips

Audio clips play part of a WAV file at its own speed, without following tempo changes. Each file gets an audio channel in the channel rack, which sets its volume, pan, and mixer insert; double-click an audio clip to select its channel.

- Import: press Cmd+I, click "+ audio" in the playlist toolbar, or drop WAV files on the window. The clip starts at the song start marker on the first free track.
- Record: press Alt+Shift+R or click "rec audio" to arm the default input device, then play in song mode. Each stretch of playback becomes a take, saved as `recordings/take N.wav` next to the project, or in the app's data folder for an unsaved project, and placed on the first free track. Input latency is compensated. Click "rec audio" again to disarm. The app asks for microphone access the first time.
- Edit: drag a clip's left or right edge to trim it, Alt-click to split it, and Cmd+C, Cmd+V, and Cmd+D to copy, paste, and duplicate it. After an import or a take, the playlist brush is that file, so a click places the whole file again.

A take recorded over a loop keeps going past the loop end as one clip.

## Renaming

Right-click a channel name, a mixer strip, or a playlist track name to open a menu with rename and delete; channels also have preview. Right-click the pattern picker in the top bar or the clip picker in the automation editor to rename a pattern or automation clip. Enter or clicking away saves the name, Escape cancels, and Cmd+Z undoes it. Right-clicking a playlist clip still deletes it.

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
daw batch song.dawproj edits.txt            # many edits with one load and save
daw plugins                                # installed plugins from the scan cache
daw params song.dawproj 42 Cutoff=0.3      # set plugin parameters
daw export song.dawproj song.wav --range 16bar..24bar --stems stems
daw analyze song.wav stems/*.wav           # levels and octave bands
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
| Alt+Shift+R | Arm audio recording from the microphone |
| Delete | Delete the selection in the focused panel |
| Cmd+Z / Cmd+Shift+Z | Undo / redo |
| Cmd+N / Cmd+O / Cmd+S / Cmd+Shift+S | New / open / save / save as |
| Cmd+E | Export WAV |
| Cmd+I | Import a WAV file as an audio clip |
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
