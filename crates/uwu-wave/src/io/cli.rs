//! The command-line runner behind [`Song::run`](crate::song::Song::run).
//!
//! ```text
//! cargo run --release --example my_song -- [options]
//!
//!   (no options)            render to out/<name>.wav and print a report
//!   -o, --out <file>        output file (.wav)
//!   -p, --play              play in real time instead of rendering a file
//!       --loop              with --play: loop the (--from/--to) range forever
//!       --from <pos>        start here      pos = bar | bar:beat | <secs>s | section/marker name
//!       --to <pos>          stop here       (a section name means the end of that section)
//!       --solo a,b          only these tracks
//!       --mute a,b          silence these tracks
//!       --stems [dir]       also write one wav per track/bus (default out/<name>_stems/)
//!       --midi <file.mid>   also export the notes as a Standard MIDI File
//!       --sr <hz>           sample rate
//!       --bits 16|24|32     wav format (32 = float; default 24)
//!       --info              print the arrangement and exit
//!       --no-tail           stop exactly at the end (no reverb/release tail)
//!   -h, --help
//! ```
//! Bars and beats are 0-based, like [`bar`](crate::time::bar) in code: `--from 8:2` is bar 8, beat 2.

use super::wav::{BitDepth, write_wav};
use crate::engine::{RenderOptions, sanitize};
use crate::song::{ChanKind, Song};
use crate::time::Time;
use std::path::PathBuf;

/// Parsed command-line options.
#[derive(Clone, Debug, PartialEq)]
pub struct Args {
    pub out: Option<PathBuf>,
    pub play: bool,
    pub looping: bool,
    pub from: Option<String>,
    pub to: Option<String>,
    pub solo: Vec<String>,
    pub mute: Vec<String>,
    /// `Some(None)` = stems into the default folder.
    pub stems: Option<Option<PathBuf>>,
    pub midi: Option<PathBuf>,
    pub sr: Option<u32>,
    pub bits: BitDepth,
    pub info: bool,
    pub no_tail: bool,
    pub help: bool,
}

impl Default for Args {
    fn default() -> Args {
        Args {
            out: None,
            play: false,
            looping: false,
            from: None,
            to: None,
            solo: vec![],
            mute: vec![],
            stems: None,
            midi: None,
            sr: None,
            bits: BitDepth::Int24,
            info: false,
            no_tail: false,
            help: false,
        }
    }
}

/// Usage text.
pub const USAGE: &str = "\
usage: cargo run --release --example <song> -- [options]

  (no options)            render to out/<name>.wav and print a report
  -o, --out <file>        output file (.wav)
  -p, --play              play in real time instead of rendering a file
      --loop              with --play: loop the (--from/--to) range
      --from <pos>        start position: bar | bar:beat | <secs>s | section/marker name
      --to <pos>          end position (a section name means the end of that section)
      --solo a,b          only these tracks are audible
      --mute a,b          silence these tracks
      --stems [dir]       also write one wav per track/bus (default out/<name>_stems/)
      --midi <file.mid>   also export the notes as a Standard MIDI File
      --sr <hz>           sample rate (default: the song's, usually 48000)
      --bits 16|24|32     wav format (32 = float; default 24)
      --info              print the arrangement and exit
      --no-tail           stop exactly at the end (no reverb/release tail)
  -h, --help              this help

bars and beats are 0-based like bar(n) in code: --from 8:2 = bar 8, beat 2";

fn split_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

/// Parse arguments (without the program name). Accepts `--flag value` and `--flag=value`.
pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = args.into_iter().peekable();
    while let Some(raw) = it.next() {
        let (flag, inline) = match raw.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (raw.clone(), None),
        };
        let mut inline = inline;
        let mut value = |name: &str| -> Result<String, String> {
            if let Some(v) = inline.take() {
                return Ok(v);
            }
            it.next().ok_or_else(|| format!("{name} needs a value"))
        };
        match flag.as_str() {
            "-h" | "--help" => a.help = true,
            "-o" | "--out" => a.out = Some(PathBuf::from(value(&flag)?)),
            "-p" | "--play" => a.play = true,
            "--loop" => a.looping = true,
            "--from" => a.from = Some(value(&flag)?),
            "--to" => a.to = Some(value(&flag)?),
            "--solo" => a.solo.extend(split_list(&value(&flag)?)),
            "--mute" => a.mute.extend(split_list(&value(&flag)?)),
            "--midi" => a.midi = Some(PathBuf::from(value(&flag)?)),
            "--info" => a.info = true,
            "--no-tail" => a.no_tail = true,
            "--sr" => {
                let v = value(&flag)?;
                let sr: u32 = v
                    .trim_end_matches("hz")
                    .trim_end_matches("Hz")
                    .parse()
                    .map_err(|_| format!("bad sample rate {v:?}"))?;
                if !(8000..=384_000).contains(&sr) {
                    return Err(format!("sample rate {sr} out of range"));
                }
                a.sr = Some(sr);
            }
            "--bits" => {
                a.bits = match value(&flag)?.as_str() {
                    "16" => BitDepth::Int16,
                    "24" => BitDepth::Int24,
                    "32" | "32f" | "float" => BitDepth::Float32,
                    v => return Err(format!("--bits must be 16, 24 or 32 (got {v:?})")),
                }
            }
            "--stems" => {
                a.stems = Some(match inline.take() {
                    Some(v) => Some(PathBuf::from(v)),
                    None => match it.peek() {
                        Some(n) if !n.starts_with('-') => Some(PathBuf::from(it.next().unwrap())),
                        _ => None,
                    },
                });
            }
            f => return Err(format!("unknown option {f:?}")),
        }
    }
    Ok(a)
}

