//! The structured sample library: `assets/library/<family>/<instrument>/<articulation>/`.
//!
//! Each articulation folder holds audio files plus a `zones.txt` manifest:
//!
//! ```text
//! # header: instrument-level defaults
//! @attack=0.01 @release=0.8 @gain_db=0 @tune=0 @loop=1
//! # one zone per line (only file + root are required)
//! file=G3_f_rr1.wav root=55 lo=54 hi=56 vlo=0.0 vhi=1.0 tune=-0.05 gain_db=-2 rr=1 dyn=f loop_start=12345 loop_end=67890
//! # remote (lazily downloaded) audio
//! file=kick.wav root=36 url=https://… size=97406 sha256=0d8e…
//! ```
//!
//! Zone tokens: `file root lo hi vlo vhi tune gain_db pan rr dyn layer loop_start loop_end sr
//! offset end xfade url size sha256` (loop points in frames at `sr`, default 44100; `offset`/`end`
//! in seconds). Header tokens: `attack decay sustain release gain_db tune loop oneshot vel_xfade
//! voices trim`. Zones on different `layer`s sound together.
//!
//! ```no_run
//! use uwu_wave::prelude::*;
//! let lib = Library::open().unwrap();
//! for id in lib.instruments() {
//!     println!("{id}: {:?}", lib.articulations(&id));
//! }
//! let violins = Sampler::library("strings/violins-1/staccato");
//! let patch = Library::patch("brass/horn/sustain").unwrap();
//! println!("{} zones, range {:?}", patch.zones.len(), patch.range());
//! let horn = patch.sampler().release(0.6);
//! ```

use super::load::assets_dir;
use super::sampler::{Remote, Sampler, Src, Zone};
use super::voice::Region;
use crate::Error;
use std::path::{Path, PathBuf};

/// Articulation folders tried (in order) when an id names only the instrument.
pub const DEFAULT_ARTICS: [&str; 6] = [
    "sustain",
    "normal",
    "legato",
    "staccato",
    "pizzicato",
    "tremolo",
];

/// The library folder: `$UWU_LIBRARY`, else `<assets>/library`.
pub fn library_dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("UWU_LIBRARY").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let d = assets_dir()?.join("library");
    d.is_dir().then_some(d)
}

/// A handle on the sample library.
#[derive(Clone, Debug)]
pub struct Library {
    root: PathBuf,
}

/// One zone line of a manifest, before it becomes a [`Zone`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ZoneSpec {
    pub file: String,
    pub root: f32,
    pub lo: Option<f32>,
    pub hi: Option<f32>,
    pub vlo: f32,
    pub vhi: f32,
    pub tune: f32,
    pub gain_db: f32,
    pub pan: f32,
    pub rr: u32,
    pub dynamic: Option<String>,
    pub layer: u8,
    pub loop_start: Option<u64>,
    pub loop_end: Option<u64>,
    pub sr: f32,
    pub offset: f32,
    pub end: Option<f32>,
    pub xfade: Option<f32>,
    pub url: Option<String>,
    pub size: Option<u64>,
    pub sha256: Option<String>,
}

/// A parsed `zones.txt`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Manifest {
    /// Header values (`@key=value`).
    pub header: Vec<(String, String)>,
    pub zones: Vec<ZoneSpec>,
}

