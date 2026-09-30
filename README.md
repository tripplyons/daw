# DAW

A digital audio workstation (DAW) written in Rust with [iced](https://iced.rs). It hosts VST3 and Audio Unit plugins, arranges its panels as tiles the way an i3 or sway window manager does, and has FL Studio style patterns, a playlist, a mixer, and an automation editor.

It runs on macOS and Linux. Audio Units are macOS only. On Linux, plugin editor windows use X11, or XWayland on Wayland desktops.

## Requirements

- Rust 1.88 or newer (edition 2024), installed with [rustup](https://rustup.rs)
- macOS: Xcode Command Line Tools (`xcode-select --install`)
- Linux: the ALSA, X11, xkbcommon, and Wayland development packages. On Debian or Ubuntu:

```sh
sudo apt-get install build-essential pkg-config libclang-dev libasound2-dev libx11-dev libxkbcommon-dev libwayland-dev
```

On Linux, audio goes through ALSA, which PipeWire and PulseAudio also accept. File dialogs use the XDG desktop portal, or `zenity` when no portal is running. Without an output device the app still edits projects and exports WAV files.

## Run

```sh
git clone https://github.com/tripplyons/daw.git
cd daw
cargo run --release
```

The first build takes a few minutes. Debug builds also work (`cargo run`); the workspace compiles dependencies with optimizations so audio keeps up in either mode.

Tempo edits apply when you press Enter or leave the BPM field.

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
- Record: press Alt+Shift+R or click "rec audio" to arm the default input device, then play in song mode. Each stretch of playback becomes a take, written to the recordings folder and placed on the first free track. Saving the project embeds the take in the project file. Input latency is compensated. Click "rec audio" again to disarm. The app asks for microphone access the first time.
- Edit: drag a clip's left or right edge to trim it, Alt-click to split it, and Cmd+C, Cmd+V, and Cmd+D to copy, paste, and duplicate it. After an import or a take, the playlist brush is that file, so a click places the whole file again.

A take recorded over a loop keeps going past the loop end as one clip.

Plugin parameter drags create one undo step in both the parameter panel and native plugin windows. Undo and redo restore plugin state and pending parameter values, including automation recorded during the drag.

Select audio clips to show their pitch, duration, and reverse controls below the playlist. Pitch shifts keep the duration; duration changes keep the pitch. Type semitones and cents, or an exact duration ratio, then press Enter or click "set". Reset buttons restore pitch, cents, or the duration ratio. The pitch slider moves in cents. Turn on "stretch" in the playlist toolbar to stretch an audio clip by dragging its edge. Pitch and stretch drags show a preview and process audio on release. With stretch off, edge drags trim the file. These edits stay in the project and leave the source WAV unchanged. Recent processed versions are reused by undo and redo. Inactive cached audio is limited to 256 MiB and 64 entries; the current project's audio stays available.

Select pattern or audio clips and click "consolidate", or press Cmd+Alt+C in the playlist, to render the selection to a stereo WAV on a free track. The originals are muted and retained for undo. Insert effects and routing are rendered into the file; master effects and master gain remain live. Settings has separate export and consolidation tails from 0 to 120 seconds. These values are saved in the project. A consolidated clip includes its chosen tail. Ranged renders stop new notes and clips at the range end, then render the effect tail.

## Patterns and unique clips

Right-click the pattern picker and choose "clone pattern" to copy its notes into a new pattern. Select playlist clips and click "unique", or press Cmd+U in the playlist, to give each clip an independent source. Other instances keep their original pattern or automation envelope. Audio clips get independent channel settings and continue sharing the same WAV file. Placement and trims stay intact.

## Mixer sends and sidechains

Select a mixer insert. Its effects panel has an output route, a "send" picker for parallel audio, and a "sidechain" picker for detector-only audio. Each send has a level slider and a remove button. Gain, pan, and send level changes update playback as you drag. Sends use the source insert's signal after its effects, fader, and pan. Routes that would create feedback are refused.

A sidechain feeds the destination plugin's first auxiliary audio input. Use an effect that supports an external sidechain and enable that input in the plugin if needed. The detector signal is separate from the destination's audible input. The source keeps its normal output route.

## Live MIDI

Open settings (Cmd+comma), choose a MIDI input, and select an instrument channel. "Rescan inputs" updates the device list. The choice is saved in the config. MIDI note input plays the selected instrument even while stopped; a note-off returns to the instrument that received its note-on.

Click "rec midi" or press Cmd+Shift+R to arm recording, then play. Pattern mode records into the current pattern. Song mode creates a "MIDI take" pattern and playlist clip at the start marker or loop start, growing the take as needed. Pitch, velocity, start, and duration are recorded. Notes crossing a loop boundary are split; holding a key across several passes fills one loop. Stop or disarm to finish held notes.

## Renaming

Right-click a channel name, a mixer strip, or a playlist track name to open a menu with rename and delete; channels also have preview. Right-click the pattern picker in the top bar or the clip picker in the automation editor to rename a pattern or automation clip. Enter or clicking away saves the name, Escape cancels, and Cmd+Z undoes it. Right-clicking a playlist clip still deletes it.

## Files

- Projects are saved as `.dawproj` files. Closing, starting a new project, or opening another one asks to save unsaved changes first.
- Every save embeds the project data, plugin states, and all referenced audio and sampler WAVs in one `.dawproj` file. Imported audio, recorded takes, and consolidated audio travel with that file. Saves replace the project atomically; a missing audio file fails the save and leaves the previous file intact.
- Saves, autosaves, and packaging write on a background worker. Closing waits for a requested save to finish. Edits made during a save remain unsaved after it finishes.
- Autosave defaults to every two minutes when the project has changed, including plugin parameter edits and changes during playback. Each snapshot embeds the referenced WAVs and captures current plugin settings. Settings can change the interval or turn it off. Ten snapshots per project are kept under `~/Library/Application Support/daw/backups` on macOS or `~/.local/share/daw/backups` on Linux. "Recover backup" opens a snapshot as an unsaved project, so Save As keeps the recovery without overwriting the backup.
- Project files are ZIP archives with a text project document and an `assets` folder. Opening extracts working copies to the app's cache for playback; the saved file contains the originals. Older text-only `.dawproj` files still open and become self-contained on the next save.
- "Package project" in settings writes the same contents with a `.dawzip` extension. Both extensions open in the app and work with the CLI. Plugins must still be installed; files managed internally by a third-party plugin are not included.
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
daw export song.dawproj song.wav --range 16bar..24bar --tail 4.5 --stems stems
daw analyze song.wav stems/*.wav           # levels and octave bands
daw pack song.dawproj song.dawzip          # project and referenced audio
daw unpack song.dawzip relocated-song      # destination must be a new folder
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
| Cmd+Shift+R | Arm MIDI note recording |
| Cmd+U in playlist | Make selected clips unique |
| Cmd+Alt+C in playlist | Consolidate selection to audio |
| M in playlist | Mute or unmute selected clips |
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

On macOS, the native sidechain tests use the free kHs Compressor VST3 and Audio Unit plugins. With both installed:

```sh
cargo test -p daw-plugins --test khs_sidechain -- --ignored --test-threads=1
```

The workspace has four crates:

- `daw-model`: the project data (patterns, playlist, mixer, automation, layout)
- `daw-engine`: real-time audio rendering and the built-in synth
- `daw-plugins`: VST3 and Audio Unit scanning, loading, and editor windows
- `daw`: the iced app

## License

MIT. See [LICENSE](LICENSE).