fn beats_per_bar(song: &Song) -> f64 {
    let (n, d) = song.meter_sig();
    n as f64 * 4.0 / d.max(1) as f64
}

/// Resolve a position: `12` (bar), `12:2.5` (bar:beat), `30s` / `1:30s` (seconds / minutes:seconds),
/// or a section/marker name. For an end position a section name means the section's end.
pub fn parse_position(song: &Song, s: &str, is_end: bool) -> Result<Time, String> {
    let s = s.trim();
    let bpb = beats_per_bar(song);
    if let Some(sec) = s.strip_suffix('s').filter(|x| {
        x.chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
    }) {
        let secs = match sec.split_once(':') {
            Some((m, x)) => m
                .parse::<f64>()
                .ok()
                .zip(x.parse::<f64>().ok())
                .map(|(m, x)| m * 60.0 + x),
            None => sec.parse::<f64>().ok(),
        };
        if let Some(secs) = secs {
            return Ok(Time(secs_to_beats(song, secs)));
        }
    }
    if let Ok(b) = s.parse::<f64>() {
        return Ok(Time(b * bpb));
    }
    if let Some((b, beat)) = s.split_once(':') {
        if let (Ok(b), Ok(beat)) = (b.parse::<f64>(), beat.parse::<f64>()) {
            return Ok(Time(b * bpb + beat));
        }
    }
    if let Some(sec) = song.sections().into_iter().find(|x| x.name == s) {
        return Ok(if is_end { sec.end() } else { sec.start });
    }
    if let Some(t) = song.locate(s) {
        return Ok(t);
    }
    let names: Vec<String> = song.markers().into_iter().map(|(n, _)| n).collect();
    Err(format!(
        "unknown position {s:?} (sections/markers: {})",
        if names.is_empty() {
            "none".into()
        } else {
            names.join(", ")
        }
    ))
}

fn secs_to_beats(song: &Song, target: f64) -> f64 {
    let tempo = song.tempo_param();
    let (mut beat, mut secs) = (0.0f64, 0.0f64);
    let step = 1.0 / 32.0;
    while secs < target {
        let bpm = (tempo.at(crate::param::Clock { beat, secs }) as f64).max(1.0);
        let dt = step * 60.0 / bpm;
        if secs + dt >= target {
            return beat + (target - secs) / dt * step;
        }
        secs += dt;
        beat += step;
    }
    beat
}

fn fmt_secs(s: f64) -> String {
    let s = s.max(0.0);
    format!("{}:{:04.1}", (s / 60.0).floor() as u64, s % 60.0)
}