impl Manifest {
    /// Parse manifest text.
    pub fn parse(text: &str) -> Result<Manifest, Error> {
        let mut m = Manifest::default();
        for (ln, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with('@') {
                for tok in line.split_whitespace() {
                    let tok = tok.trim_start_matches('@');
                    let (k, v) = tok.split_once('=').unwrap_or((tok, "1"));
                    m.header.push((k.to_string(), v.to_string()));
                }
                continue;
            }
            let bad = |what: &str| Error::Invalid(format!("zones.txt line {}: {what}", ln + 1));
            let mut z = ZoneSpec {
                vhi: 1.0,
                sr: 44100.0,
                ..ZoneSpec::default()
            };
            let mut have_root = false;
            for tok in line.split_whitespace() {
                let Some((k, v)) = tok.split_once('=') else {
                    return Err(bad(&format!("expected key=value, got {tok:?}")));
                };
                let num = || {
                    v.parse::<f32>()
                        .map_err(|_| bad(&format!("{k}: bad number {v:?}")))
                };
                let int = || {
                    v.parse::<u64>()
                        .map_err(|_| bad(&format!("{k}: bad integer {v:?}")))
                };
                match k {
                    "file" => z.file = v.to_string(),
                    "root" => {
                        z.root = super::load::parse_note(v)
                            .or_else(|| v.parse().ok())
                            .ok_or_else(|| bad(&format!("bad root {v:?}")))?;
                        have_root = true;
                    }
                    "lo" => z.lo = Some(num()?),
                    "hi" => z.hi = Some(num()?),
                    "vlo" => z.vlo = num()?,
                    "vhi" => z.vhi = num()?,
                    "tune" => z.tune = num()?,
                    "gain_db" => z.gain_db = num()?,
                    "pan" => z.pan = num()?,
                    "rr" => z.rr = int()? as u32,
                    "dyn" => z.dynamic = Some(v.to_string()),
                    "layer" => z.layer = int()? as u8,
                    "loop_start" => z.loop_start = Some(int()?),
                    "loop_end" => z.loop_end = Some(int()?),
                    "sr" => z.sr = num()?,
                    "offset" => z.offset = num()?,
                    "end" => z.end = Some(num()?),
                    "xfade" => z.xfade = Some(num()?),
                    "url" => z.url = Some(v.to_string()),
                    "size" => z.size = Some(int()?),
                    "sha256" => z.sha256 = Some(v.to_ascii_lowercase()),
                    _ => {} // unknown tokens are ignored (forward compatible)
                }
            }
            if z.file.is_empty() || !have_root {
                return Err(bad("zones need file= and root="));
            }
            m.zones.push(z);
        }
        Ok(m)
    }

    /// A header value.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.header
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
    fn num(&self, key: &str) -> Option<f32> {
        self.get(key).and_then(|v| v.parse().ok())
    }

    /// Serialize (the builder writes manifests with this).
    pub fn to_text(&self) -> String {
        let mut o = String::new();
        if !self.header.is_empty() {
            let h: Vec<String> = self
                .header
                .iter()
                .map(|(k, v)| format!("@{k}={v}"))
                .collect();
            o += &h.join(" ");
            o.push('\n');
        }
        for z in &self.zones {
            o += &z.to_line();
            o.push('\n');
        }
        o
    }
}

