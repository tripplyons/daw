use clap::Parser;
use daw_model::time::TimeSignature;
use daw_model::{PluginFormat, PluginRef, Project, Source, Target};
use daw_plugins::scan::Catalog;
use daw_plugins::{PluginInfo, PluginKind};

use super::parse::{self, Time};
use super::{Cli, Command, edit};

fn edit(project: &mut Project, args: &str) -> Result<String, String> {
    edit_with(project, args, Catalog::default())
}

fn edit_with(project: &mut Project, args: &str, catalog: Catalog) -> Result<String, String> {
    let words = ["daw", "edit", "song.dawproj"].into_iter().chain(args.split_whitespace());
    let cli = Cli::try_parse_from(words).map_err(|e| e.to_string())?;
    let Some(Command::Edit { op, .. }) = cli.command else { panic!("not an edit command") };
    edit::apply(project, op, || catalog)
}

fn id(output: Result<String, String>) -> u64 {
    output.unwrap().parse().unwrap()
}

#[test]
fn times_parse_and_print_in_the_same_form() {
    let sig = TimeSignature::default();
    let ticks = |text: &str| text.parse::<Time>().unwrap().ticks(sig);
    assert_eq!(ticks("960"), 960);
    assert_eq!(ticks("2bar"), 7680);
    assert_eq!(ticks("1bar+2beat"), 5760);
    assert_eq!(ticks("3step"), 720);
    assert_eq!(ticks("0.5beat"), 480);
    assert_eq!("3bar".parse::<Time>().unwrap().ticks(TimeSignature { numerator: 3, denominator: 4 }), 8640);
    assert!("2bars+x".parse::<Time>().is_err());
    assert!("-1beat".parse::<Time>().is_err());
    for t in [0, 100, 240, 960, 3840, 5760] {
        assert_eq!(ticks(&parse::time(t, sig)), t);
    }
}

#[test]
fn keys_parse_and_print_in_the_same_form() {
    assert_eq!(parse::key("C4"), Ok(60));
    assert_eq!(parse::key("c#4"), Ok(61));
    assert_eq!(parse::key("Bb2"), Ok(46));
    assert_eq!(parse::key("C-1"), Ok(0));
    assert_eq!(parse::key("64"), Ok(64));
    assert!(parse::key("H4").is_err());
    assert_eq!(parse::key("G9"), Ok(127));
    assert!(parse::key("G#9").is_err());
    for key in 0..=127 {
        assert_eq!(parse::key(&parse::key_name(key)), Ok(key));
    }
}

#[test]
fn targets_and_shapes_parse_and_print_in_the_same_form() {
    for text in ["tempo", "channel-volume:3", "channel-pan:3", "cutoff:3", "insert-volume:1", "insert-pan:1", "plugin:7:42"] {
        assert_eq!(parse::target_name(parse::target(text).unwrap()), text);
    }
    assert!(parse::target("plugin:7").is_err());
    for text in ["linear", "curve", "s-curve", "hold", "stairs:4", "pulse:8"] {
        assert_eq!(parse::shape_name(parse::shape(text).unwrap()), text);
    }
}

#[test]
fn notes_grow_the_pattern_and_steps_replace_a_lane() {
    let mut project = Project::new();
    let channel = project.channels[0].id;
    let pattern = project.patterns[0].id;
    let bar = project.signature.ticks_per_bar();

    edit(&mut project, &format!("note add {} {} E4 1bar+1beat 1beat --velocity 0.5", pattern.0, channel.0)).unwrap();
    let notes = project.pattern(pattern).unwrap().notes(channel);
    assert_eq!((notes[0].key, notes[0].start, notes[0].velocity), (64, bar + 960, 0.5));
    assert_eq!(project.pattern(pattern).unwrap().length, bar * 2);

    edit(&mut project, &format!("note steps {} {} x...|x...|x...|x... --key C2", pattern.0, channel.0)).unwrap();
    let starts: Vec<u64> = project.pattern(pattern).unwrap().notes(channel).iter().map(|n| n.start).collect();
    assert_eq!(starts, vec![0, 960, 1920, 2880]);

    let removed = edit(&mut project, &format!("note remove {} {} --start 1beat", pattern.0, channel.0)).unwrap();
    assert_eq!(removed, "removed 1 note");
    assert!(edit(&mut project, &format!("note add {} 999 C4 0 1beat", pattern.0)).unwrap_err().contains("no channel 999"));
    assert!(edit(&mut project, &format!("note add {} {} C4 0 0", pattern.0, channel.0)).is_err());
}