/// An ASCII arrangement view: tempo/meter/length, a section header, and one row per track with
/// `▇` wherever it has clips.
pub fn arrangement(song: &Song) -> String {
    let bpb = beats_per_bar(song);
    let end = song.end().0;
    let bars = (end / bpb).ceil().max(1.0) as usize;
    let per = bars.div_ceil(96).max(1); // bars per column
    let cols = bars.div_ceil(per);
    let (tm, secs) = super::midi::tempo_map(song, end);
    let (num, den) = song.meter_sig();
    let tempo = if tm.len() == 1 {
        format!("{:.0} bpm", tm[0].1)
    } else {
        let lo = tm.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
        let hi = tm.iter().map(|p| p.1).fold(0.0, f64::max);
        format!(
            "{:.0}→{:.0} bpm ({:.0}..{:.0}, automated)",
            tm[0].1,
            tm.last().unwrap().1,
            lo,
            hi
        )
    };
    let mut o = String::new();
    o += &format!(
        "♪ {} — {tempo}, {num}/{den}, {bars} bars, {}\n",
        song.name(),
        fmt_secs(secs)
    );
    let w = 14;
    let col_of = |beat: f64| ((beat / bpb) / per as f64).floor().max(0.0) as usize;
    // ruler
    let mut ruler = vec![' '; cols + 8];
    let step = if per == 1 { 8 } else { 8 * per };
    let mut b = 0;
    while b < bars {
        let c = b / per;
        for (k, ch) in b.to_string().chars().enumerate() {
            if c + k < ruler.len() {
                ruler[c + k] = ch;
            }
        }
        b += step;
    }
    o += &format!(
        "{:<w$} {}\n",
        if per == 1 { "bar" } else { "bar (×{per})" }.replace("{per}", &per.to_string()),
        ruler.iter().collect::<String>().trim_end()
    );
    // sections
    let secs_info = song.sections();
    if !secs_info.is_empty() {
        let mut row = vec![' '; cols + 1];
        for s in &secs_info {
            let (a, b) = (
                col_of(s.start.0),
                col_of(s.end().0 - 1e-9).max(col_of(s.start.0)),
            );
            if a < row.len() {
                row[a] = '|';
            }
            for (k, ch) in s.name.chars().enumerate() {
                let c = a + 1 + k;
                if c > b || c >= row.len() {
                    break;
                }
                row[c] = ch;
            }
        }
        o += &format!(
            "{:<w$} {}\n",
            "sections",
            row.iter().collect::<String>().trim_end()
        );
    }
    for t in song.tracks() {
        let name: String = t.name().chars().take(w).collect();
        if t.kind() != ChanKind::Track {
            continue;
        }
        let regions = t.regions();
        let row: String = (0..cols)
            .map(|c| {
                let (a, b) = ((c * per) as f64 * bpb, ((c + 1) * per) as f64 * bpb);
                if regions.iter().any(|&(s, e)| s < b && e > a) {
                    '▇'
                } else {
                    '·'
                }
            })
            .collect();
        o += &format!("{name:<w$} {row}  {} notes\n", t.notes().len());
    }
    let buses: Vec<String> = song
        .tracks()
        .into_iter()
        .filter(|t| t.kind() == ChanKind::Bus)
        .map(|t| t.name())
        .collect();
    if !buses.is_empty() {
        o += &format!("buses: {}\n", buses.join(", "));
    }
    let marks: Vec<String> = song
        .markers()
        .into_iter()
        .filter(|(n, _)| !secs_info.iter().any(|s| &s.name == n))
        .map(|(n, t)| format!("{n}@{t}"))
        .collect();
    if !marks.is_empty() {
        o += &format!("markers: {}\n", marks.join("  "));
    }
    o
}

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1)
}

/// Run with the process arguments (called by [`Song::run`]).
pub fn run(song: Song) {
    run_with(song, std::env::args().skip(1))
}