fn fmt_f(x: f32) -> String {
    let s = format!("{:.4}", x);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

impl ZoneSpec {
    /// A zone for `file` at `root`.
    pub fn new(file: impl Into<String>, root: f32) -> ZoneSpec {
        ZoneSpec {
            file: file.into(),
            root,
            vhi: 1.0,
            sr: 44100.0,
            ..ZoneSpec::default()
        }
    }
    /// One manifest line.
    pub fn to_line(&self) -> String {
        let mut t = vec![
            format!("file={}", self.file),
            format!("root={}", fmt_f(self.root)),
        ];
        if let Some(x) = self.lo {
            t.push(format!("lo={}", fmt_f(x)));
        }
        if let Some(x) = self.hi {
            t.push(format!("hi={}", fmt_f(x)));
        }
        if self.vlo != 0.0 || self.vhi != 1.0 {
            t.push(format!("vlo={} vhi={}", fmt_f(self.vlo), fmt_f(self.vhi)));
        }
        for (k, v) in [
            ("tune", self.tune),
            ("gain_db", self.gain_db),
            ("pan", self.pan),
        ] {
            if v != 0.0 {
                t.push(format!("{k}={}", fmt_f(v)));
            }
        }
        if self.rr > 0 {
            t.push(format!("rr={}", self.rr));
        }
        if let Some(d) = &self.dynamic {
            t.push(format!("dyn={d}"));
        }
        if self.layer > 0 {
            t.push(format!("layer={}", self.layer));
        }
        if let (Some(a), Some(b)) = (self.loop_start, self.loop_end) {
            t.push(format!("loop_start={a} loop_end={b}"));
            if self.sr != 44100.0 {
                t.push(format!("sr={}", fmt_f(self.sr)));
            }
        }
        if self.offset != 0.0 {
            t.push(format!("offset={}", fmt_f(self.offset)));
        }
        if let Some(e) = self.end {
            t.push(format!("end={}", fmt_f(e)));
        }
        if let Some(x) = self.xfade {
            t.push(format!("xfade={}", fmt_f(x)));
        }
        if let Some(u) = &self.url {
            t.push(format!("url={u}"));
        }
        if let Some(s) = self.size {
            t.push(format!("size={s}"));
        }
        if let Some(h) = &self.sha256 {
            t.push(format!("sha256={h}"));
        }
        t.join(" ")
    }

    /// Build the playable zone (`dir` = the articulation folder).
    pub fn to_zone(&self, dir: &Path, loops: bool) -> Zone {
        let path = dir.join(&self.file);
        let src = match (&self.url, self.size, &self.sha256) {
            (Some(url), Some(size), Some(sha)) => Src::remote(
                path,
                Remote {
                    url: url.clone(),
                    size,
                    sha256: sha.clone(),
                },
            ),
            _ => Src::path(path),
        };
        let mut z = Zone::with_src(src, self.root);
        if self.lo.is_some() || self.hi.is_some() {
            let lo = self.lo.unwrap_or(self.root);
            let hi = self.hi.unwrap_or(self.root);
            z = z.keys(lo, hi);
        }
        z.vel = (self.vlo.min(self.vhi), self.vlo.max(self.vhi));
        z.tune = self.tune;
        z.gain = crate::units::db_to_gain(self.gain_db);
        z.pan = self.pan;
        z.layer = self.layer;
        z.dynamic = self.dynamic.clone();
        let mut r = Region {
            start: self.offset,
            end: self.end,
            ..Region::default()
        };
        if loops && let (Some(a), Some(b)) = (self.loop_start, self.loop_end) {
            r.looped = Some((a as f32 / self.sr, b as f32 / self.sr));
        }
        if let Some(x) = self.xfade {
            r.xfade = x;
        }
        z.region = r;
        z
    }
}

/// A loaded articulation: zones + envelope defaults. Make it playable with [`Patch::sampler`].
#[derive(Clone)]
pub struct Patch {
    /// `family/instrument/articulation`.
    pub id: String,
    pub dir: PathBuf,
    pub zones: Vec<Zone>,
    pub manifest: Manifest,
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    pub gain_db: f32,
    pub tune: f32,
    pub oneshot: bool,
    pub vel_xfade: f32,
}

impl Patch {
    /// Load `family/instrument[/articulation]` from the default library.
    pub fn load(id: &str) -> Result<Patch, Error> {
        Library::open()?.get(id)
    }
    /// Lowest and highest playable key.
    pub fn range(&self) -> (f32, f32) {
        self.zones
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), z| {
                match z.keys {
                    Some((lo, hi)) => (a.min(lo), b.max(hi)),
                    None => (a.min(z.root), b.max(z.root)),
                }
            })
    }
    /// Distinct dynamic labels (`pp`..`ff`) in velocity order.
    pub fn dynamics(&self) -> Vec<String> {
        let mut v: Vec<(f32, String)> = self
            .zones
            .iter()
            .filter_map(|z| z.dynamic.clone().map(|d| (z.vel.0, d)))
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut out: Vec<String> = vec![];
        for (_, d) in v {
            if !out.contains(&d) {
                out.push(d);
            }
        }
        out
    }
    /// A [`Sampler`] playing this patch with its envelope defaults.
    pub fn sampler(&self) -> Sampler {
        let mut s = Sampler::new().named(self.id.clone());
        for z in &self.zones {
            s = s.zone(z.clone());
        }
        s = s
            .adsr(self.attack, self.decay, self.sustain, self.release)
            .gain(crate::units::db_to_gain(self.gain_db));
        if self.tune != 0.0 {
            s = s.tune(self.tune);
        }
        if self.vel_xfade > 0.0 {
            s = s.vel_xfade(self.vel_xfade);
        }
        if self.oneshot {
            s = s.oneshot();
        }
        if let Some(v) = self.manifest.num("voices") {
            s = s.voices(v as usize);
        }
        s
    }
}

impl std::fmt::Debug for Patch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Patch({}, {} zones)", self.id, self.zones.len())
    }
}

fn subdirs(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| !n.starts_with('.'))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

