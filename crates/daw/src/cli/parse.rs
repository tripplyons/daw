//! Text forms for command-line values. Each parser has a matching formatter
//! so `daw show` prints values in a form the edit commands accept.

use std::str::FromStr;

use daw_model::automation::Shape;
use daw_model::time::{Grid, TICKS_PER_BEAT, TimeSignature, Ticks};
use daw_model::{AutomationId, ChannelId, ClipSource, InsertId, InstanceId, PatternId, STEP_TICKS, Target, Waveform};

/// A position or length: ticks, or musical units joined by `+`, such as
/// `960`, `2bar`, `1bar+2beat`, or `3step`. A beat is a quarter note and a
/// step a sixteenth. Bars depend on the time signature, so they resolve late.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Time {
    bars: f64,
    ticks: f64,
}

impl Time {
    pub fn ticks(self, signature: TimeSignature) -> Ticks {
        (self.bars * signature.ticks_per_bar() as f64 + self.ticks).round() as Ticks
    }
}

impl FromStr for Time {
    type Err = String;

    fn from_str(text: &str) -> Result<Time, String> {
        let mut time = Time { bars: 0.0, ticks: 0.0 };
        for term in text.split('+') {
            let term = term.trim();
            let split = term.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(term.len());
            let (number, unit) = term.split_at(split);
            let number: f64 = number.parse().map_err(|_| format!("bad time {text:?}; expected e.g. 960, 2bar, 1bar+2beat, 3step"))?;
            if !number.is_finite() || number < 0.0 {
                return Err(format!("time {text:?} must not be negative"));
            }
            match unit {
                "" | "tick" | "ticks" => time.ticks += number,
                "beat" | "beats" => time.ticks += number * f64::from(TICKS_PER_BEAT),
                "step" | "steps" => time.ticks += number * STEP_TICKS as f64,
                "bar" | "bars" => time.bars += number,
                _ => return Err(format!("unknown time unit {unit:?} in {text:?}; use bar, beat, step, or tick")),
            }
        }
        Ok(time)
    }
}

/// The largest whole unit that divides `ticks`, e.g. `2bar`, `3beat`, `5step`, or `100`.
pub fn time(ticks: Ticks, signature: TimeSignature) -> String {
    let bar = signature.ticks_per_bar();
    let beat = Ticks::from(TICKS_PER_BEAT);
    match ticks {
        0 => "0".into(),
        t if t.is_multiple_of(bar) => format!("{}bar", t / bar),
        t if t.is_multiple_of(beat) => format!("{}beat", t / beat),
        t if t.is_multiple_of(STEP_TICKS) => format!("{}step", t / STEP_TICKS),
        t => t.to_string(),
    }
}

const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// A MIDI key number or a note name where C4 is 60, such as `C4`, `F#3`, or `Bb2`.
pub fn key(text: &str) -> Result<u8, String> {
    if let Ok(number) = text.parse::<u8>() {
        return if number <= 127 { Ok(number) } else { Err(format!("key {number} is above 127")) };
    }
    let bad = || format!("bad key {text:?}; expected a MIDI number or a name like C4, F#3, Bb2");
    let mut chars = text.chars();
    let letter = chars.next().ok_or_else(bad)?.to_ascii_uppercase();
    let mut semitone: i32 = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return Err(bad()),
    };
    let rest = chars.as_str();
    let octave = match rest.chars().next() {
        Some('#') => {
            semitone += 1;
            &rest[1..]
        }
        Some('b') => {
            semitone -= 1;
            &rest[1..]
        }
        _ => rest,
    };
    let octave: i32 = octave.parse().map_err(|_| bad())?;
    let number = (octave + 1) * 12 + semitone;
    u8::try_from(number).ok().filter(|&n| n <= 127).ok_or_else(|| format!("key {text:?} is outside MIDI range 0..127"))
}

pub fn key_name(key: u8) -> String {
    format!("{}{}", NOTE_NAMES[usize::from(key % 12)], i32::from(key / 12) - 1)
}

