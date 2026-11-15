//! `uwu library build`: builds the structured sample library (`assets/library`) from the source
//! mirrors (Virtual Playing Orchestra, Philharmonia MP3s, the piano set, cluesurf/wave).
//!
//! Each articulation folder is rewritten from scratch. Audio is hard-linked from the sources
//! when possible (VPO, piano), decoded with `ffmpeg` (Philharmonia MP3 → trimmed 16-bit mono
//! WAV), or referenced remotely (wavebase LFS files: `url=`/`size=`/`sha256=`, fetched lazily).

use super::lfs::{self, LfsPointer};
use super::library::{Manifest, ZoneSpec};
use super::load::parse_note;
use crate::Error;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Builder options.
#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// Folder holding the source mirrors (`VirtualPlayingOrchestra-main/`,
    /// `philharmonia-samples-gh-pages/`, `piano/`, `wavebase/`).
    pub sources: PathBuf,
    /// Output library folder.
    pub out: PathBuf,
    /// Only these parts (`vpo phil piano wavebase`); empty = all.
    pub only: Vec<String>,
    /// Parallel ffmpeg jobs.
    pub jobs: usize,
}

/// One built patch (for INDEX.md).
#[derive(Clone, Debug)]
pub struct Built {
    pub id: String,
    pub zones: usize,
    pub lo: f32,
    pub hi: f32,
    pub dyns: Vec<String>,
    pub source: &'static str,
    pub remote: usize,
}

/// Default sources folder: `<assets>` if it still holds the mirrors, else
/// `~/.cache/uwu-wave/sources`.
pub fn default_sources() -> PathBuf {
    if let Some(a) = super::load::assets_dir()
        && (a.join("VirtualPlayingOrchestra-main").is_dir()
            || a.join("philharmonia-samples-gh-pages").is_dir())
    {
        return a;
    }
    lfs::cache_dir().join("sources")
}

// ---------------------------------------------------------------------------------------------
// small helpers

/// A note name in any of the source spellings: `a#3`, `Db2`, `F#6`, `3_Gb` (octave first),
/// `Ax2`, `c4`. Returns the MIDI number.
pub fn note_any(s: &str) -> Option<f32> {
    if let Some((o, n)) = s.split_once('_')
        && o.len() == 1
        && o.as_bytes()[0].is_ascii_digit()
    {
        let n: String = n
            .chars()
            .take_while(|c| c.is_ascii_alphabetic() || *c == '#')
            .collect();
        return parse_note(&format!("{n}{o}"));
    }
    let mut t = s.to_string();
    if let Some(f) = t.get(..1) {
        t = f.to_ascii_uppercase() + &t[1..];
    }
    parse_note(&t)
}

/// MIDI note → name like `C#4`.
pub fn note_name(m: f32) -> String {
    const N: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    if !m.is_finite() {
        return "?".into();
    }
    let m = m.round() as i32;
    format!("{}{}", N[m.rem_euclid(12) as usize], m.div_euclid(12) - 1)
}

/// File-system friendly lowercase name.
pub fn clean_name(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        match c {
            '#' => o.push('s'),
            ' ' | '/' => o.push('-'),
            c if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' => {
                o.push(c.to_ascii_lowercase())
            }
            _ => o.push('_'),
        }
    }
    o
}

fn link_or_copy(src: &Path, dst: &Path) -> Result<(), Error> {
    if dst.exists() {
        std::fs::remove_file(dst)?;
    }
    if std::fs::hard_link(src, dst).is_err() {
        std::fs::copy(src, dst)?;
    }
    Ok(())
}

fn fresh_dir(d: &Path) -> Result<(), Error> {
    if d.exists() {
        std::fs::remove_dir_all(d)?;
    }
    std::fs::create_dir_all(d)?;
    Ok(())
}

/// `(sample_rate, frames, smpl loop)` from a WAV header (RIFF chunk walk, no decoding).
/// `(sample_rate, frames, smpl loop)`.
pub type WavInfo = (u32, u64, Option<(u64, u64)>);

pub fn wav_info(path: &Path) -> Option<WavInfo> {
    let d = std::fs::read(path).ok()?;
    if d.len() < 12 || &d[..4] != b"RIFF" || &d[8..12] != b"WAVE" {
        return None;
    }
    let (mut sr, mut block, mut data_len, mut lp) = (0u32, 1u64, 0u64, None);
    let mut i = 12;
    while i + 8 <= d.len() {
        let id = &d[i..i + 4];
        let sz = u32::from_le_bytes([d[i + 4], d[i + 5], d[i + 6], d[i + 7]]) as usize;
        let body = &d[(i + 8).min(d.len())..(i + 8 + sz).min(d.len())];
        match id {
            b"fmt " if body.len() >= 16 => {
                sr = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
                block = u16::from_le_bytes([body[12], body[13]]).max(1) as u64;
            }
            b"data" => data_len = sz as u64,
            b"smpl" if body.len() >= 60 => {
                let n = u32::from_le_bytes([body[28], body[29], body[30], body[31]]);
                if n > 0 {
                    let a = u32::from_le_bytes([body[44], body[45], body[46], body[47]]) as u64;
                    let b = u32::from_le_bytes([body[48], body[49], body[50], body[51]]) as u64;
                    if b > a + 16 {
                        lp = Some((a, b));
                    }
                }
            }
            _ => {}
        }
        i += 8 + sz + (sz & 1);
    }
    Some((sr, data_len / block, lp))
}

const DYN_ORDER: [&str; 6] = ["pp", "p", "mp", "mf", "f", "ff"];

fn dyn_rank(d: &str) -> usize {
    DYN_ORDER.iter().position(|x| *x == d).unwrap_or(3)
}

/// Assign velocity ranges by dynamic labels: per layer and root, the dynamics recorded for
/// that note split 0..1 equally (softest first), so every note covers the whole velocity range
/// even when some dynamics are missing for it. Returns whether any note has several layers.
fn assign_dyn_ranges(zones: &mut [ZoneSpec]) -> bool {
    let mut groups: BTreeMap<(u8, i32), Vec<String>> = BTreeMap::new();
    for z in zones.iter() {
        if let Some(d) = &z.dynamic {
            let g = groups
                .entry((z.layer, (z.root * 16.0).round() as i32))
                .or_default();
            if !g.contains(d) {
                g.push(d.clone());
            }
        }
    }
    let mut multi = false;
    for ((layer, root), mut ds) in groups {
        ds.sort_by_key(|d| dyn_rank(d));
        multi |= ds.len() > 1;
        let k = ds.len() as f32;
        for z in zones
            .iter_mut()
            .filter(|z| z.layer == layer && (z.root * 16.0).round() as i32 == root)
        {
            if let Some(i) = z
                .dynamic
                .as_ref()
                .and_then(|d| ds.iter().position(|x| x == d))
            {
                z.vlo = i as f32 / k;
                z.vhi = (i + 1) as f32 / k;
            }
        }
    }
    multi
}

/// Fill `lo`/`hi` for zones without a key range: split halfway between neighbouring roots of
/// the same layer; the outermost zones extend 3 semitones.
fn assign_key_ranges(zones: &mut [ZoneSpec]) {
    let mut layers: Vec<u8> = zones.iter().map(|z| z.layer).collect();
    layers.sort();
    layers.dedup();
    for layer in layers {
        let mut roots: Vec<f32> = zones
            .iter()
            .filter(|z| z.layer == layer)
            .map(|z| z.root)
            .collect();
        roots.sort_by(f32::total_cmp);
        roots.dedup();
        for z in zones
            .iter_mut()
            .filter(|z| z.layer == layer && z.lo.is_none())
        {
            let i = roots.iter().position(|r| *r == z.root).unwrap_or(0);
            let lo = if i == 0 {
                z.root - 3.0
            } else {
                ((roots[i - 1] + z.root) / 2.0).floor() + 1.0
            };
            let hi = if i + 1 == roots.len() {
                z.root + 3.0
            } else {
                ((z.root + roots[i + 1]) / 2.0).floor()
            };
            z.lo = Some(lo.max(0.0));
            z.hi = Some(hi.min(127.0));
        }
    }
}