impl Library {
    /// Open the default library (`$UWU_LIBRARY` or `<assets>/library`).
    pub fn open() -> Result<Library, Error> {
        match library_dir() {
            Some(d) => Ok(Library { root: d }),
            None => Err(Error::Invalid(
                "sample library not found: expected <assets>/library (build it with `uwu library build`, or set UWU_LIBRARY / UWU_ASSETS)".into(),
            )),
        }
    }
    /// Open a library at an explicit folder.
    pub fn at(root: impl Into<PathBuf>) -> Library {
        Library { root: root.into() }
    }
    /// The library folder.
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Instrument families present (`strings`, `brass`…).
    pub fn families(&self) -> Vec<String> {
        subdirs(&self.root)
            .into_iter()
            .filter(|f| f != "ir")
            .collect()
    }
    /// Every instrument id (`family/instrument`) that has at least one articulation.
    pub fn instruments(&self) -> Vec<String> {
        let mut v = vec![];
        for fam in self.families() {
            for inst in subdirs(&self.root.join(&fam)) {
                let id = format!("{fam}/{inst}");
                if !self.articulations(&id).is_empty() {
                    v.push(id);
                }
            }
        }
        v
    }
    /// Articulations of an instrument (folders with a `zones.txt`).
    pub fn articulations(&self, instrument: &str) -> Vec<String> {
        let d = self.root.join(instrument);
        subdirs(&d)
            .into_iter()
            .filter(|a| d.join(a).join("zones.txt").is_file())
            .collect()
    }
    /// Impulse responses (`ir/*.wav`), by name without extension.
    pub fn irs(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(self.root.join("ir"))
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.extension().map(|x| x == "wav").unwrap_or(false))
                    .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }
    /// Path of an impulse response (`"church"`).
    pub fn ir(&self, name: &str) -> Result<PathBuf, Error> {
        let p = self
            .root
            .join("ir")
            .join(format!("{}.wav", name.trim_end_matches(".wav")));
        if p.is_file() {
            Ok(p)
        } else {
            Err(Error::Invalid(format!(
                "no impulse response {name:?} (have: {})",
                self.irs().join(", ")
            )))
        }
    }

    /// Resolve an id to its articulation folder: `family/instrument/artic`, or
    /// `family/instrument` (→ first of [`DEFAULT_ARTICS`] present, else the first one).
    pub fn resolve(&self, id: &str) -> Result<(String, PathBuf), Error> {
        let id = id.trim().trim_matches('/');
        let parts: Vec<&str> = id.split('/').collect();
        let full = match parts.len() {
            3 => id.to_string(),
            2 => {
                let arts = self.articulations(id);
                let pick = DEFAULT_ARTICS
                    .iter()
                    .find(|a| arts.iter().any(|x| x == *a))
                    .map(|a| a.to_string())
                    .or_else(|| arts.first().cloned());
                match pick {
                    Some(a) => format!("{id}/{a}"),
                    None => return Err(self.not_found(id)),
                }
            }
            _ => return Err(self.not_found(id)),
        };
        let dir = self.root.join(&full);
        if dir.join("zones.txt").is_file() {
            Ok((full, dir))
        } else {
            Err(self.not_found(&full))
        }
    }

    fn not_found(&self, id: &str) -> Error {
        let parts: Vec<&str> = id.split('/').collect();
        let inst = if parts.len() >= 2 {
            format!("{}/{}", parts[0], parts[1])
        } else {
            id.to_string()
        };
        let arts = self.articulations(&inst);
        if !arts.is_empty() {
            return Error::Invalid(format!(
                "no articulation {id:?}; {inst} has: {}",
                arts.join(", ")
            ));
        }
        let key = parts.last().copied().unwrap_or(id);
        let all = self.instruments();
        let mut near: Vec<&String> = all
            .iter()
            .filter(|i| {
                let name = i.rsplit('/').next().unwrap_or(i);
                i.starts_with(parts[0]) || name.contains(key) || key.contains(name)
            })
            .collect();
        if near.is_empty() {
            near = all.iter().collect();
        }
        let near: Vec<String> = near.into_iter().take(24).cloned().collect();
        Error::Invalid(format!(
            "no instrument {id:?} in {}; try: {}",
            self.root.display(),
            near.join(", ")
        ))
    }

    /// Load a patch (`family/instrument[/articulation]`).
    pub fn get(&self, id: &str) -> Result<Patch, Error> {
        let (full, dir) = self.resolve(id)?;
        let text = std::fs::read_to_string(dir.join("zones.txt"))?;
        let manifest =
            Manifest::parse(&text).map_err(|e| Error::Invalid(format!("{full}: {e}")))?;
        let loops = manifest.get("loop").map(|v| v != "0").unwrap_or(true);
        let zones = manifest
            .zones
            .iter()
            .map(|z| z.to_zone(&dir, loops))
            .collect();
        let num = |k: &str, d: f32| manifest.num(k).unwrap_or(d);
        Ok(Patch {
            id: full,
            dir,
            zones,
            attack: num("attack", 0.003),
            decay: num("decay", 0.0),
            sustain: num("sustain", 1.0),
            release: num("release", 0.3),
            gain_db: num("gain_db", 0.0),
            tune: num("tune", 0.0),
            oneshot: manifest.get("oneshot").map(|v| v != "0").unwrap_or(false),
            vel_xfade: num("vel_xfade", 0.0),
            manifest,
        })
    }

    /// Load a patch from the default library (`Library::patch("strings/violins-1/staccato")`).
    pub fn patch(id: &str) -> Result<Patch, Error> {
        Library::open()?.get(id)
    }
}

