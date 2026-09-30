//! Audio clips: takes recorded from the microphone and imported WAV files.
//! Each file becomes an audio channel, and its clips play parts of the file.

use std::path::{Path, PathBuf};

use daw_engine::input::Take;
use daw_engine::song::PlayMode;
use daw_model::time::{Ticks, seconds_to_ticks, ticks_to_seconds};
use daw_model::{ChannelId, Source};

use super::App;

impl App {
    /// Arm or disarm the microphone. While armed, every stretch of song
    /// playback records a take.
    pub fn toggle_audio_recording(&mut self) {
        if self.session.recording_armed() {
            let takes = self.session.disarm_recording();
            self.set_status("audio recording off");
            self.place_takes(takes);
            return;
        }
        if let Err(error) = self.session.arm_recording() {
            self.set_status(format!("could not open the microphone: {error}"));
            return;
        }
        // Takes are placed by song position, so pattern playback cannot record.
        if self.mode != PlayMode::Song {
            self.mode = PlayMode::Song;
            self.refresh();
            self.return_to_start();
        }
        self.set_status("audio recording armed: play to record");
    }

    /// Disarm and place the takes so far, before the project, the play mode,
    /// or the engine's clock changes underneath the recorder.
    pub fn stop_audio_recording(&mut self) {
        if self.session.recording_armed() {
            self.toggle_audio_recording();
        }
    }

    /// Place the takes that ended since the last tick.
    pub(super) fn collect_takes(&mut self) {
        let (takes, overflowed) = self.session.collect_takes();
        if overflowed {
            self.set_status("audio input fell behind; part of the take is missing");
        }
        self.place_takes(takes);
    }

    fn place_takes(&mut self, takes: Vec<Take>) {
        for take in takes {
            match self.place_take(take) {
                Ok(Some(path)) => self.set_status(format!("recorded {}", path.display())),
                Ok(None) => {}
                Err(error) => self.set_status(format!("could not save the take: {error}")),
            }
        }
    }

    /// Write a take to the recordings folder and place it on the first free
    /// track. Returns the file, or `None` when nothing was recorded after
    /// the song start marker.
    pub(super) fn place_take(&mut self, take: Take) -> Result<Option<PathBuf>, String> {
        let channels = usize::from(take.channels.max(1));
        let rate = f64::from(take.sample_rate);
        let bpm = self.project.bpm;
        // Input latency reaches back before playback was heard; that part
        // was recorded before the song started.
        let floor = self.song_start.max(0.0);
        let skip = if take.start < floor { (ticks_to_seconds(floor - take.start, bpm) * rate).round() as usize } else { 0 };
        let samples = take.samples.get(skip * channels..).unwrap_or_default();
        let frames = samples.len() / channels;
        if frames == 0 {
            return Ok(None);
        }
        let path = write_take(&recordings_dir(self.path.as_deref()), &samples[..frames * channels], take.channels, take.sample_rate)?;
        let start = take.start.max(floor).round() as Ticks;
        let length = seconds_to_ticks(frames as f64 / rate, bpm).round() as Ticks;
        self.checkpoint();
        self.add_audio(&path, start, length);
        self.edited();
        Ok(Some(path))
    }