fn summarize(id: &str, zones: &[ZoneSpec], source: &'static str) -> Built {
    let lo = zones
        .iter()
        .map(|z| z.lo.unwrap_or(z.root))
        .fold(f32::INFINITY, f32::min);
    let hi = zones
        .iter()
        .map(|z| z.hi.unwrap_or(z.root))
        .fold(f32::NEG_INFINITY, f32::max);
    let dyns = DYN_ORDER
        .iter()
        .filter(|d| zones.iter().any(|z| z.dynamic.as_deref() == Some(**d)))
        .map(|d| d.to_string())
        .collect();
    Built {
        id: id.to_string(),
        zones: zones.len(),
        lo,
        hi,
        dyns,
        source,
        remote: zones.iter().filter(|z| z.url.is_some()).count(),
    }
}

fn write_patch(
    out: &Path,
    id: &str,
    header: &[(&str, String)],
    mut zones: Vec<ZoneSpec>,
    source: &'static str,
    built: &Mutex<Vec<Built>>,
) -> Result<(), Error> {
    if zones.is_empty() {
        return Ok(());
    }
    zones.sort_by(|a, b| {
        a.layer
            .cmp(&b.layer)
            .then(a.root.total_cmp(&b.root))
            .then(a.vlo.total_cmp(&b.vlo))
            .then(a.rr.cmp(&b.rr))
    });
    let m = Manifest {
        header: header
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
        zones: zones.clone(),
    };
    let dir = out.join(id);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("zones.txt"),
        format!(
            "# {id} — generated by `uwu library build` from {source}\n{}",
            m.to_text()
        ),
    )?;
    built.lock().unwrap().push(summarize(id, &zones, source));
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// DecentSampler presets

/// A `<group>` of a `.dspreset` with its `<sample>`s (attributes merged with the group's
/// defaults; the sample's own attributes win).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DsGroup {
    pub attrs: BTreeMap<String, String>,
    pub samples: Vec<BTreeMap<String, String>>,
}

fn parse_attrs(s: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        let ks = i;
        while i < b.len() && b[i] != b'=' && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        let key = &s[ks..i];
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() || b[i] != b'=' {
            continue;
        }
        i += 1;
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let q = b[i];
        if q == b'"' || q == b'\'' {
            let vs = i + 1;
            let ve = s[vs..].find(q as char).map(|k| vs + k).unwrap_or(s.len());
            m.insert(key.to_string(), s[vs..ve].to_string());
            i = ve + 1;
        } else {
            let vs = i;
            while i < b.len() && !b[i].is_ascii_whitespace() {
                i += 1;
            }
            m.insert(key.to_string(), s[vs..i].to_string());
        }
    }
    m
}

fn tag_is(tag: &str, name: &str) -> Option<String> {
    let rest = tag.strip_prefix(name)?;
    (rest.is_empty() || rest.starts_with(char::is_whitespace) || rest.starts_with('/'))
        .then(|| rest.to_string())
}

/// Parse the `<groups>` section of a DecentSampler preset (a tiny tag scanner, no XML crate).
/// Returns the `<groups>` attributes and the groups.
pub fn parse_dspreset(text: &str) -> (BTreeMap<String, String>, Vec<DsGroup>) {
    let start = text.find("<groups").unwrap_or(text.len());
    let end = text[start..]
        .find("</groups>")
        .map(|k| start + k)
        .unwrap_or(text.len());
    let s = &text[start..end];
    let mut root = BTreeMap::new();
    let mut groups: Vec<DsGroup> = vec![];
    let mut cur: Option<DsGroup> = None;
    let mut i = 0;
    while let Some(k) = s[i..].find('<') {
        let a = i + k;
        let Some(e) = s[a..].find('>').map(|x| a + x) else {
            break;
        };
        let tag = &s[a + 1..e];
        i = e + 1;
        if let Some(rest) = tag_is(tag, "groups") {
            root = parse_attrs(&rest);
        } else if let Some(rest) = tag_is(tag, "group") {
            if let Some(g) = cur.take() {
                groups.push(g);
            }
            let g = DsGroup {
                attrs: parse_attrs(&rest),
                samples: vec![],
            };
            if rest.trim_end().ends_with('/') {
                groups.push(g);
            } else {
                cur = Some(g);
            }
        } else if tag.starts_with("/group") {
            if let Some(g) = cur.take() {
                groups.push(g);
            }
        } else if let Some(rest) = tag_is(tag, "sample") {
            let Some(g) = cur.as_mut() else { continue };
            let mut m = g.attrs.clone();
            m.extend(parse_attrs(&rest));
            if m.contains_key("path") {
                g.samples.push(m);
            }
        }
    }
    if let Some(g) = cur.take() {
        groups.push(g);
    }
    (root, groups)
}

/// `"6dB"` → 6, `"0.5"` (linear) → -6.02.
fn volume_db(v: &str) -> f32 {
    let v = v.trim();
    if let Some(d) = v.strip_suffix("dB").or_else(|| v.strip_suffix("db")) {
        return d.trim().parse().unwrap_or(0.0);
    }
    v.parse::<f32>()
        .map(|g| crate::units::gain_to_db(g.max(1e-6)))
        .unwrap_or(0.0)
}