pub fn waveform(text: &str) -> Result<Waveform, String> {
    match text {
        "sine" => Ok(Waveform::Sine),
        "saw" => Ok(Waveform::Saw),
        "square" => Ok(Waveform::Square),
        _ => Err(format!("bad waveform {text:?}; use sine, saw, or square")),
    }
}

pub fn waveform_name(waveform: Waveform) -> &'static str {
    match waveform {
        Waveform::Sine => "sine",
        Waveform::Saw => "saw",
        Waveform::Square => "square",
    }
}

/// `linear`, `curve`, `s-curve`, `hold`, `stairs:N`, or `pulse:N`.
pub fn shape(text: &str) -> Result<Shape, String> {
    let count = |n: &str| n.parse::<u8>().ok().filter(|&n| n > 0).ok_or(format!("bad step count in {text:?}"));
    match text.split_once(':') {
        None => match text {
            "linear" => Ok(Shape::Linear),
            "curve" => Ok(Shape::Curve),
            "s-curve" => Ok(Shape::SCurve),
            "hold" => Ok(Shape::Hold),
            _ => Err(format!("bad shape {text:?}; use linear, curve, s-curve, hold, stairs:N, or pulse:N")),
        },
        Some(("stairs", n)) => Ok(Shape::Stairs(count(n)?)),
        Some(("pulse", n)) => Ok(Shape::Pulse(count(n)?)),
        Some(_) => Err(format!("bad shape {text:?}; use linear, curve, s-curve, hold, stairs:N, or pulse:N")),
    }
}

pub fn shape_name(shape: Shape) -> String {
    match shape {
        Shape::Hold => "hold".into(),
        Shape::Linear => "linear".into(),
        Shape::Curve => "curve".into(),
        Shape::SCurve => "s-curve".into(),
        Shape::Stairs(n) => format!("stairs:{n}"),
        Shape::Pulse(n) => format!("pulse:{n}"),
    }
}

/// `tempo`, `channel-volume:ID`, `channel-pan:ID`, `cutoff:ID`,
/// `insert-volume:ID`, `insert-pan:ID`, or `plugin:INSTANCE:PARAM`.
pub fn target(text: &str) -> Result<Target, String> {
    let bad = || {
        format!(
            "bad target {text:?}; use tempo, channel-volume:ID, channel-pan:ID, cutoff:ID, insert-volume:ID, insert-pan:ID, or plugin:INSTANCE:PARAM"
        )
    };
    let parts: Vec<&str> = text.split(':').collect();
    let number = |i: usize| parts.get(i).and_then(|p| p.parse::<u64>().ok()).ok_or_else(bad);
    let target = match parts[0] {
        "tempo" if parts.len() == 1 => Target::Tempo,
        "channel-volume" if parts.len() == 2 => Target::ChannelVolume(ChannelId(number(1)?)),
        "channel-pan" if parts.len() == 2 => Target::ChannelPan(ChannelId(number(1)?)),
        "cutoff" if parts.len() == 2 => Target::SynthCutoff(ChannelId(number(1)?)),
        "insert-volume" if parts.len() == 2 => Target::InsertVolume(InsertId(number(1)?)),
        "insert-pan" if parts.len() == 2 => Target::InsertPan(InsertId(number(1)?)),
        "plugin" if parts.len() == 3 => {
            let param = u32::try_from(number(2)?).map_err(|_| bad())?;
            Target::Plugin { instance: InstanceId(number(1)?), param }
        }
        _ => return Err(bad()),
    };
    Ok(target)
}

pub fn target_name(target: Target) -> String {
    match target {
        Target::Tempo => "tempo".into(),
        Target::ChannelVolume(id) => format!("channel-volume:{}", id.0),
        Target::ChannelPan(id) => format!("channel-pan:{}", id.0),
        Target::SynthCutoff(id) => format!("cutoff:{}", id.0),
        Target::InsertVolume(id) => format!("insert-volume:{}", id.0),
        Target::InsertPan(id) => format!("insert-pan:{}", id.0),
        Target::Plugin { instance, param } => format!("plugin:{}:{param}", instance.0),
    }
}