#[test]
fn clips_add_missing_tracks_and_default_to_the_whole_source() {
    let mut project = Project::new();
    let pattern = project.patterns[0].id;
    let clip = id(edit(&mut project, &format!("clip add pattern:{} 20 2bar", pattern.0)));
    assert_eq!(project.playlist.tracks.len(), 21);
    let placed = project.playlist.clips.iter().find(|c| c.id.0 == clip).unwrap();
    assert_eq!((placed.track, placed.start, placed.length), (20, 7680, 3840));

    edit(&mut project, &format!("clip set {clip} --offset 1beat --length 3beat")).unwrap();
    let placed = project.playlist.clips.iter().find(|c| c.id.0 == clip).unwrap();
    assert_eq!((placed.offset, placed.length), (960, 2880));
    assert!(edit(&mut project, &format!("clip set {clip} --track 99")).is_err());
    assert!(edit(&mut project, "clip add pattern:999 0 0").unwrap_err().contains("no pattern 999"));
}

#[test]
fn automation_starts_at_the_target_value_and_points_stay_sorted() {
    let mut project = Project::new();
    project.bpm = 140.0;
    let clip = id(edit(&mut project, "automation add tempo --length 2bar"));
    let automation = project.automation_clip(daw_model::AutomationId(clip)).unwrap();
    assert_eq!(automation.envelope.points[0].value, daw_model::automation::tempo_to_normalized(140.0));
    assert_eq!(automation.envelope.points[1].time, 7680);
    assert!(edit(&mut project, "automation add tempo").unwrap_err().contains("already has automation"));
    assert!(edit(&mut project, "automation add channel-volume:999").is_err());

    assert_eq!(edit(&mut project, &format!("point add {clip} 1bar 0.9 --shape curve --tension -0.5")).unwrap(), "1");
    assert_eq!(edit(&mut project, &format!("point set {clip} 1 --time 3bar")).unwrap(), "2");
    let automation = project.automation_clip(daw_model::AutomationId(clip)).unwrap();
    let times: Vec<u64> = automation.envelope.points.iter().map(|p| p.time).collect();
    assert_eq!(times, vec![0, 7680, 11520]);
    assert_eq!(automation.length, 11520, "the clip grows to whole bars");
}

#[test]
fn edits_refuse_what_the_app_does_not_allow() {
    let mut project = Project::new();
    let [a, b] = [1, 2].map(|i| project.mixer.inserts[i].id.0);
    edit(&mut project, &format!("insert set {a} --output {b} --pan -0.5")).unwrap();
    assert_eq!(project.mixer.inserts[1].pan, -0.5);
    assert!(edit(&mut project, &format!("insert set {b} --output {a}")).unwrap_err().contains("loop"));
    assert!(edit(&mut project, "insert remove 0").is_err());
    let pattern = project.patterns[0].id.0;
    assert!(edit(&mut project, &format!("pattern remove {pattern}")).unwrap_err().contains("at least one"));
    let channel = project.channels[0].id.0;
    assert!(edit(&mut project, &format!("channel set {channel} --sample kick.wav")).is_err());
    assert!(edit(&mut project, &format!("channel set {channel} --volume 1.5")).is_err());
}

#[test]
fn plugins_resolve_by_id_or_unambiguous_name() {
    let info = |format, id: &str, name: &str, kind| PluginInfo {
        plugin: PluginRef { format, id: id.into(), path: String::new(), name: name.into(), vendor: String::new() },
        kind,
        category: String::new(),
    };
    let catalog = Catalog {
        plugins: vec![
            info(PluginFormat::Vst3, "V1", "Vital", PluginKind::Instrument),
            info(PluginFormat::AudioUnit, "A1", "Vital", PluginKind::Instrument),
            info(PluginFormat::Vst3, "O1", "OTT", PluginKind::Effect),
        ],
        failed: Vec::new(),
    };
    let mut project = Project::new();
    assert!(edit_with(&mut project, "channel add lead --plugin vital", catalog.clone()).unwrap_err().contains("several"));
    let channel = id(edit_with(&mut project, "channel add lead --plugin V1", catalog.clone()));
    let Source::Plugin(instance) = project.channel(daw_model::ChannelId(channel)).unwrap().source else { panic!() };
    assert_eq!(project.plugin(instance).unwrap().plugin.id, "V1");
    assert!(edit_with(&mut project, "effect add 1 V1", catalog.clone()).unwrap_err().contains("not an effect"));
    let effect = id(edit_with(&mut project, "effect add 1 ott", catalog.clone()));
    assert_eq!(project.mixer.inserts[1].effects.len(), 1);
    edit(&mut project, &format!("automation add plugin:{effect}:3")).unwrap();
    edit(&mut project, &format!("effect remove {effect}")).unwrap();
    assert!(project.mixer.inserts[1].effects.is_empty());
    assert!(project.automation_for(Target::Plugin { instance: daw_model::InstanceId(effect), param: 3 }).is_none());
    assert!(edit(&mut project, "channel add x --plugin vital").unwrap_err().contains("plugin cache is empty"));
}

