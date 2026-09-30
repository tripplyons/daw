---
name: daw-project
description: Inspect, create, edit, and render DAW .dawproj project files with the `daw` CLI. Use when asked to write music, change a song, add notes, patterns, channels, automation, mixer routing, or effects, read what a project contains, or export a project to WAV.
---

# Working on .dawproj projects

Use the `daw` subcommands to change projects. A `.dawproj` is a ZIP archive containing the project document and referenced WAVs. Normal saves, CLI edits, and autosaves include the audio, so the project file can be moved alone. Older text-only projects are upgraded on save. The commands keep ids unique, clean up references when something is removed, and refuse edits the app would not allow, such as mixer routes that loop.

## Setup

Use `daw` from PATH when it is installed (`command -v daw`). Otherwise, inside the DAW repository, build once and call the binary directly; `cargo run` also works but checks the build on every call.

```sh
DAW=daw                                       # installed with `cargo install --path crates/daw`
DAW=target/release/daw                        # or, in the repository, after `cargo build --release -p daw`
```

`$DAW --help` and `$DAW edit <file> <noun> --help` list every command and option.

## Workflow

1. Read the project: `$DAW show song.dawproj`. It prints every channel, pattern, automation clip, playlist clip, and mixer insert with its id.
2. Make changes. For more than a few edits, write a batch script and run it with `$DAW batch song.dawproj` (see below): one load and save, and each plugin loads once. Single edits go through `$DAW edit song.dawproj <noun> <verb> ...`, which prints only the new id for commands that create something.
3. Check the result with `show`, `show --pattern ID`, or `show --automation ID`.
4. Render and measure instead of guessing (see below). Render only the section you changed with `--range`, and get every mixer insert's stem from the same render with `--stems`.

On error, a command prints `error: ...` to stderr, exits 1, and leaves the file unchanged. A batch reports the failing line and saves nothing.

If the app has the same project open, saving in the app overwrites CLI edits, and the app may have saved changes since your last edit. Ask the user to close it or reopen it after your edits, and `show` the project again before relying on ids you read earlier.

## Batch scripts

Each line is what follows `daw edit FILE`, or `params INSTANCE NAME=VALUE ...`, or `presets INSTANCE --load NAME | --load-file PATH`. `NAME = COMMAND` keeps the command's output, such as a new id, and `$NAME` or `${NAME}` uses it on later lines. Words split like a shell's, so quote names with spaces. Lines starting with `#` are comments.

```sh
$DAW batch song.dawproj <<'EOF'
p = pattern add --name 'lead line' --length 2bar
lead = channel add lead --plugin 61756d75-56697461-54797465
presets $lead --load-file "/Users/me/Music/Vital/Warm Keys.vital"
params $lead Volume=0.6 'Macro 1'=0.3
note add $p $lead C4 0 1beat
clip add pattern:$p 0 0
EOF
```

Variables hold only command outputs, not environment variables, so write paths out in full. For computed parts (melodies, loops over bars), generate the script with Python or a shell loop and pipe it in. A 700-line build runs in about 2 seconds, against about 45 seconds as separate `daw edit` calls.

## Rendering and measuring

- `$DAW export FILE OUT.wav --range 64bar..72bar --stems DIR` renders 8 bars in a few seconds and writes `DIR/NN name.wav` for the master and each insert that a channel feeds. A full song with plugins renders at about 2x real time, so save whole-song exports for the end. A range starts silent: notes that began before it are not heard.
- `$DAW analyze OUT.wav DIR/*.wav` prints peak, RMS, stereo width, and octave-band levels (63 Hz to 16 kHz) per file; add `--bars 4 --bpm 175` for a row per 4 bars. Compare stems' RMS to balance a mix, and the master's bands to judge tone. A smooth mix falls about 3 to 5 dB per octave above 500 Hz; a flat top from 1 kHz up sounds harsh.

## Model

