//! Self-contained project archives, atomic saves, and recoverable snapshots.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use daw_model::{Project, Source};
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

static SERIAL: AtomicU64 = AtomicU64::new(0);

pub fn stamp() -> String {
    let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    format!("{time}-{}-{}", std::process::id(), SERIAL.fetch_add(1, Ordering::Relaxed))
}

fn audio_path(source: &mut Source) -> Option<&mut String> {
    match source { Source::Audio { path } | Source::Sampler { path, .. } => Some(path), _ => None }
}

pub fn load(path: &Path) -> Result<Project, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut signature = [0; 4];
    let count = file.read(&mut signature).map_err(|e| format!("{}: {e}", path.display()))?;
    if count == 4 && signature[..2] == *b"PK" {
        // Playback needs file paths, but the archive remains the complete
        // saved project. These extracted copies can be rebuilt on every open.
        let cache = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("daw").join("projects");
        std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
        let manifest = unpack(path, &cache.join(stamp()))?;
        return load_document(&manifest);
    }
    load_document(path)
}

fn load_document(path: &Path) -> Result<Project, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut project = Project::from_ron(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    validate(&project)?;
    let base = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
    let base = base.parent().ok_or("project has no parent folder")?;
    for channel in &mut project.channels {
        if let Some(path) = audio_path(&mut channel.source) && Path::new(path).is_relative() {
            *path = base.join(&*path).to_string_lossy().into_owned();
        }
    }
    Ok(project)
}

fn validate(project: &Project) -> Result<(), String> {
    project.render.validate()?;
    if project.patterns.is_empty() || project.mixer.inserts.first().is_none_or(|i| i.id != daw_model::MASTER) {
        return Err("project needs a pattern and a master mixer insert".into());
    }
    for clip in &project.playlist.clips {
        if !clip.audio.stretch.is_finite() || !(0.125..=8.0).contains(&clip.audio.stretch) {
            return Err(format!("clip {}: stretch must be between 0.125 and 8", clip.id.0));
        }
        if !clip.audio.semitones.is_finite() || !(-48.0..=48.0).contains(&clip.audio.semitones) {
            return Err(format!("clip {}: pitch must be between -48 and 48 semitones", clip.id.0));
        }
    }
    for insert in project.mixer.inserts.iter().skip(1) {
        if !project.mixer.can_route(insert.id, insert.output) { return Err(format!("invalid output route on {}", insert.name)); }
        for send in &insert.sends {
            if !project.mixer.can_route(insert.id, send.to) || !send.level.is_finite() || !(0.0..=2.0).contains(&send.level) {
                return Err(format!("invalid send on {}", insert.name));
            }
        }
    }
    Ok(())
}

pub fn save(path: &Path, project: &Project) -> Result<(), String> {
    pack(path, project)
}

pub fn pack(path: &Path, project: &Project) -> Result<(), String> {
    validate(project)?;
    let temporary = path.with_extension(format!("{}.tmp", stamp()));
    let result = (|| -> Result<(), String> {
        let file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary).map_err(|e| e.to_string())?;
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut stored = project.clone();
        let mut copied = HashMap::<PathBuf, String>::new();
        for channel in &mut stored.channels {
            let Some(path) = audio_path(&mut channel.source) else { continue };
            let source = std::fs::canonicalize(&*path).map_err(|e| format!("missing audio file {path}: {e}"))?;
            if let Some(name) = copied.get(&source) { *path = name.clone(); continue; }
            let name = format!("assets/{}.wav", copied.len());
            zip.start_file(&name, options).map_err(|e| e.to_string())?;
            let mut input = std::fs::File::open(&source).map_err(|e| e.to_string())?;
            std::io::copy(&mut input, &mut zip).map_err(|e| e.to_string())?;
            copied.insert(source, name.clone());
            *path = name;
        }
        zip.start_file("project.dawproj", options).map_err(|e| e.to_string())?;
        zip.write_all(stored.to_ron().map_err(|e| e.to_string())?.as_bytes()).map_err(|e| e.to_string())?;
        zip.finish().map_err(|e| e.to_string())?.sync_all().map_err(|e| e.to_string())?;
        std::fs::rename(&temporary, path).map_err(|e| e.to_string())
    })();
    if result.is_err() { let _ = std::fs::remove_file(&temporary); }
    result
}