/// What to make of one VPO group: (instrument id, articulation, layer, dynamic override).
type Pick = (&'static str, &'static str, u8, Option<&'static str>);

fn vpo_plan(preset: &str, g: usize) -> Option<Pick> {
    let layer = (g - 1) as u8;
    let simple = |inst: &'static str, art: &'static str| Some((inst, art, layer, None));
    match preset {
        "1st-violin-SEC-accent" => simple("strings/violins-1", "accent"),
        "1st-violin-SEC-pizzicato" => simple("strings/violins-1", "pizzicato"),
        "1st-violin-SEC-staccato" => simple("strings/violins-1", "staccato"),
        "1st-violin-SEC-sustain" => simple("strings/violins-1", "sustain"),
        "1st-violin-SEC-tremolo" => simple("strings/violins-1", "tremolo"),
        "2nd-violin-SEC-pizzicato" => simple("strings/violins-2", "pizzicato"),
        "2nd-violin-SEC-staccato" => simple("strings/violins-2", "staccato"),
        "2nd-violin-SEC-sustain" => simple("strings/violins-2", "sustain"),
        "2nd-violin-SEC-tremolo" => simple("strings/violins-2", "tremolo"),
        "viola-SEC-pizzicato" => simple("strings/violas", "pizzicato"),
        "viola-SEC-staccato" => Some(("strings/violas", "staccato", 0, None)),
        "viola-SEC-sustain" => simple("strings/violas", "sustain"),
        "viola-SEC-tremolo" => simple("strings/violas", "tremolo"),
        "cello-SEC-sustain" => simple("strings/cellos", "sustain"),
        "bass-SEC-pizzicato" => simple("strings/basses", "pizzicato"),
        "bass-SEC-staccato" => Some(("strings/basses", "staccato", 0, None)),
        "bass-SEC-sustain" => simple("strings/basses", "sustain"),
        "bass-SEC-tremolo" => simple("strings/basses", "tremolo"),
        "all-strings-SEC-sustain" => simple("strings/strings-ensemble", "sustain"),
        "all-strings-SOLO-sustain" => match g {
            4 => Some(("strings/viola-solo", "sustain", 0, Some("f"))),
            5 => Some(("strings/viola-solo", "sustain", 0, Some("mf"))),
            6 => Some(("strings/violin-solo", "sustain", 0, None)),
            _ => None,
        },
        "all-brass-SEC-sustain" => match g {
            1 => Some(("brass/tuba", "sustain", 0, None)),
            2 => Some(("brass/horns", "sustain", 0, Some("ff"))),
            3 => Some(("brass/horns", "sustain", 0, Some("mp"))),
            4 => Some(("brass/trombones", "sustain", 0, Some("f"))),
            5 => Some(("brass/trombones", "sustain", 0, Some("p"))),
            6 => Some(("brass/trumpet", "sustain", 0, Some("f"))),
            7 => Some(("brass/trumpet", "sustain", 0, Some("p"))),
            _ => None,
        },
        "all-woodwinds-SEC-sustain" => simple("woodwinds/woodwinds-ensemble", "sustain"),
        "piccolo-SOLO-accent" => simple("woodwinds/piccolo", "accent"),
        "piccolo-SOLO-staccato" => simple("woodwinds/piccolo", "staccato"),
        "piccolo-SOLO-sustain" => simple("woodwinds/piccolo", "sustain"),
        "celesta" => (g == 1).then_some(("percussion/celesta", "normal", 0, None)),
        "glockenspiel" => simple("percussion/glockenspiel", "normal"),
        "xylophone" => Some(("percussion/xylophone", "normal", 0, None)),
        "vibraphone-open" => (g == 1).then_some(("percussion/vibraphone", "normal", 0, None)),
        "tubular-bells" => Some(("percussion/tubular-bells", "normal", 0, None)),
        "timpani-hit" => simple("percussion/timpani", "normal"),
        "harp-sustain" => simple("plucked/harp", "normal"),
        "choir-FEMALE-sustain" => simple("choir/female", "sustain"),
        "choir-MALE-normal-mod-wheel" => simple("choir/male", "sustain"),
        "choir-MIXED-sustain" => simple("choir/mixed", "sustain"),
        _ => None,
    }
}

/// The brass ensemble: one layer per instrument, dynamics within it.
fn brass_ensemble(g: usize) -> (u8, Option<&'static str>) {
    match g {
        1 => (0, None),
        2 => (1, Some("ff")),
        3 => (1, Some("mp")),
        4 => (2, Some("f")),
        5 => (2, Some("p")),
        6 => (3, Some("f")),
        _ => (3, Some("p")),
    }
}

fn num(m: &BTreeMap<String, String>, k: &str) -> Option<f32> {
    m.get(k).and_then(|v| v.trim().parse().ok())
}

fn rr_in(name: &str) -> u32 {
    let low = name.to_ascii_lowercase();
    low.find("rr")
        .map(|k| {
            low[k + 2..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
        })
        .and_then(|d| d.parse().ok())
        .unwrap_or(0)
}

struct PatchAcc {
    header: Vec<(&'static str, String)>,
    zones: Vec<ZoneSpec>,
}

#[allow(clippy::too_many_arguments)]
fn vpo_emit(
    src: &Path,
    out: &Path,
    acc: &mut BTreeMap<String, PatchAcc>,
    root_db: f32,
    full: &str,
    layer: u8,
    dyn_: Option<&str>,
    g: &DsGroup,
) -> Result<(), Error> {
    let dir = out.join(full);
    std::fs::create_dir_all(&dir)?;
    let a = &g.attrs;
    let mut header: Vec<(&'static str, String)> = vec![];
    for k in ["attack", "decay", "release"] {
        if let Some(v) = num(a, k) {
            header.push((k, format!("{v}")));
        }
    }
    if let Some(v) = num(a, "sustain") {
        header.push((
            "sustain",
            format!("{}", if v > 1.0 { v / 100.0 } else { v }),
        ));
    }
    let entry = acc.entry(full.to_string()).or_insert(PatchAcc {
        header,
        zones: vec![],
    });
    for s in &g.samples {
        let path = s["path"].replace('\\', "/");
        let rel = path
            .strip_prefix("Samples/")
            .or_else(|| path.strip_prefix("samples/"))
            .unwrap_or(&path)
            .to_string();
        let file = src.join("samples").join(&rel);
        let fname = Path::new(&rel)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if fname.starts_with("silence") || s.get("trigger").map(|t| t == "release").unwrap_or(false)
        {
            continue;
        }
        if !file.is_file() {
            eprintln!("  ! missing {}", file.display());
            continue;
        }
        let dst_name = clean_name(&fname);
        link_or_copy(&file, &dir.join(&dst_name))?;
        let mut z = ZoneSpec::new(dst_name, num(s, "rootNote").unwrap_or(60.0));
        z.lo = num(s, "loNote");
        z.hi = num(s, "hiNote");
        z.tune = num(s, "tuning").unwrap_or(0.0);
        z.gain_db = root_db + s.get("volume").map(|v| volume_db(v)).unwrap_or(0.0);
        if let (Some(lv), Some(hv)) = (num(s, "loVel"), num(s, "hiVel")) {
            z.vlo = lv / 127.0;
            z.vhi = ((hv + 1.0) / 128.0).min(1.0);
            z.dynamic = Some(if lv > 0.0 { "f" } else { "p" }.into());
        }
        z.rr = num(s, "seqPosition")
            .map(|v| v as u32)
            .unwrap_or_else(|| rr_in(&fname));
        if let Some(d) = dyn_ {
            z.dynamic = Some(d.into());
        }
        z.layer = layer;
        let info = wav_info(&file);
        let sr = info.map(|i| i.0).unwrap_or(44100).max(1) as f32;
        z.sr = sr;
        if let Some(st) = num(s, "start") {
            z.offset = st / sr;
        }
        let lp = match (num(s, "loopStart"), num(s, "loopEnd")) {
            (Some(a), Some(b)) => Some((a as u64, b as u64)),
            _ => info.and_then(|i| i.2),
        };
        if s.get("loopEnabled").map(|v| v != "false").unwrap_or(true)
            && let Some((a, b)) = lp
        {
            z.loop_start = Some(a);
            z.loop_end = Some(b);
        }
        entry.zones.push(z);
    }
    Ok(())
}

fn build_vpo(src: &Path, out: &Path, built: &Mutex<Vec<Built>>) -> Result<(), Error> {
    let mut presets: Vec<PathBuf> = std::fs::read_dir(src)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "dspreset").unwrap_or(false))
        .collect();
    presets.sort();
    let mut acc: BTreeMap<String, PatchAcc> = BTreeMap::new();
    let mut targets: Vec<String> = vec![];
    for p in &presets {
        let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
        for g in 1..=12 {
            if let Some((id, art, _, _)) = vpo_plan(&stem, g) {
                targets.push(format!("{id}/{art}"));
            }
        }
    }
    targets.push("brass/brass-ensemble/sustain".into());
    targets.sort();
    targets.dedup();
    for t in &targets {
        fresh_dir(&out.join(t))?;
    }
    for p in presets {
        let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&p)?;
        let (root, groups) = parse_dspreset(&text);
        let root_db = root.get("volume").map(|v| volume_db(v)).unwrap_or(0.0);
        let mut seen: Vec<Vec<String>> = vec![];
        let norm = |g: &DsGroup| -> Vec<String> {
            g.samples
                .iter()
                .map(|s| {
                    let mut x = s["path"].to_ascii_lowercase();
                    for k in 1..=4 {
                        x = x
                            .replace(&format!("rr{k}"), "rr")
                            .replace(&format!("_{k}-pb"), "_rr-pb");
                    }
                    x
                })
                .collect()
        };
        for (i, g) in groups.iter().enumerate() {
            if g.attrs
                .get("enabled")
                .map(|v| v == "false")
                .unwrap_or(false)
                || g.samples.is_empty()
            {
                continue;
            }
            if g.attrs
                .get("volume")
                .map(|v| volume_db(v) < -100.0)
                .unwrap_or(false)
            {
                continue;
            }
            let set: Vec<String> = g.samples.iter().map(|s| s["path"].clone()).collect();
            if seen.contains(&set) {
                continue; // duplicate group (same files) adds nothing
            }
            seen.push(set);
            if let Some((id, art, mut layer, dyn_)) = vpo_plan(&stem, i + 1) {
                // groups that only differ by round-robin take share a layer
                if let Some(j) = (0..i).find(|&j| norm(&groups[j]) == norm(g)) {
                    layer = layer.min(j as u8);
                }
                vpo_emit(
                    src,
                    out,
                    &mut acc,
                    root_db,
                    &format!("{id}/{art}"),
                    layer,
                    dyn_,
                    g,
                )?;
            }
            if stem == "all-brass-SEC-sustain" {
                let (layer, dyn_) = brass_ensemble(i + 1);
                vpo_emit(
                    src,
                    out,
                    &mut acc,
                    root_db,
                    "brass/brass-ensemble/sustain",
                    layer,
                    dyn_,
                    g,
                )?;
            }
        }
    }
    for (full, mut p) in acc {
        let mut ls: Vec<u8> = p.zones.iter().map(|z| z.layer).collect();
        ls.sort();
        ls.dedup();
        for z in &mut p.zones {
            z.layer = ls.iter().position(|l| *l == z.layer).unwrap_or(0) as u8;
        }
        let multi = assign_dyn_ranges(&mut p.zones);
        let mut header = p.header;
        if multi {
            header.push(("vel_xfade", "0.15".into()));
        }
        if full.contains("ensemble") {
            header.push(("gain_db", "-5".into())); // several sections stack up
        }
        if full.starts_with("percussion/") || full.starts_with("plucked/") {
            header.retain(|(k, _)| *k != "sustain" && *k != "decay");
            header.push(("loop", "0".into())); // struck/plucked: let the samples ring out
            if !header.iter().any(|(k, _)| *k == "release") {
                header.push(("release", "1.5".into()));
            }
        }
        write_patch(
            out,
            &full,
            &header,
            p.zones,
            "Virtual Playing Orchestra",
            built,
        )?;
    }
    // cello section articulations that have samples but no preset
    for (d, art, release) in [
        ("cello-SEC-pizzicato", "pizzicato", 0.3),
        ("cello-SEC-staccato", "staccato", 0.25),
        ("cello-SEC-tremolo", "tremolo", 0.6),
    ] {
        let sd = src.join("samples").join(d);
        if !sd.is_dir() {
            continue;
        }
        let id = format!("strings/cellos/{art}");
        let dir = out.join(&id);
        fresh_dir(&dir)?;
        let mut zones = vec![];
        for f in super::load::wav_files(&sd)? {
            let name = f.file_name().unwrap().to_string_lossy().into_owned();
            let stem = name
                .trim_end_matches(".wav")
                .trim_end_matches("-PB-loop")
                .trim_end_matches("-PB");
            let toks: Vec<&str> = stem.split('_').collect();
            let root = if toks.len() >= 2 && toks[0].len() == 1 {
                note_any(&format!("{}_{}", toks[0], toks[1]))
            } else {
                // VSCO-style names (pizzT_A2, spic_A2) put middle C at C3: one octave up
                toks.iter()
                    .skip(1)
                    .find_map(|t| note_any(t))
                    .map(|m| m + 12.0)
            };
            let Some(root) = root else { continue };
            let dst = clean_name(&name);
            link_or_copy(&f, &dir.join(&dst))?;
            let mut z = ZoneSpec::new(dst, root);
            let low = stem.to_ascii_lowercase();
            if low.contains("_v1") {
                z.dynamic = Some("mp".into());
            } else if low.contains("_v2") {
                z.dynamic = Some("f".into());
            }
            z.rr = rr_in(&low);
            if let Some((_, _, Some((a, b)))) = wav_info(&f) {
                z.loop_start = Some(a);
                z.loop_end = Some(b);
            }
            zones.push(z);
        }
        let multi = assign_dyn_ranges(&mut zones);
        assign_key_ranges(&mut zones);
        let mut header = vec![("release", format!("{release}"))];
        if art != "tremolo" {
            header.push(("decay", "1.5".into()));
            header.push(("sustain", "0".into()));
        }
        if multi {
            header.push(("vel_xfade", "0.15".into()));
        }
        write_patch(out, &id, &header, zones, "Virtual Playing Orchestra", built)?;
    }
    // impulse responses
    let ir = out.join("ir");
    std::fs::create_dir_all(&ir)?;
    if let Ok(rd) = std::fs::read_dir(src.join("IR")) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension()
                .map(|x| x.eq_ignore_ascii_case("wav"))
                .unwrap_or(false)
            {
                link_or_copy(&p, &ir.join(clean_name(&e.file_name().to_string_lossy())))?;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Philharmonia

fn phil_dyn(s: &str) -> Option<&'static str> {
    Some(match s {
        "pianissimo" => "pp",
        "piano" => "p",
        "mezzo-piano" => "mp",
        "mezzo-forte" => "mf",
        "forte" => "f",
        "fortissimo" => "ff",
        _ => return None,
    })
}

fn dur_rank(d: &str) -> i32 {
    match d {
        "025" => 1,
        "05" => 2,
        "1" => 3,
        "15" => 4,
        "long" => 5,
        "very-long" => 6,
        _ => 0,
    }
}

/// Articulation folder for a Philharmonia technique (`None` = skip).
fn phil_artic(t: &str, plucked: bool) -> Option<&'static str> {
    let t = t.strip_prefix("arco-").unwrap_or(t);
    Some(match t {
        "normal" if plucked => "normal",
        "normal" => "sustain",
        "staccato" => "staccato",
        "staccatissimo" => "staccatissimo",
        "spiccato" => "spiccato",
        "pizz-normal" => "pizzicato",
        "tremolo" => "tremolo",
        "legato" => "legato",
        "tenuto" => "tenuto",
        "harmonic" | "natural-harmonic" | "harmonics" => "harmonic",
        "col-legno-battuto" => "col-legno",
        "mute" | "con-sord" => "muted",
        "major-trill" => "trill",
        "minor-trill" => "trill-minor",
        "martele" => "marcato",
        "fluttertonguing" => "flutter",
        _ => return None,
    })
}

#[derive(Clone, Debug)]
struct Job {
    src: PathBuf,
    dst: PathBuf,
    max_secs: f32,
    looped: bool,
}

/// Decode with ffmpeg, trim leading silence (4 ms pre-roll), cap the length, fade the end and
/// write 16-bit mono. Returns (frames, loop points for long sustains).
fn convert(job: &Job) -> Result<(f32, Option<(u64, u64)>), Error> {
    let o = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(&job.src)
        .args(["-f", "f32le", "-ac", "1", "-ar", "44100", "-"])
        .output()
        .map_err(|e| Error::Other(format!("ffmpeg: {e}")))?;
    if !o.status.success() {
        return Err(Error::Other(format!(
            "ffmpeg failed on {}: {}",
            job.src.display(),
            String::from_utf8_lossy(&o.stderr)
        )));
    }
    let x: Vec<f32> = o
        .stdout
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    let peak = x.iter().fold(0f32, |m, v| m.max(v.abs()));
    if peak < 1e-4 {
        return Err(Error::Other(format!("{}: silent", job.src.display())));
    }
    let sr = 44100.0;
    let first = x.iter().position(|v| v.abs() > peak * 0.02).unwrap_or(0);
    let start = first.saturating_sub((0.004 * sr) as usize);
    let last = x
        .iter()
        .rposition(|v| v.abs() > peak * 0.0015)
        .unwrap_or(x.len() - 1);
    let max = (job.max_secs * sr) as usize;
    let end = (last + (0.05 * sr) as usize).min(x.len()).min(start + max);
    let mut y = x[start..end].to_vec();
    let n = y.len();
    let fi = (first - start).clamp(1, n);
    for (i, v) in y.iter_mut().take(fi).enumerate() {
        *v *= i as f32 / fi as f32;
    }
    let capped = start + max < last;
    let fo = (((if capped { 0.25 } else { 0.03 }) * sr) as usize)
        .min(n / 2)
        .max(1);
    for i in 0..fo {
        y[n - 1 - i] *= i as f32 / fo as f32;
    }
    let lp = (job.looped && n as f32 > 2.5 * sr)
        .then_some(((n as f32 * 0.35) as u64, (n as f32 * 0.8) as u64));
    if let Some(d) = job.dst.parent() {
        std::fs::create_dir_all(d)?;
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&job.dst, spec)?;
    let mut rng = crate::dsp::Rng::new(n as u64);
    for v in &y {
        let d = (rng.f32() - rng.f32()) / 32767.0;
        w.write_sample(((v + d).clamp(-1.0, 1.0) * 32767.0).round() as i16)?;
    }
    w.finalize()?;
    Ok((peak, lp))
}

fn phil_inst(dir: &str) -> Option<(&'static str, bool)> {
    Some(match dir {
        "banjo" => ("plucked/banjo", true),
        "bass clarinet" => ("woodwinds/bass-clarinet", false),
        "bassoon" => ("woodwinds/bassoon", false),
        "cello" => ("strings/cello-solo", false),
        "clarinet" => ("woodwinds/clarinet", false),
        "contrabassoon" => ("woodwinds/contrabassoon", false),
        "cor anglais" => ("woodwinds/cor-anglais", false),
        "double bass" => ("strings/bass-solo", false),
        "flute" => ("woodwinds/flute", false),
        "french horn" => ("brass/horn", false),
        "guitar" => ("plucked/guitar", true),
        "mandolin" => ("plucked/mandolin", true),
        "oboe" => ("woodwinds/oboe", false),
        "saxophone" => ("woodwinds/saxophone", false),
        "trombone" => ("brass/trombone", false),
        _ => return None,
    })
}

fn phil_release(id: &str, art: &str) -> f32 {
    match (id.split('/').next().unwrap_or(""), art) {
        (_, "pizzicato") => 0.4,
        ("plucked", _) => 0.3,
        (_, "staccato" | "staccatissimo" | "spiccato") => 0.15,
        ("strings", _) => 0.35,
        ("brass", _) => 0.25,
        _ => 0.2,
    }
}

type ConvResults = BTreeMap<PathBuf, (f32, Option<(u64, u64)>)>;
type PhilPatches = BTreeMap<(String, String), Vec<(ZoneSpec, Job)>>;

fn phil_tonal(audio: &Path, out: &Path, patches: &mut PhilPatches) -> Result<(), Error> {
    let mut dirs: Vec<String> = std::fs::read_dir(audio)?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    dirs.sort();
    for d in &dirs {
        let Some((id, plucked)) = phil_inst(d) else {
            continue;
        };
        let files: Vec<PathBuf> = std::fs::read_dir(audio.join(d))?
            .flatten()
            .map(|e| e.path())
            .collect();
        let parsed: Vec<(PathBuf, Vec<String>)> = files
            .iter()
            .filter_map(|f| {
                let name = f.file_name()?.to_string_lossy().into_owned();
                let stem = name.strip_suffix(".mp3")?;
                let t: Vec<String> = stem.split('_').map(String::from).collect();
                (t.len() >= 5).then(|| (f.clone(), t))
            })
            .collect();
        let has_stacc = parsed
            .iter()
            .any(|(_, t)| t[2] != "phrase" && phil_artic(&t[4], plucked) == Some("staccato"));
        // (artic, note, dyn) -> (score, file)
        let mut best: BTreeMap<(String, i32, String), (i32, PathBuf)> = BTreeMap::new();
        for (f, t) in &parsed {
            let (Some(root), Some(dyn_)) = (note_any(&t[1]), phil_dyn(&t[3])) else {
                continue;
            };
            let rank = dur_rank(&t[2]);
            let Some(art) = phil_artic(&t[4], plucked) else {
                continue;
            };
            // phrases are repeated notes: their first note works for short articulations and
            // the opening of a tremolo/trill phrase is a sustained note
            let phrase_ok = matches!(
                art,
                "staccato"
                    | "staccatissimo"
                    | "spiccato"
                    | "pizzicato"
                    | "col-legno"
                    | "tremolo"
                    | "trill"
                    | "trill-minor"
            );
            if rank == 0 && !(t[2] == "phrase" && phrase_ok) {
                continue;
            }
            let mut consider = |art: &str, score: i32| {
                let key = (art.to_string(), root as i32, dyn_.to_string());
                if best.get(&key).map(|b| score > b.0).unwrap_or(true) {
                    best.insert(key, (score, f.clone()));
                }
            };
            let short = matches!(
                art,
                "staccato" | "staccatissimo" | "spiccato" | "pizzicato" | "col-legno"
            );
            consider(art, if short { -rank } else { rank });
            if art == "sustain" && !has_stacc && t[2] == "025" {
                consider("staccato", -rank);
            }
        }
        for ((art, root, dyn_), (_, f)) in best {
            let phrase = f.to_string_lossy().contains("_phrase_");
            let fname = format!("{}_{}.wav", clean_name(&note_name(root as f32)), dyn_);
            let short = matches!(
                art.as_str(),
                "staccato" | "staccatissimo" | "spiccato" | "col-legno"
            );
            let looped = !plucked
                && matches!(
                    art.as_str(),
                    "sustain" | "tremolo" | "trill" | "trill-minor" | "muted"
                );
            let dst = out.join(id).join(&art).join(&fname);
            let mut z = ZoneSpec::new(fname, root as f32);
            z.dynamic = Some(dyn_);
            let max_secs = match (short || art == "pizzicato", phrase) {
                (true, true) => 0.4,
                (true, false) => 1.5,
                (false, true) => 3.0,
                _ if plucked => 8.0,
                _ => 6.0,
            };
            patches
                .entry((id.to_string(), art.clone()))
                .or_default()
                .push((
                    z,
                    Job {
                        src: f,
                        dst,
                        max_secs,
                        looped,
                    },
                ));
        }
    }
    Ok(())
}

fn phil_percussion(perc: &Path, out: &Path, patches: &mut PhilPatches) -> Result<(), Error> {
    let mut pdirs: Vec<String> = std::fs::read_dir(perc)
        .map(|r| {
            r.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    pdirs.sort();
    for pd in pdirs {
        let id = format!("percussion/{}", clean_name(&pd.to_ascii_lowercase()));
        let mut by_tech: BTreeMap<String, Vec<(String, i32, PathBuf)>> = BTreeMap::new();
        for e in std::fs::read_dir(perc.join(&pd))?.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".mp3") else {
                continue;
            };
            let t: Vec<&str> = stem.split('_').collect();
            if t.len() < 5 || t[2] == "phrase" {
                continue;
            }
            let dyn_ = phil_dyn(t[3]).unwrap_or("mf").to_string();
            by_tech
                .entry(t[4].to_string())
                .or_default()
                .push((dyn_, dur_rank(t[2]), e.path()));
        }
        if by_tech.is_empty() {
            continue;
        }
        let pref = [
            "struck-singly",
            "with-snares",
            "bass-drum-mallet",
            "hand",
            "undamped",
            "hit",
            "struck",
        ];
        let main = pref
            .iter()
            .find(|p| by_tech.contains_key(**p))
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                by_tech
                    .iter()
                    .max_by_key(|(_, v)| v.len())
                    .map(|(k, _)| k.clone())
                    .unwrap_or_default()
            });
        for (tech, mut files) in by_tech {
            let art = if tech == main {
                "normal".to_string()
            } else {
                clean_name(&tech)
            };
            files.sort();
            let mut rr: BTreeMap<String, u32> = BTreeMap::new();
            for (dyn_, rank, f) in files {
                let k = rr.entry(dyn_.clone()).or_insert(0);
                *k += 1;
                let fname = format!("{}_{}_rr{}.wav", art, dyn_, k);
                let dst = out.join(&id).join(&art).join(&fname);
                let mut z = ZoneSpec::new(fname, 60.0);
                z.lo = Some(0.0);
                z.hi = Some(127.0);
                z.dynamic = Some(dyn_);
                z.rr = *k;
                let max_secs = if rank >= 5 { 10.0 } else { 6.0 };
                patches.entry((id.clone(), art.clone())).or_default().push((
                    z,
                    Job {
                        src: f,
                        dst,
                        max_secs,
                        looped: false,
                    },
                ));
            }
        }
    }
    Ok(())
}