- A project has channels (instruments), patterns (notes for any channels), automation clips (an envelope over one parameter), a playlist (tracks holding clips that place patterns and automation clips in time), and a mixer (inserts with effect chains).
- Channels, patterns, automation clips, playlist clips, mixer inserts, and plugin instances share one id counter, so an id names exactly one thing. Read ids from `show`; never guess them. Insert 0 is the master.
- Notes belong to a pattern and a channel. Notes and automation points have no ids: notes are matched by key and start, points by index.
- Nothing plays in song mode until it is on the playlist. After making a pattern or automation clip, place it with `clip add`.
- Audio clips play part of a WAV file at its own speed. Add the file with `channel add NAME --audio WAV`, which embeds the WAV on save, then place it with `clip add audio:CHANNEL TRACK START`. `--offset` skips into the file. Changing the tempo does not stretch the audio or change clip lengths.
- A new project (`$DAW new song.dawproj`) has 8 mixer inserts, a saw synth channel on insert 1, one empty 1-bar pattern, and 16 empty tracks. Channels added later take the next unused insert.

## Value forms

`show` prints values in the same forms the commands accept.

- Time: ticks (`960`), or units joined by `+`: `2bar`, `3beat`, `5step`, `1bar+2beat`, `0.5beat`. A beat is a quarter note (960 ticks) and a step a sixteenth (240 ticks). Bars follow the time signature.
- Key: MIDI number or name with C4 = 60: `C4`, `F#3`, `Bb2`.
- Clip source: `pattern:ID`, `automation:ID`, or `audio:CHANNEL`.
- Automation target: `tempo`, `channel-volume:ID`, `channel-pan:ID`, `cutoff:ID` (built-in synth channels), `insert-volume:ID`, `insert-pan:ID`, `plugin:INSTANCE:PARAM`.
- Shape of the segment after a point: `linear`, `curve`, `s-curve`, `hold`, `stairs:N`, `pulse:N`. Tension from -1 to 1 bends `curve` and `s-curve`.
- Booleans: `--mute true` or `--mute false`.

Automation values are normalized from 0 to 1:

| Target | 0 | 1 |
| --- | --- | --- |
| tempo | 1 bpm | 999 bpm (bpm = 1 + 998 × value, so value = (bpm - 1) / 998; 120 bpm is about 0.1192) |
| channel-volume | silent | gain 1 (the channel volume itself) |
| insert-volume | silent | gain 2, about +6 dB (value = insert volume / 2) |
| channel-pan, insert-pan | left | right (0.5 is center) |
| cutoff | 40 Hz | 18 kHz, exponential |
| plugin params | the plugin's range; `$DAW params FILE INSTANCE` lists current values with units |

## Commands

```sh
$DAW new FILE [--force]
$DAW show FILE [--pattern ID | --automation ID] [--json]
$DAW plugins [--scan] [--json]            # installed plugins from the scan cache
$DAW params FILE INSTANCE [--find TEXT]   # load a plugin and list its parameters
$DAW params FILE INSTANCE --describe PARAM          # what the plugin shows across PARAM's range
$DAW params FILE INSTANCE PARAM=VALUE...  # set parameters (normalized 0..1) and save them
$DAW presets FILE INSTANCE [--find TEXT]  # list factory presets (Audio Units)
$DAW presets FILE INSTANCE --load NAME_OR_INDEX
$DAW presets FILE INSTANCE --load-file PRESET        # e.g. a Vital .vital file (Audio Units)
$DAW export FILE OUT.wav [--range START..END] [--stems DIR]
$DAW analyze WAV... [--bars N --bpm B]
$DAW batch FILE [SCRIPT]                  # script from a file or standard input

$DAW edit FILE set [--name N] [--bpm B] [--signature 3/4] [--grid 1/16] [--loop 0..8bar | --no-loop]
$DAW edit FILE channel add NAME [--synth sine|saw|square | --sampler WAV [--root KEY] | --audio WAV | --plugin NAME_OR_ID]
$DAW edit FILE channel set ID [--name --volume 0..1 --pan -1..1 --mute --insert ID --waveform --attack --release --cutoff --sample --root]
$DAW edit FILE channel remove ID
$DAW edit FILE pattern add [--name N] [--length TIME]
$DAW edit FILE pattern set ID [--name N] [--length TIME]
$DAW edit FILE pattern remove ID
$DAW edit FILE note add PATTERN CHANNEL KEY START LENGTH [--velocity 0..1]
$DAW edit FILE note steps PATTERN CHANNEL "x...x...x...x..." [--key KEY] [--velocity V]
$DAW edit FILE note remove PATTERN CHANNEL [--key KEY] [--start TIME]
$DAW edit FILE clip add SOURCE TRACK START [--length TIME] [--offset TIME]
$DAW edit FILE clip set ID [--track --start --length --offset]
$DAW edit FILE clip remove ID
$DAW edit FILE track add [--name N] | set INDEX [--name --mute] | remove INDEX
$DAW edit FILE insert add [--name N] | set ID [--name --volume 0..2 --pan --mute --solo --output ID] | remove ID
$DAW edit FILE effect add INSERT NAME_OR_ID | remove INSTANCE
$DAW edit FILE automation add TARGET [--name N] [--length TIME] [--value 0..1]
$DAW edit FILE automation set ID [--name N] [--length TIME] | remove ID
$DAW edit FILE point add AUTOMATION TIME VALUE [--shape S] [--tension T]
$DAW edit FILE point set AUTOMATION INDEX [--time --value --shape --tension] | remove AUTOMATION INDEX
```