impl Sampler {
    /// A sampler for a library patch (`"strings/violins-1/staccato"`, or `"keys/piano"` for the
    /// default articulation). Panics with a helpful list of nearby ids if it doesn't exist.
    pub fn library(id: &str) -> Sampler {
        Sampler::try_library(id).unwrap_or_else(|e| panic!("uwu-wave: {e}"))
    }
    /// Like [`Sampler::library`] but returns an error.
    pub fn try_library(id: &str) -> Result<Sampler, Error> {
        Ok(Library::patch(id)?.sampler())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trip() {
        let text = "# test\n@attack=0.01 @release=0.8 @loop=1\nfile=G3_f_rr1.wav root=55 lo=54 hi=56 vlo=0.5 vhi=1 tune=-0.05 gain_db=-2 rr=1 dyn=f loop_start=12345 loop_end=67890\nfile=x.wav root=C4 layer=1 url=https://x/y.wav size=10 sha256=ABCD\n";
        let m = Manifest::parse(text).unwrap();
        assert_eq!(m.get("attack"), Some("0.01"));
        assert_eq!(m.zones.len(), 2);
        let z = &m.zones[0];
        assert_eq!(
            (z.root, z.lo, z.hi, z.vlo, z.vhi),
            (55.0, Some(54.0), Some(56.0), 0.5, 1.0)
        );
        assert_eq!(
            (z.rr, z.dynamic.as_deref(), z.loop_start, z.loop_end),
            (1, Some("f"), Some(12345), Some(67890))
        );
        assert_eq!(m.zones[1].root, 60.0);
        assert_eq!(m.zones[1].layer, 1);
        assert_eq!(m.zones[1].sha256.as_deref(), Some("abcd"));
        let again = Manifest::parse(&m.to_text()).unwrap();
        assert_eq!(again, m);
        let zone = z.to_zone(Path::new("/tmp"), true);
        assert_eq!(zone.keys, Some((54.0, 56.0)));
        let (a, b) = zone.region.looped.unwrap();
        assert!((a - 12345.0 / 44100.0).abs() < 1e-6 && (b - 67890.0 / 44100.0).abs() < 1e-6);
        assert!(Manifest::parse("file=a.wav").is_err());
        assert!(Manifest::parse("root=60").is_err());
        assert!(Manifest::parse("file=a.wav root=60 lo=x").is_err());
    }

    #[test]
    fn library_on_temp_tree() {
        let root = std::env::temp_dir().join(format!("uwu-lib-test-{}", std::process::id()));
        let d = root.join("brass/horn/sustain");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("zones.txt"),
            "@release=0.5\nfile=a.wav root=60 lo=50 hi=70\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("brass/horn/staccato")).unwrap();
        std::fs::write(
            root.join("brass/horn/staccato/zones.txt"),
            "file=b.wav root=62\n",
        )
        .unwrap();
        let lib = Library::at(&root);
        assert_eq!(lib.instruments(), vec!["brass/horn".to_string()]);
        assert_eq!(lib.articulations("brass/horn"), vec!["staccato", "sustain"]);
        let p = lib.get("brass/horn").unwrap();
        assert_eq!(p.id, "brass/horn/sustain");
        assert_eq!(p.release, 0.5);
        assert_eq!(p.range(), (50.0, 70.0));
        let e = lib.get("brass/horn/pizzicato").unwrap_err().to_string();
        assert!(e.contains("staccato, sustain"), "{e}");
        let e = lib.get("brass/hron").unwrap_err().to_string();
        assert!(e.contains("brass/horn"), "{e}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn real_library_opens() {
        let Ok(lib) = Library::open() else {
            eprintln!("no library, skipping");
            return;
        };
        let ids = lib.instruments();
        if ids.is_empty() {
            return;
        }
        for id in ids.iter().take(200) {
            for a in lib.articulations(id) {
                let p = lib.get(&format!("{id}/{a}")).unwrap();
                assert!(!p.zones.is_empty(), "{id}/{a}");
            }
        }
    }
}