/// Run with explicit arguments.
pub fn run_with<I: IntoIterator<Item = String>>(song: Song, args: I) {
    let a = match parse_args(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    if a.help {
        println!("{USAGE}");
        return;
    }
    if a.info {
        print!("{}", arrangement(&song));
        return;
    }
    let name = sanitize(&song.name());
    if let Some(m) = &a.midi {
        match super::midi::write_midi(&song, m) {
            Ok(()) => println!("→ {}", m.display()),
            Err(e) => fail(e),
        }
    }
    let from = a
        .from
        .as_deref()
        .map(|s| parse_position(&song, s, false).unwrap_or_else(|e| fail(e)));
    let to =
        a.to.as_deref()
            .map(|s| parse_position(&song, s, true).unwrap_or_else(|e| fail(e)));
    let opts = RenderOptions {
        sample_rate: a.sr,
        from,
        to,
        solo: a.solo.clone(),
        mute: a.mute.clone(),
        stems: a.stems.is_some(),
        no_tail: a.no_tail,
    };
    if a.play {
        #[cfg(feature = "play")]
        match super::play::play(&song, opts.clone(), a.looping) {
            Ok(report) => {
                println!("{report}");
                return;
            }
            Err(super::play::PlayError::Render(e)) => fail(e),
            Err(super::play::PlayError::NoDevice(e)) => {
                eprintln!("⚠ no audio output ({e}); rendering to a file instead");
            }
        }
        #[cfg(not(feature = "play"))]
        eprintln!("⚠ built without the `play` feature; rendering to a file instead");
    } else if a.looping {
        eprintln!("note: --loop only makes sense with --play");
    }
    let mut engine = song.engine(&opts).unwrap_or_else(|e| fail(e));
    let r = engine.render_all();
    let path = a
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("out/{name}.wav")));
    if let Err(e) = write_wav(&path, r.sr, &r.audio, a.bits) {
        fail(e);
    }
    println!("{}\n→ {}", r.report, path.display());
    if let Some(dir) = &a.stems {
        let dir = dir
            .clone()
            .unwrap_or_else(|| PathBuf::from(format!("out/{name}_stems")));
        match r.write_stems(&dir) {
            Ok(files) => println!("→ {} stems in {}", files.len(), dir.display()),
            Err(e) => fail(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::Clip;
    use crate::instrument::Silence;
    use crate::units::Units;

    fn args(s: &str) -> Result<Args, String> {
        parse_args(s.split_whitespace().map(String::from))
    }

    #[test]
    fn parses_flags() {
        let a = args("-o x.wav --play --loop --from verse --to 12:2 --solo a,b --mute=c --stems --midi m.mid --sr 44100 --bits 16 --no-tail").unwrap();
        assert_eq!(a.out, Some(PathBuf::from("x.wav")));
        assert!(a.play && a.looping && a.no_tail);
        assert_eq!(a.from.as_deref(), Some("verse"));
        assert_eq!(a.to.as_deref(), Some("12:2"));
        assert_eq!(a.solo, vec!["a", "b"]);
        assert_eq!(a.mute, vec!["c"]);
        assert_eq!(a.stems, Some(None));
        assert_eq!(a.sr, Some(44100));
        assert_eq!(a.bits, BitDepth::Int16);
        assert_eq!(
            args("--stems stems/").unwrap().stems,
            Some(Some(PathBuf::from("stems/")))
        );
        assert!(args("--bogus").is_err());
        assert!(args("--bits 12").is_err());
        assert!(args("--from").is_err());
    }

    #[test]
    fn positions_and_info() {
        let song = Song::new("pos").bpm(120);
        let t = song.track("lead", Silence);
        let _bus = song.bus("verb");
        song.section("intro", 4.bars(), |_| {});
        song.section("verse", 8.bars(), |s| {
            s.play(&t, Clip::new(4.beats()).note(0, 1, 60, 1.0));
        });
        assert_eq!(parse_position(&song, "2", false).unwrap(), Time(8.0));
        assert_eq!(parse_position(&song, "2:1.5", false).unwrap(), Time(9.5));
        assert_eq!(parse_position(&song, "verse", false).unwrap(), Time(16.0));
        assert_eq!(parse_position(&song, "verse", true).unwrap(), Time(48.0));
        assert!((parse_position(&song, "4s", false).unwrap().0 - 8.0).abs() < 1e-6);
        assert!(parse_position(&song, "nope", false).is_err());
        let info = arrangement(&song);
        assert!(info.contains("|int|verse"), "{info}");
        let row = info.lines().find(|l| l.starts_with("lead")).unwrap();
        assert!(row.contains("····▇▇▇▇▇▇▇▇"), "{info}");
        assert!(info.contains("buses: verb"));
    }
}