    /// Render only the selected pattern/audio clips through their mixer
    /// paths. Master processing remains live on the consolidated audio.
    pub fn consolidate_selection(&mut self) -> Result<(), String> {
        let selected = self.playlist.selected.clone();
        let clips: Vec<_> = self.project.playlist.clips.iter()
            .filter(|c| selected.contains(&c.id) && !c.muted && !matches!(c.source, daw_model::ClipSource::Automation(_))).cloned().collect();
        let start = clips.iter().map(|c| c.start).min().ok_or("select pattern or audio clips first")?;
        let end = clips.iter().map(|c| c.end()).max().ok_or("selection has no end")?;
        self.poll_midi();
        self.finish_midi();
        self.stop_audio_recording();
        self.session.store_states(&mut self.project);
        let mut render = self.project.clone();
        render.playlist.loop_range = None;
        let master_automation: Vec<_> = render.automation.iter().filter(|a| matches!(a.target,
            daw_model::Target::InsertVolume(daw_model::MASTER) | daw_model::Target::InsertPan(daw_model::MASTER)))
            .map(|a| a.id).collect();
        render.playlist.clips.retain(|c| match c.source {
            daw_model::ClipSource::Automation(id) => !master_automation.contains(&id),
            _ => selected.contains(&c.id),
        });
        let master = render.mixer.insert_mut(daw_model::MASTER).ok_or("missing master insert")?;
        master.effects.clear();
        master.volume = 1.0;
        master.pan = 0.0;
        master.mute = false;
        let folder = recordings_dir(self.path.as_deref());
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let path = folder.join(format!("consolidated-{}.wav", crate::project_files::stamp()));
        let options = crate::session::RenderOptions { depth: daw_engine::output::BitDepth::Float32, range: Some((start, end)), tail_seconds: self.project.render.consolidation_tail_seconds };
        if let Err(error) = self.session.export(&render, &path, self.mode, options, &[]) {
            let _ = std::fs::remove_file(&path);
            self.refresh();
            return Err(error);
        }
        self.checkpoint();
        for clip in &mut self.project.playlist.clips {
            if clips.iter().any(|c| c.id == clip.id) { clip.muted = true; }
        }
        let tail = seconds_to_ticks(options.tail_seconds, self.project.bpm).round() as Ticks;
        let channel = self.add_audio(&path, start, end - start + tail);
        let audio = self.project.channel_mut(channel).expect("new channel");
        audio.volume = 1.0;
        audio.pan = 0.0;
        audio.insert = daw_model::MASTER;
        self.playlist.selected = self.project.playlist.clips.iter().filter(|c| c.source == daw_model::ClipSource::Audio(channel)).map(|c| c.id).collect();
        self.playing = false;
        self.edited();
        self.return_to_start();
        self.set_status(format!("consolidated {} clips", clips.len()));
        Ok(())
    }

    /// Add a WAV file as an audio channel with one clip of the whole file at `start`.
    pub fn import_audio(&mut self, path: &Path, start: Ticks) -> Result<ChannelId, String> {
        let path = std::fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let length = file_length(&path.to_string_lossy(), self.project.bpm)?;
        self.checkpoint();
        let channel = self.add_audio(&path, start, length);
        self.edited();
        self.set_status(format!("imported {}", path.display()));
        Ok(channel)
    }

    /// Length of an audio channel's whole file at the current tempo, once loaded.
    pub fn audio_length(&self, channel: ChannelId) -> Option<Ticks> {
        let Source::Audio { path } = &self.project.channel(channel)?.source else { return None };
        let (sample, _) = self.session.waveform(path)?;
        let seconds = sample.left.len() as f64 / sample.sample_rate;
        Some((seconds_to_ticks(seconds, self.project.bpm).round() as Ticks).max(1))
    }

    fn add_audio(&mut self, path: &Path, start: Ticks, length: Ticks) -> ChannelId {
        let name = path.file_stem().unwrap_or_default().to_string_lossy();
        let channel = self.project.add_channel(&name, Source::Audio { path: path.to_string_lossy().into_owned() });
        let track = self.project.free_track(start, start + length.max(1));
        self.project.add_audio_clip(track, start, channel, length);
        self.selected_channel = Some(channel);
        self.playlist.brush = Some(daw_model::ClipSource::Audio(channel));
        channel
    }
}

/// Length of a WAV file in ticks at `bpm`, read from its header.
pub fn file_length(path: &str, bpm: f64) -> Result<Ticks, String> {
    let reader = hound::WavReader::open(path).map_err(|e| format!("could not read {path}: {e}"))?;
    let seconds = f64::from(reader.duration()) / f64::from(reader.spec().sample_rate);
    Ok((seconds_to_ticks(seconds, bpm).round() as Ticks).max(1))
}

/// `recordings` next to the project file, or in the app's data folder for
/// a project that has not been saved.
pub(super) fn recordings_dir(project: Option<&Path>) -> PathBuf {
    match project.and_then(Path::parent) {
        Some(folder) => folder.join("recordings"),
        None => dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("daw").join("recordings"),
    }
}

/// Write interleaved samples to the first free `take N.wav` in `dir`.
fn write_take(dir: &Path, samples: &[f32], channels: u16, sample_rate: u32) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let path = (1..).map(|n| dir.join(format!("take {n}.wav"))).find(|p| !p.exists()).expect("a free name");
    let spec = hound::WavSpec { channels, sample_rate, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let write = || -> Result<(), hound::Error> {
        let mut writer = hound::WavWriter::create(&path, spec)?;
        for &sample in samples {
            writer.write_sample(sample)?;
        }
        writer.finalize()
    };
    write().map_err(|e| format!("could not write {}: {e}", path.display()))?;
    Ok(path)
}