#[test]
fn param_assignments_resolve_by_id_or_name() {
    use daw_plugins::ParamInfo;
    let param = |id, name: &str| ParamInfo { id, name: name.into(), units: String::new(), steps: 0, default: 0.0, automatable: true };
    let params = vec![param(1, "Mix"), param(2, "Cutoff"), param(3, "Gain"), param(4, "Gain")];
    assert_eq!(super::plugins::parse_assignment("Sync Mode=1"), Ok(("Sync Mode".into(), 1.0)));
    assert_eq!(super::plugins::parse_assignment("a=b=0.5"), Ok(("a=b".into(), 0.5)));
    assert!(super::plugins::parse_assignment("Mix").is_err());
    assert!(super::plugins::parse_assignment("Mix=1.5").is_err());
    assert_eq!(super::plugins::find_param(&params, "mix").unwrap().id, 1);
    assert_eq!(super::plugins::find_param(&params, "2").unwrap().name, "Cutoff");
    assert!(super::plugins::find_param(&params, "Gain").unwrap_err().contains("use an id"));
    assert!(super::plugins::find_param(&params, "Drive").is_err());
}

#[test]
fn batch_keeps_outputs_as_variables_and_stops_at_errors() {
    let mut project = Project::new();
    let script = "
        # comments and blank lines are skipped

        p = pattern add --name 'lead line' --length 2bar
        lead = channel add lead --synth square
        note add $p $lead C4 0 1beat
        note add ${p} $lead E4 1beat 1beat
        clip add pattern:$p 3 0
    ";
    let output = super::batch::run(&mut project, script).unwrap();
    let pattern = project.patterns.iter().find(|p| p.name == "lead line").unwrap();
    let lead = project.channels.iter().find(|c| c.name == "lead").unwrap();
    assert_eq!(output, format!("p = {}\nlead = {}\n{}", pattern.id.0, lead.id.0, project.playlist.clips[0].id.0));
    assert_eq!(pattern.notes(lead.id).len(), 2);

    let error = super::batch::run(&mut project, "x = channel add x\nnote add 999 $x C4 0 1beat").unwrap_err();
    assert!(error.starts_with("line 2: no pattern 999"), "{error}");
    assert!(super::batch::run(&mut project, "note add $missing 9 C4 0 1beat").unwrap_err().contains("$missing is not set"));
    assert!(super::batch::run(&mut project, "channel frobnicate").unwrap_err().starts_with("line 1:"));
}

#[test]
fn analyze_finds_a_tone_in_its_octave_band() {
    let path = std::env::temp_dir().join(format!("daw-analyze-{}.wav", std::process::id()));
    let spec = hound::WavSpec { channels: 2, sample_rate: 48_000, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut writer = hound::WavWriter::create(&path, spec).unwrap();
    for i in 0..48_000 {
        let s = 0.5 * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48_000.0).sin();
        writer.write_sample(s).unwrap();
        writer.write_sample(s).unwrap();
    }
    writer.finalize().unwrap();
    let report = super::analyze::analyze(&[&path], None).unwrap();
    let row = report.lines().nth(2).unwrap();
    let numbers: Vec<f64> = row.split(|c: char| c.is_whitespace() || c == '|').filter_map(|w| w.parse().ok()).collect();
    let (peak, rms, width, bands) = (numbers[0], numbers[1], numbers[2], &numbers[3..]);
    assert!((peak - -6.0).abs() < 0.1, "{row}");
    assert!((rms - -9.0).abs() < 0.1, "{row}");
    assert_eq!(width, -99.9, "identical channels have no side signal");
    let loudest = bands.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
    assert_eq!(loudest, 4, "1 kHz lands in the 1k band: {row}");
    std::fs::remove_file(path).unwrap();
}
