//! Text for mixer values in the units the user reads, shared by the app and
//! the CLI.

pub fn gain_text(gain: f32) -> String {
    if gain <= 0.0001 { "-inf dB".into() } else { format!("{:.1} dB", 20.0 * gain.log10()) }
}

pub fn pan_text(pan: f32) -> String {
    match pan {
        p if p.abs() < 0.005 => "C".into(),
        p if p < 0.0 => format!("{:.0}L", -p * 100.0),
        p => format!("{:.0}R", p * 100.0),
    }
}