/// `folder` must be new. Every archive entry stays within it, including audio references.
pub fn unpack(path: &Path, folder: &Path) -> Result<PathBuf, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut archive = ZipArchive::new(file).map_err(|e| e.to_string())?;
    std::fs::create_dir(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let result = (|| -> Result<PathBuf, String> {
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            let name = entry.enclosed_name().ok_or("archive contains an unsafe path")?;
            if entry.unix_mode().is_some_and(|mode| mode & 0o170000 == 0o120000) { return Err("archive contains a symbolic link".into()); }
            let out = folder.join(name);
            if entry.is_dir() { std::fs::create_dir_all(&out).map_err(|e| e.to_string())?; continue; }
            if let Some(parent) = out.parent() { std::fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&out).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut file).map_err(|e| e.to_string())?;
        }
        let project_path = folder.join("project.dawproj");
        let mut text = String::new();
        std::fs::File::open(&project_path).map_err(|e| e.to_string())?.read_to_string(&mut text).map_err(|e| e.to_string())?;
        let mut project = Project::from_ron(&text).map_err(|e| e.to_string())?;
        let root = std::fs::canonicalize(folder).map_err(|e| e.to_string())?;
        for channel in &mut project.channels {
            let Some(path) = audio_path(&mut channel.source) else { continue };
            if Path::new(path).is_absolute() { return Err("package audio path must be relative".into()); }
            let resolved = std::fs::canonicalize(root.join(&*path)).map_err(|e| format!("package audio {path}: {e}"))?;
            if !resolved.starts_with(&root) { return Err("package audio path leaves the archive".into()); }
        }
        load_document(&project_path)?;
        Ok(project_path)
    })();
    if result.is_err() { let _ = std::fs::remove_dir_all(folder); }
    result
}

pub fn backups_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("daw").join("backups")
}

pub fn backup_key(name: &str, path: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned()).hash(&mut hash);
    let name: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' }).take(60).collect();
    format!("{name}-{:016x}", hash.finish())
}