Behavior worth knowing:

- `note add` and `note steps` grow the pattern to whole bars that hold the new notes. `note steps` replaces all of that channel's notes in the pattern.
- `note remove` with no filters clears the channel's notes in that pattern.
- `pattern set --length` and `automation set --length` also resize playlist clips that show the whole source.
- `set --bpm` accepts 1 to 999, the same range tempo automation covers.
- `clip add` adds tracks up to the index you give. Its length defaults to the source's length, less `--offset`; for `audio:CHANNEL`, that is the WAV file's length at the project tempo.
- `automation add` makes two points, at 0 and at the clip's length, both at the target's current value or `--value`. Each point sets the shape of the segment that follows it. One clip per target. `point add` past the clip's end grows the clip; `point add` and `point set` print the point's index after sorting by time.
- `--plugin` and `effect add` match an exact plugin id or a case-insensitive name. When a plugin exists in several formats, pass the id from `$DAW plugins`. If the cache is empty, run `$DAW plugins --scan` (it can take a minute).
- INSTANCE in `params` and `presets` is a plugin instance id (as `show` prints next to plugin channels and insert effects, and `effect add` prints) or the id of a plugin channel.

## Plugin sounds and settings

- `params FILE INSTANCE NAME=VALUE ...` sets any number of parameters in one call and saves them into the plugin's state. PARAM is an id or a case-insensitive name; quote names with spaces: `"Sync Mode"=1`.
- Values are normalized. Find the value for a setting with `--describe`, which prints what the plugin displays at evenly spaced values. For example, kHs Filter's cutoff shows 160 Hz at 0.3 and 640 Hz at 0.5, so it maps as 20 Hz × 2^(10 × value).
- `presets --load` picks an Audio Unit factory preset. Some plugins list only placeholder names (Serum 2 shows "Prog 1" and so on).
- `presets --load-file` replaces a JUCE-based Audio Unit's state with a file in the plugin's own format, which for Vital is a `.vital` preset. Use the AU version of the plugin; VST3 does not expose this state. Other JUCE plugins need their own state format, for example Dexed expects its XML with a DX7 cartridge inside.
- Loading a plugin takes about a second. Set several parameters per call, or use a batch, which loads each plugin once.

## Example: a 4-bar loop

```sh
$DAW new beat.dawproj
$DAW show beat.dawproj                  # synth channel 9, pattern 10 in a new project
$DAW batch beat.dawproj <<'EOF'
set --bpm 124 --loop 0..4bar
kick = channel add kick --synth sine
note steps 10 $kick x...x...x...x... --key C2
note add 10 9 C4 0 1beat --velocity 0.7
note add 10 9 E4 1beat 1beat --velocity 0.7
note add 10 9 G4 2beat 1beat --velocity 0.7
note add 10 9 B4 3beat 1beat --velocity 0.7
clip add pattern:10 0 0 --length 4bar
sweep = automation add cutoff:9 --value 0.2
# A new clip has points 0 and 1 at its start and end. Point 0 owns the shape of the segment after it.
point set $sweep 0 --shape curve --tension 0.5
point set $sweep 1 --value 0.9
clip add automation:$sweep 1 0
EOF
$DAW export beat.dawproj beat.wav --stems beat-stems
$DAW analyze beat.wav beat-stems/*.wav
```

Run `show` before using ids in a real project; the ids above only hold for a fresh `new`.
