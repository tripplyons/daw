//! `daw analyze`: levels and octave-band balance of rendered WAV files, to
//! check a mix without listening.

use std::f64::consts::PI;
use std::fmt::Write;
use std::path::Path;

/// Octave band centers in Hz.
const BANDS: [f64; 9] = [63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];

struct Audio {
    sample_rate: f64,
    left: Vec<f32>,
    right: Vec<f32>,
}

/// One row per file, plus one per section when `section_seconds` is given.
pub fn analyze(paths: &[impl AsRef<Path>], section_seconds: Option<f64>) -> Result<String, String> {
    let mut out = String::from("file                            peak    rms  width |");
    for band in BANDS {
        let label = if band >= 1000.0 { format!("{}k", band / 1000.0) } else { format!("{band}") };
        let _ = write!(out, "{label:>6}");
    }
    out.push_str("\n  dBFS; width is side level relative to mid; bands are dB RMS per octave");
    for path in paths {
        let path = path.as_ref();
        let audio = read(path)?;
        let name = path.file_stem().map_or_else(|| path.display().to_string(), |s| s.to_string_lossy().into_owned());
        let _ = write!(out, "\n{}", row(&truncate(&name, 28), &audio.left, &audio.right, audio.sample_rate));
        if let Some(seconds) = section_seconds {
            let size = (seconds * audio.sample_rate).round() as usize;
            for (index, start) in (0..audio.left.len()).step_by(size.max(1)).enumerate() {
                let end = (start + size).min(audio.left.len());
                let label = format!("  section {index} ({:.1}s)", start as f64 / audio.sample_rate);
                let _ = write!(out, "\n{}", row(&label, &audio.left[start..end], &audio.right[start..end], audio.sample_rate));
            }
        }
    }
    Ok(out)
}

fn row(label: &str, left: &[f32], right: &[f32], sample_rate: f64) -> String {
    let mid: Vec<f32> = left.iter().zip(right).map(|(l, r)| (l + r) / 2.0).collect();
    let side: Vec<f32> = left.iter().zip(right).map(|(l, r)| (l - r) / 2.0).collect();
    let peak = left.iter().chain(right).fold(0.0f32, |m, s| m.max(s.abs()));
    let mut text = format!(
        "{label:28} {:6.1} {:6.1} {:6.1} |",
        db(f64::from(peak)),
        db(rms(&mid)),
        db(rms(&side) / rms(&mid).max(1e-12))
    );
    for band in BANDS {
        let level = if band < sample_rate / 2.0 { db(rms(&bandpass(&mid, sample_rate, band))) } else { -99.9 };
        let _ = write!(text, "{level:6.1}");
    }
    text
}

/// RBJ constant-peak band-pass, one octave wide.
fn bandpass(input: &[f32], sample_rate: f64, center: f64) -> Vec<f32> {
    let w = 2.0 * PI * center / sample_rate;
    let alpha = w.sin() / (2.0 * std::f64::consts::SQRT_2);
    let a0 = 1.0 + alpha;
    let (b0, b2) = (alpha / a0, -alpha / a0);
    let (a1, a2) = (-2.0 * w.cos() / a0, (1.0 - alpha) / a0);
    let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
    input
        .iter()
        .map(|&x| {
            let x = f64::from(x);
            let y = b0 * x + b2 * x2 - a1 * y1 - a2 * y2;
            (x2, x1, y2, y1) = (x1, x, y1, y);
            y as f32
        })
        .collect()
}

fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum::<f64>() / samples.len() as f64).sqrt()
}

/// Decibels, floored at -99.9 so silence and mono read cleanly.
fn db(value: f64) -> f64 {
    (20.0 * value.max(1e-12).log10()).max(-99.9)
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width { text.to_string() } else { text.chars().take(width - 1).chain(['…']).collect() }
}

fn read(path: &Path) -> Result<Audio, String> {
    let error = |e: hound::Error| format!("could not read {}: {e}", path.display());
    let mut reader = hound::WavReader::open(path).map_err(error)?;
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>().map_err(error)?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().map(|s| s.map(|s| s as f32 * scale)).collect::<Result<_, _>>().map_err(error)?
        }
    };
    let channels = usize::from(spec.channels.max(1));
    let left: Vec<f32> = samples.iter().step_by(channels).copied().collect();
    let right = if channels > 1 { samples.iter().skip(1).step_by(channels).copied().collect() } else { left.clone() };
    Ok(Audio { sample_rate: f64::from(spec.sample_rate), left, right })
}