/// `pattern:ID`, `automation:ID`, or `audio:CHANNEL`.
pub fn clip_source(text: &str) -> Result<ClipSource, String> {
    let bad = || format!("bad clip source {text:?}; use pattern:ID, automation:ID, or audio:CHANNEL");
    let (kind, id) = text.split_once(':').ok_or_else(bad)?;
    let id: u64 = id.parse().map_err(|_| bad())?;
    match kind {
        "pattern" => Ok(ClipSource::Pattern(PatternId(id))),
        "automation" => Ok(ClipSource::Automation(AutomationId(id))),
        "audio" => Ok(ClipSource::Audio(ChannelId(id))),
        _ => Err(bad()),
    }
}

pub fn clip_source_name(source: ClipSource) -> String {
    match source {
        ClipSource::Pattern(id) => format!("pattern:{}", id.0),
        ClipSource::Automation(id) => format!("automation:{}", id.0),
        ClipSource::Audio(id) => format!("audio:{}", id.0),
    }
}

/// `N/D`, such as `4/4` or `6/8`.
pub fn signature(text: &str) -> Result<TimeSignature, String> {
    let bad = || format!("bad time signature {text:?}; expected e.g. 4/4 or 6/8");
    let (numerator, denominator) = text.split_once('/').ok_or_else(bad)?;
    let numerator: u32 = numerator.parse().map_err(|_| bad())?;
    let denominator: u32 = denominator.parse().map_err(|_| bad())?;
    if numerator == 0 || !matches!(denominator, 1 | 2 | 4 | 8 | 16 | 32) {
        return Err(bad());
    }
    Ok(TimeSignature { numerator, denominator })
}

/// The labels the app shows: `off`, `bar`, `beat`, `1/16`, `1/8T`, `1/8.`.
pub fn grid(text: &str) -> Result<Grid, String> {
    let bad = || format!("bad grid {text:?}; use off, bar, beat, 1/N, 1/NT, or 1/N.");
    let division = |n: &str| n.parse::<u32>().ok().filter(|&n| n > 0).ok_or_else(bad);
    match text {
        "off" => Ok(Grid::Off),
        "bar" => Ok(Grid::Bar),
        "beat" => Ok(Grid::Beat),
        _ => {
            let n = text.strip_prefix("1/").ok_or_else(bad)?;
            if let Some(n) = n.strip_suffix('T') {
                Ok(Grid::Triplet(division(n)?))
            } else if let Some(n) = n.strip_suffix('.') {
                Ok(Grid::Dotted(division(n)?))
            } else {
                Ok(Grid::Division(division(n)?))
            }
        }
    }
}

/// `START..END`, such as `0..8bar`.
pub fn range(text: &str) -> Result<(Time, Time), String> {
    let (start, end) = text.split_once("..").ok_or(format!("bad range {text:?}; expected START..END, e.g. 0..8bar"))?;
    Ok((start.parse()?, end.parse()?))
}

/// A fraction in `0..=1`: velocities and normalized automation values.
pub fn unit(text: &str) -> Result<f32, String> {
    bounded(text, 0.0, 1.0)
}

/// Pan from -1 (left) to 1 (right).
pub fn pan(text: &str) -> Result<f32, String> {
    bounded(text, -1.0, 1.0)
}

pub fn bounded(text: &str, min: f32, max: f32) -> Result<f32, String> {
    let value: f32 = text.parse().map_err(|_| format!("bad number {text:?}"))?;
    if (min..=max).contains(&value) { Ok(value) } else { Err(format!("{value} is outside {min}..{max}")) }
}

/// Seconds appended to an offline render for reverb and delay tails.
pub fn tail(text: &str) -> Result<f64, String> {
    let value: f64 = text.parse().map_err(|_| format!("bad tail {text:?}"))?;
    daw_model::RenderSettings::check_tail("tail", value)
}