fn build_phil(src: &Path, out: &Path, jobs: usize, built: &Mutex<Vec<Built>>) -> Result<(), Error> {
    let audio = src.join("audio");
    let mut patches: PhilPatches = BTreeMap::new();
    phil_tonal(&audio, out, &mut patches)?;
    phil_percussion(&audio.join("percussion"), out, &mut patches)?;
    for (id, art) in patches.keys() {
        fresh_dir(&out.join(id).join(art))?;
    }
    let all: Vec<&Job> = patches.values().flatten().map(|(_, j)| j).collect();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let results: Mutex<ConvResults> = Mutex::new(BTreeMap::new());
    let total = all.len();
    std::thread::scope(|s| {
        for _ in 0..jobs.max(1) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(job) = all.get(i) else { break };
                    match convert(job) {
                        Ok(r) => {
                            results.lock().unwrap().insert(job.dst.clone(), r);
                        }
                        Err(e) => eprintln!("\n  ! {e}"),
                    }
                    let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if d.is_multiple_of(25) || d == total {
                        eprint!("\r  philharmonia: {d}/{total} files\x1b[K");
                    }
                }
            });
        }
    });
    eprintln!();
    let results = results.into_inner().unwrap();
    for ((id, art), items) in patches {
        let mut zones = vec![];
        let mut peaks: BTreeMap<String, f32> = BTreeMap::new();
        for (z, job) in &items {
            if let Some((pk, _)) = results.get(&job.dst) {
                let e = peaks
                    .entry(z.dynamic.clone().unwrap_or_default())
                    .or_insert(0.0);
                *e = e.max(*pk);
            }
        }
        for (mut z, job) in items {
            let Some((_, lp)) = results.get(&job.dst) else {
                continue;
            };
            // level each dynamic layer: loudest sample of the layer peaks at a target that
            // keeps a musical (but not extreme) pp..ff spread
            let target = match z.dynamic.as_deref() {
                Some("pp") => -16.0,
                Some("p") => -12.0,
                Some("mp") => -9.0,
                Some("mf") => -6.5,
                Some("f") => -4.0,
                _ => -2.5,
            };
            let lp_peak = peaks
                .get(&z.dynamic.clone().unwrap_or_default())
                .copied()
                .unwrap_or(1.0);
            z.gain_db = ((target - crate::units::gain_to_db(lp_peak)) * 10.0).round() / 10.0;
            if let Some((a, b)) = lp {
                z.loop_start = Some(*a);
                z.loop_end = Some(*b);
                z.xfade = Some(0.25);
            }
            zones.push(z);
        }
        let perc = id.starts_with("percussion/");
        let multi = assign_dyn_ranges(&mut zones);
        if !perc {
            assign_key_ranges(&mut zones);
        }
        let release = if perc { 0.5 } else { phil_release(&id, &art) };
        let mut header = vec![
            ("attack", "0.002".to_string()),
            ("release", format!("{release}")),
        ];
        if multi {
            header.push(("vel_xfade", "0.12".into()));
        }
        if perc {
            header.push(("oneshot", "1".into()));
        }
        write_patch(
            out,
            &format!("{id}/{art}"),
            &header,
            zones,
            "Philharmonia Orchestra",
            built,
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// piano, wavebase

fn build_piano(src: &Path, out: &Path, built: &Mutex<Vec<Built>>) -> Result<(), Error> {
    let id = "keys/piano/sustain";
    let dir = out.join(id);
    fresh_dir(&dir)?;
    let mut zones = vec![];
    let mut count: BTreeMap<u8, u32> = BTreeMap::new();
    for f in super::load::wav_files(src)? {
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        let b = name.as_bytes();
        if b.len() > 2 && b[0] == b[1] && b[0].is_ascii_lowercase() && b[2] == b'1' {
            continue; // cc1/dd1/ee1(+s/f) are byte-identical duplicates of c1/d1/e1
        }
        let Some(k) = super::load::piano_key(&name) else {
            continue;
        };
        let n = count.entry(k).or_insert(0);
        *n += 1;
        let dst = format!(
            "{}{}.wav",
            clean_name(&note_name(k as f32)),
            if *n > 1 {
                format!("_rr{n}")
            } else {
                String::new()
            }
        );
        link_or_copy(&f, &dir.join(&dst))?;
        let mut z = ZoneSpec::new(dst, k as f32);
        z.lo = Some(k as f32);
        z.hi = Some(k as f32);
        z.rr = *n;
        zones.push(z);
    }
    write_patch(
        out,
        id,
        &[("release", "0.35".into())],
        zones,
        "piano (assets/piano)",
        built,
    )
}

/// One wavebase file: a local copy if it's real or already cached, plus the remote reference
/// for LFS pointers (so a missing file is fetched on first use).
fn wb_zone(f: &Path, dir: &Path, dst: &str, root: f32) -> Result<ZoneSpec, Error> {
    let mut z = ZoneSpec::new(dst.to_string(), root);
    match LfsPointer::read(f) {
        None => link_or_copy(f, &dir.join(dst))?,
        Some(ptr) => {
            let cached = lfs::cached_path(&ptr);
            if std::fs::metadata(&cached)
                .map(|m| m.len() == ptr.size)
                .unwrap_or(false)
            {
                link_or_copy(&cached, &dir.join(dst))?;
            }
            z.url = Some(lfs::url_for(f)?);
            z.size = Some(ptr.size);
            z.sha256 = Some(ptr.oid);
        }
    }
    Ok(z)
}

fn take_no(name: &str) -> u32 {
    name.trim_end_matches(".wav")
        .rsplit(['-', '_'])
        .next()
        .and_then(|t| t.parse().ok())
        .unwrap_or(0)
}

fn build_wavebase(src: &Path, out: &Path, built: &Mutex<Vec<Built>>) -> Result<(), Error> {
    let base = src.join("base");
    let pitched: [(&str, &str, &str, f32); 5] = [
        ("bass", "plucked/bass-guitar/normal", "0.07", 0.0),
        (
            "guitar/stratocaster",
            "plucked/electric-guitar/normal",
            "0.12",
            0.0,
        ),
        (
            "guitar/parker-fly",
            "plucked/electric-guitar-fly/normal",
            "0.12",
            0.0,
        ),
        ("harp/celtic", "plucked/celtic-harp/normal", "1.2", -0.14),
        ("harp/zither", "plucked/zither/normal", "1.0", 0.0),
    ];
    for (sub, id, release, tune) in pitched {
        let d = base.join(sub);
        if !d.is_dir() {
            continue;
        }
        let od = out.join(id);
        fresh_dir(&od)?;
        let mut files = super::load::wav_files(&d)?;
        for s in 1..=6 {
            let sd = d.join(format!("string-{s}"));
            if sd.is_dir() {
                files.extend(super::load::wav_files(&sd)?);
            }
        }
        let mut zones = vec![];
        for f in files {
            let name = f.file_name().unwrap().to_string_lossy().into_owned();
            let Some(root) = super::load::note_in_name(&name) else {
                continue;
            };
            let mut z = wb_zone(&f, &od, &clean_name(&name), root)?;
            z.tune = if sub == "bass" {
                // measured: the low strings of this recording are slightly out of tune
                match name.get(..8) {
                    Some("string-4") => -0.15,
                    Some("string-3") => 0.12,
                    Some("string-2") => 0.03,
                    _ => 0.0,
                }
            } else {
                tune
            };
            if name.contains("take-") || sub.contains("parker") {
                z.rr = take_no(&name);
            }
            zones.push(z);
        }
        write_patch(
            out,
            id,
            &[("release", release.into())],
            zones,
            "cluesurf/wave",
            built,
        )?;
    }
    // the acoustic kit: GM-mapped top-level drum files (toms/crash 2 derived by retuning)
    let kit: [(u8, &str, f32); 19] = [
        (36, "kick.wav", 0.0),
        (35, "kick-2.wav", 0.0),
        (37, "stick.wav", 0.0),
        (38, "snare.wav", 0.0),
        (40, "snare-2.wav", 0.0),
        (39, "snap.wav", 0.0),
        (42, "hat-closed.wav", 0.0),
        (44, "hat-closed-4.wav", 0.0),
        (46, "hat-opened.wav", 0.0),
        (41, "tom.wav", 0.0),
        (43, "tom.wav", 2.0),
        (45, "tom-2.wav", 0.0),
        (47, "tom-2.wav", 2.0),
        (48, "tom-2.wav", 4.0),
        (50, "tom-2.wav", 7.0),
        (49, "crash.wav", 0.0),
        (57, "crash.wav", 1.5),
        (51, "ride.wav", 0.0),
        (54, "tap.wav", 0.0),
    ];
    let d = base.join("drum");
    if d.is_dir() {
        let id = "drums/acoustic-kit/normal";
        let od = out.join(id);
        fresh_dir(&od)?;
        let mut zones = vec![];
        for (key, file, tune) in kit {
            let f = d.join(file);
            if !f.exists() {
                continue;
            }
            let mut z = wb_zone(&f, &od, file, key as f32)?;
            z.lo = Some(key as f32);
            z.hi = Some(key as f32);
            z.tune = tune;
            zones.push(z);
        }
        write_patch(
            out,
            id,
            &[("oneshot", "1".into()), ("release", "0.05".into())],
            zones,
            "cluesurf/wave",
            built,
        )?;
    }
    // every other folder of one-shots, mapped chromatically from C2 (36)
    for (top, family) in [("drum", "drums"), ("nature", "sfx")] {
        let root_dir = base.join(top);
        if !root_dir.is_dir() {
            continue;
        }
        let mut stack = vec![root_dir.clone()];
        while let Some(dir) = stack.pop() {
            let mut subs: Vec<PathBuf> = std::fs::read_dir(&dir)?
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect();
            subs.sort();
            stack.extend(subs);
            if dir == root_dir && top == "drum" {
                continue; // top-level drum files = the acoustic kit
            }
            let files = super::load::wav_files(&dir)?;
            if files.is_empty() {
                continue;
            }
            let rel = dir
                .strip_prefix(&base)
                .unwrap()
                .to_string_lossy()
                .replace('/', "-");
            let rel = if top == "drum" {
                rel.trim_start_matches("drum-").to_string()
            } else {
                rel
            };
            let id = format!("{family}/{}/normal", clean_name(&rel));
            let od = out.join(&id);
            fresh_dir(&od)?;
            let start = if files.len() <= 92 { 36 } else { 0 };
            let mut zones = vec![];
            for (i, f) in files.iter().take(128).enumerate() {
                let key = (start + i) as f32;
                let name = f.file_name().unwrap().to_string_lossy().into_owned();
                let mut z = wb_zone(f, &od, &clean_name(&name), key)?;
                z.lo = Some(key);
                z.hi = Some(key);
                zones.push(z);
            }
            write_patch(
                out,
                &id,
                &[("oneshot", "1".into()), ("release", "0.05".into())],
                zones,
                "cluesurf/wave",
                built,
            )?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// docs

fn write_index(out: &Path, built: &[Built]) -> Result<(), Error> {
    let mut b: Vec<&Built> = built.iter().collect();
    b.sort_by(|x, y| x.id.cmp(&y.id));
    let mut o = String::from(
        "# uwu-wave sample library — index\n\nGenerated by `uwu library build`. Play any entry with `Sampler::library(\"family/instrument/articulation\")` (or just `family/instrument` for its default articulation). Dynamics are velocity layers (softest first).\n",
    );
    let mut fam = String::new();
    let (mut tz, mut tr) = (0, 0);
    for x in &b {
        let f = x.id.split('/').next().unwrap_or("");
        if f != fam {
            fam = f.to_string();
            o += &format!(
                "\n## {fam}\n\n| id | zones | range | dynamics | source |\n|---|---|---|---|---|\n"
            );
        }
        tz += x.zones;
        tr += x.remote;
        let remote = if x.remote > 0 {
            format!(" ({} remote)", x.remote)
        } else {
            String::new()
        };
        let dyns = if x.dyns.is_empty() {
            "–".to_string()
        } else {
            x.dyns.join(" ")
        };
        o += &format!(
            "| `{}` | {}{} | {}–{} | {} | {} |\n",
            x.id,
            x.zones,
            remote,
            note_name(x.lo),
            note_name(x.hi),
            dyns,
            x.source
        );
    }
    let irs = super::library::Library::at(out).irs();
    let irs: Vec<String> = irs.iter().map(|i| format!("`ir/{i}.wav`")).collect();
    o += &format!(
        "\n## ir\n\n{}\n\n---\n{} patches, {} zones ({} remote, fetched on first use).\n",
        irs.join(", "),
        b.len(),
        tz,
        tr
    );
    std::fs::write(out.join("INDEX.md"), o)?;
    Ok(())
}

/// Rebuild INDEX.md from the manifests on disk. Returns the number of patches.
pub fn reindex(out: &Path) -> Result<usize, Error> {
    let lib = super::library::Library::at(out);
    let mut built = vec![];
    for id in lib.instruments() {
        for a in lib.articulations(&id) {
            let full = format!("{id}/{a}");
            let text = std::fs::read_to_string(out.join(&full).join("zones.txt"))?;
            let first = text.lines().next().unwrap_or("");
            let source: &'static str = if first.contains("Virtual Playing") {
                "Virtual Playing Orchestra"
            } else if first.contains("Philharmonia") {
                "Philharmonia Orchestra"
            } else if first.contains("cluesurf") {
                "cluesurf/wave"
            } else {
                "piano (assets/piano)"
            };
            let m = Manifest::parse(&text)?;
            built.push(summarize(&full, &m.zones, source));
        }
    }
    write_index(out, &built)?;
    Ok(built.len())
}

const LICENSES: &str = r#"# Sample library — sources and licenses

The audio in this folder comes from several free sources; the first line of every `zones.txt`
names its source. Keep this file with the library.

## Virtual Playing Orchestra (DecentSampler edition)
<https://github.com/eodowd/VirtualPlayingOrchestra>, compiled by Paul Battersby
(<http://virtualplaying.com/virtual-playing-orchestra/>). Used for: the string sections
(`violins-1`, `violins-2`, `violas`, `cellos`, `basses`, `strings-ensemble`), `violin-solo`,
`viola-solo`, `brass/{horns,trombones,trumpet,tuba,brass-ensemble}`, `woodwinds/piccolo`,
`woodwinds/woodwinds-ensemble`, `choir/*`, `plucked/harp`, `percussion/{celesta,glockenspielThe 88-key piano recordings from the original `assets/piano` folder. Check the original source
before redistributing the raw samples.

## cluesurf/wave (`plucked/{bass-guitar,electric-guitar,electric-guitar-fly,celtic-harp,zither}`, `drums/*`, `sfx/*`)
<https://github.com/cluesurf/wave> — "100% public domain, free sound samples". These zones are
*remote*: `zones.txt` has `url=`, `size=` and `sha256=`, and uwu-wave downloads each file the
first time it's used into `~/.cache/uwu-wave/lfs` (`UWU_OFFLINE=1` disables downloading).
"#;

/// Build (or rebuild parts of) the library. Prints progress to stderr.
pub fn build(opts: &BuildOptions) -> Result<Vec<Built>, Error> {
    let want = |p: &str| opts.only.is_empty() || opts.only.iter().any(|x| x == p);
    std::fs::create_dir_all(&opts.out)?;
    let built = Mutex::new(vec![]);
    let s = &opts.sources;
    let t0 = std::time::Instant::now();
    let vpo = s.join("VirtualPlayingOrchestra-main");
    if want("vpo") && vpo.is_dir() {
        eprintln!("▸ Virtual Playing Orchestra");
        build_vpo(&vpo, &opts.out, &built)?;
    }
    let phil = s.join("philharmonia-samples-gh-pages");
    if want("phil") && phil.is_dir() {
        eprintln!("▸ Philharmonia (ffmpeg × {})", opts.jobs);
        build_phil(&phil, &opts.out, opts.jobs, &built)?;
    }
    if want("piano") && s.join("piano").is_dir() {
        eprintln!("▸ piano");
        build_piano(&s.join("piano"), &opts.out, &built)?;
    }
    if want("wavebase") && s.join("wavebase").is_dir() {
        eprintln!("▸ wavebase (remote zones)");
        build_wavebase(&s.join("wavebase"), &opts.out, &built)?;
    }
    std::fs::write(opts.out.join("LICENSES.md"), LICENSES)?;
    let n = reindex(&opts.out)?;
    eprintln!(
        "✓ library: {n} patches in {} ({:.0}s)",
        opts.out.display(),
        t0.elapsed().as_secs_f32()
    );
    Ok(built.into_inner().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"<?xml version="1.0"?>
<DecentSampler minVersion="1.0.0">
  <ui><labeled-knob label="Attack"/></ui>
  <groups volume="-3dB">
    <group attack="0.18" release="1.9" tuning="0.04">
      <sample path="Samples/x/1st-violins-stc-rr1-g3-PB.wav" loNote="55" hiNote="56" rootNote="55" tuning="-0.1" volume="6dB" seqPosition="1"/>
      <sample path="Samples/x/a b.wav" rootNote="57" loVel="41" hiVel="127" />
    </group>
    <group path="Samples/x/4_Gb-PB.wav" loNote="48" hiNote="54" rootNote="54">
      <sample/>
      <sample path="Samples/x/4_Gb_2-PB.wav" volume="1.5dB"/>
    </group>
    <group enabled="false"><sample path='single.wav' rootNote='60'/></group>
  </groups>
</DecentSampler>"#;

    #[test]
    fn dspreset_parser() {
        let (root, g) = parse_dspreset(FIXTURE);
        assert_eq!(root.get("volume").map(String::as_str), Some("-3dB"));
        assert_eq!(g.len(), 3);
        assert_eq!(g[0].attrs.get("attack").map(String::as_str), Some("0.18"));
        let s = &g[0].samples[0];
        assert_eq!(s["path"], "Samples/x/1st-violins-stc-rr1-g3-PB.wav");
        assert_eq!(s["tuning"], "-0.1"); // the sample overrides the group
        assert_eq!(s["seqPosition"], "1");
        assert_eq!(g[0].samples[1]["path"], "Samples/x/a b.wav");
        assert_eq!(g[0].samples[1]["hiVel"], "127");
        assert_eq!(g[0].samples[1]["tuning"], "0.04"); // inherited
        // an empty <sample/> inherits the group's path; others inherit its key range
        assert_eq!(g[1].samples.len(), 2);
        assert_eq!(g[1].samples[0]["path"], "Samples/x/4_Gb-PB.wav");
        assert_eq!(g[1].samples[1]["loNote"], "48");
        assert_eq!(g[2].attrs["enabled"], "false");
        assert_eq!(g[2].samples[0]["path"], "single.wav");
        assert!((volume_db("6dB") - 6.0).abs() < 1e-6);
        assert!((volume_db("0.5") + 6.02).abs() < 0.01);
    }

    #[test]
    fn note_spellings() {
        assert_eq!(note_any("a#3"), Some(58.0));
        assert_eq!(note_any("3_Gb"), Some(54.0));
        assert_eq!(note_any("2_Db"), Some(37.0));
        assert_eq!(note_any("c4"), Some(60.0));
        assert_eq!(note_any("Ax2"), Some(46.0));
        assert_eq!(note_any("F#6"), Some(90.0));
        assert_eq!(note_name(61.0), "C#4");
        assert_eq!(
            clean_name("1st-violins-sus-a#3.wav"),
            "1st-violins-sus-as3.wav"
        );
        assert_eq!(rr_in("Violas_spic_A3_v2_rr2-PB.wav"), 2);
        assert_eq!(take_no("A2-take-7.wav"), 7);
    }

    #[test]
    fn ranges_and_dynamics() {
        let mut z = vec![
            ZoneSpec::new("a", 48.0),
            ZoneSpec::new("b", 52.0),
            ZoneSpec::new("c", 60.0),
            ZoneSpec::new("d", 48.0),
        ];
        for (i, d) in ["p", "p", "p", "ff"].iter().enumerate() {
            z[i].dynamic = Some(d.to_string());
        }
        assert!(assign_dyn_ranges(&mut z));
        assert_eq!(
            (z[0].vlo, z[0].vhi, z[3].vlo, z[3].vhi),
            (0.0, 0.5, 0.5, 1.0)
        );
        assert_eq!(
            (z[1].vlo, z[1].vhi),
            (0.0, 1.0),
            "a note with one dynamic covers all velocities"
        );
        assign_key_ranges(&mut z);
        assert_eq!((z[0].lo, z[0].hi), (Some(45.0), Some(50.0)));
        assert_eq!((z[1].lo, z[1].hi), (Some(51.0), Some(56.0)));
        assert_eq!((z[2].lo, z[2].hi), (Some(57.0), Some(63.0)));
        assert_eq!((z[3].lo, z[3].hi), (Some(45.0), Some(50.0)));
    }
}