pub fn snapshot(folder: &Path, project: &Project) -> Result<PathBuf, String> {
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let path = folder.join(format!("{}.dawproj", stamp()));
    save(&path, project)?;
    let mut snapshots: Vec<_> = std::fs::read_dir(folder).map_err(|e| e.to_string())?.filter_map(Result::ok)
        .map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "dawproj")).collect();
    snapshots.sort();
    for old in snapshots.iter().take(snapshots.len().saturating_sub(10)) {
        std::fs::remove_file(old).map_err(|e| e.to_string())?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_model::{ClipSource, AudioEdit};

    fn folder() -> PathBuf {
        let folder = std::env::temp_dir().join(format!("daw-files-{}", stamp()));
        std::fs::create_dir(&folder).unwrap();
        folder
    }

    #[test]
    fn package_relocates_audio_and_sampler_assets_without_originals() {
        let root = folder();
        let original = root.join("source.wav");
        std::fs::write(&original, b"test audio bytes").unwrap();
        let mut project = Project::new();
        let audio = project.add_channel("audio", Source::Audio { path: original.to_string_lossy().into_owned() });
        project.add_channel("sampler", Source::Sampler { path: original.to_string_lossy().into_owned(), root_key: 60 });
        project.add_audio_clip(0, 960, audio, 480);
        project.playlist.clips[0].audio = AudioEdit { stretch: 2.0, semitones: 12.0, reverse: true };
        let archive = root.join("portable.dawzip");
        pack(&archive, &project).unwrap();
        std::fs::remove_file(original).unwrap();
        let extracted = root.join("relocated");
        let path = unpack(&archive, &extracted).unwrap();
        let mut loaded = load(&path).unwrap();
        let Source::Audio { path } = &loaded.channel(audio).unwrap().source else { panic!("audio") };
        assert_eq!(std::fs::read(path).unwrap(), b"test audio bytes");
        assert!(Path::new(path).starts_with(std::fs::canonicalize(&extracted).unwrap()));
        let Source::Sampler { path: sampler, .. } = &loaded.channels.last().unwrap().source else { panic!("sampler") };
        assert_eq!(path, sampler);
        assert_eq!(loaded.playlist.clips[0].audio, project.playlist.clips[0].audio);
        assert_eq!(std::fs::read_dir(extracted.join("assets")).unwrap().count(), 1);
        // A regular save embeds the assets again.
        save(&extracted.join("saved.dawproj"), &loaded).unwrap();
        loaded = load(&extracted.join("saved.dawproj")).unwrap();
        assert_eq!(loaded.playlist.clips[0].source, ClipSource::Audio(audio));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_pack_preserves_existing_file_and_snapshots_rotate() {
        let root = folder();
        let archive = root.join("keep.dawzip");
        std::fs::write(&archive, b"existing package").unwrap();
        let mut project = Project::new();
        project.add_channel("missing", Source::Audio { path: root.join("absent.wav").to_string_lossy().into_owned() });
        assert!(pack(&archive, &project).is_err());
        assert_eq!(std::fs::read(&archive).unwrap(), b"existing package");
        let project = Project::new();
        for _ in 0..12 { snapshot(&root.join("backups"), &project).unwrap(); }
        let paths: Vec<_> = std::fs::read_dir(root.join("backups")).unwrap().map(|p| p.unwrap().path()).collect();
        assert_eq!(paths.len(), 10);
        for path in paths { assert_eq!(load(&path).unwrap(), project); }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn normal_saves_and_backups_embed_assets_and_upgrade_legacy_projects() {
        let root = folder();
        let original = root.join("external.wav");
        std::fs::write(&original, b"source audio bytes").unwrap();
        let mut project = Project::new();
        project.add_channel("audio", Source::Audio { path: "external.wav".into() });
        project.add_channel("sampler", Source::Sampler { path: "external.wav".into(), root_key: 60 });
        let legacy = root.join("legacy.dawproj");
        std::fs::write(&legacy, project.to_ron().unwrap()).unwrap();
        let project = load(&legacy).unwrap();
        let saved = root.join("saved.dawproj");
        save(&saved, &project).unwrap();
        let backup = snapshot(&root.join("backups"), &project).unwrap();
        let mut zip = ZipArchive::new(std::fs::File::open(&saved).unwrap()).unwrap();
        assert_eq!(zip.len(), 2, "one shared asset and one project document");
        let mut document = String::new();
        zip.by_name("project.dawproj").unwrap().read_to_string(&mut document).unwrap();
        let stored = Project::from_ron(&document).unwrap();
        assert!(stored.channels.iter().filter_map(|c| match &c.source {
            Source::Audio { path } | Source::Sampler { path, .. } => Some(path), _ => None,
        }).all(|path| path == "assets/0.wav"));
        drop(zip);
        std::fs::remove_file(original).unwrap();
        for path in [&saved, &backup] {
            let mut loaded = load(path).unwrap();
            for channel in &mut loaded.channels {
                if let Some(path) = audio_path(&mut channel.source) {
                    assert_eq!(std::fs::read(path).unwrap(), b"source audio bytes");
                }
            }
        }
        let missing = root.join("missing.dawproj");
        assert!(save(&missing, &project).is_err());
        assert!(!missing.exists());
        let bytes = std::fs::read(&saved).unwrap();
        assert!(save(&saved, &project).is_err());
        assert_eq!(std::fs::read(&saved).unwrap(), bytes);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extraction_rejects_traversal_and_keeps_existing_folders() {
        let root = folder();
        let archive = root.join("unsafe.dawzip");
        let mut zip = ZipWriter::new(std::fs::File::create(&archive).unwrap());
        zip.start_file("../escaped", SimpleFileOptions::default()).unwrap();
        zip.write_all(b"outside").unwrap();
        zip.finish().unwrap();
        let extracted = root.join("extracted");
        assert!(unpack(&archive, &extracted).is_err());
        assert!(!extracted.exists());
        assert!(!root.join("escaped").exists());
        assert!(unpack(&archive, &root).is_err());
        assert!(root.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
