//! `uwu` — a tiny live-coding helper for uwu-wave songs (feature `live`).
//!
//! ```text
//! uwu list                         list the examples (songs)
//! uwu new <name>                   scaffold examples/<name>.rs from a template
//! uwu play <name> [args...]        build in release mode and play once (--play)
//! uwu render <name> [args...]      build and run with the given args (renders a wav by default)
//! uwu live <name> [args...]        play, and restart on every save of examples/<name>.rs or src/
//! ```
//! Extra args go to the song (e.g. `uwu live my_song --from chorus --loop`).
//! Run it with `cargo run --release --bin uwu -- live my_song` (or `cargo install --path crates/uwu-wave`).

#[cfg(not(feature = "live"))]
fn main() {
    eprintln!("uwu was built without the `live` feature");
    std::process::exit(1);
}

#[cfg(feature = "live")]
fn main() {
    live::main()
}

#[cfg(feature = "live")]
mod live {
    use notify::{EventKind, RecursiveMode, Watcher};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    const USAGE: &str = "\
usage: uwu <command> [args]

  list                       list the examples (songs)
  new <name>                 scaffold examples/<name>.rs from a template
  play <name> [args...]      build (release) and play once
  render <name> [args...]    build and run with args (renders out/<name>.wav by default)
  live <name> [args...]      play and restart on every save of examples/<name>.rs or src/
  library build [--sources DIR] [--out DIR] [--only vpo,phil,piano,wavebase] [--jobs N]
                             (re)build assets/library from the source mirrors
  library import philharmonia [ARCHIVE|DIR...] [--out DIR] [--jobs N]
                             convert the official Philharmonia downloads into assets/local
  library list [filter]      list instruments and articulations ([local] = overlay)
  library info <id>          zones, range and dynamics of a patch
  library index              regenerate assets/library/INDEX.md

song args are passed through, e.g.  uwu live my_song --from chorus --loop
environment: UWU_CRATE=<path to crates/uwu-wave> to use another checkout";

    fn library(args: &[String]) {
        use uwu_wave::sample::build::{self, BuildOptions};
        use uwu_wave::sample::library::{Library, library_dir};
        let sub = args.first().map(String::as_str).unwrap_or("list");
        if args.iter().any(|a| a == "-h" || a == "--help") {
            println!("{USAGE}");
            return;
        }
        if sub == "build" {
            // reject typos instead of silently rebuilding the whole library with defaults
            let known = ["--sources", "--out", "--only", "--jobs"];
            let mut i = 1;
            while i < args.len() {
                if !known.contains(&args[i].as_str()) {
                    die(format!(
                        "unknown option {:?} for `uwu library build` (expected {})",
                        args[i],
                        known.join(", ")
                    ));
                }
                i += 2;
            }
        }
        let flag = |name: &str| {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1))
                .cloned()
        };
        let default_out = || {
            library_dir()
                .or_else(|| uwu_wave::sample::assets_dir().map(|a| a.join("library")))
                .unwrap_or_else(|| PathBuf::from("assets/library"))
        };
        match sub {
            "build" => {
                let opts = BuildOptions {
                    sources: flag("--sources")
                        .map(PathBuf::from)
                        .unwrap_or_else(build::default_sources),
                    out: flag("--out").map(PathBuf::from).unwrap_or_else(default_out),
                    only: flag("--only")
                        .map(|s| s.split(',').map(String::from).collect())
                        .unwrap_or_default(),
                    jobs: flag("--jobs")
                        .and_then(|j| j.parse().ok())
                        .unwrap_or_else(|| {
                            std::thread::available_parallelism()
                                .map(|n| n.get())
                                .unwrap_or(4)
                        }),
                };
                eprintln!(
                    "sources: {}
library: {}",
                    opts.sources.display(),
                    opts.out.display()
                );
                if let Err(e) = build::build(&opts) {
                    die(e);
                }
            }
            "index" => {
                let mut roots = vec![default_out()];
                if let Some(l) = uwu_wave::sample::library::local_dir().filter(|d| d.is_dir()) {
                    roots.push(l);
                }
                for r in roots {
                    match build::reindex(&r) {
                        Ok(n) => println!("✓ {}/INDEX.md: {n} patches", r.display()),
                        Err(e) => die(e),
                    }
                }
            }
            "import" => {
                let what = args.get(1).map(String::as_str).unwrap_or("");
                if what != "philharmonia" {
                    die(
                        "usage: uwu library import philharmonia [ARCHIVE|DIR...] [--out DIR] [--jobs N]",
                    );
                }
                let mut inputs: Vec<PathBuf> = vec![];
                let mut i = 2;
                while i < args.len() {
                    match args[i].as_str() {
                        "--out" | "--jobs" => i += 2,
                        a => {
                            inputs.push(PathBuf::from(a));
                            i += 1;
                        }
                    }
                }
                if inputs.is_empty() {
                    inputs = build::find_philharmonia_archives();
                    if inputs.is_empty() {
                        eprintln!(
                            "No Philharmonia archives found.\n\n1. Download the samples (free) from {}\n   (e.g. all-samples.zip, or Brass.zip / Strings.zip / Woodwind.zip / Percussion.zip)\n2. Put the zips next to this repo, in its parent folder or in ~/Downloads — or pass their paths:\n   uwu library import philharmonia ~/Downloads/all-samples.zip",
                            build::PHILHARMONIA_URL
                        );
                        std::process::exit(1);
                    }
                }
                let out = flag("--out").map(PathBuf::from).unwrap_or_else(|| {
                    uwu_wave::sample::library::local_dir()
                        .unwrap_or_else(|| PathBuf::from("assets/local"))
                });
                let jobs = flag("--jobs")
                    .and_then(|j| j.parse().ok())
                    .unwrap_or_else(|| {
                        std::thread::available_parallelism()
                            .map(|n| n.get())
                            .unwrap_or(4)
                    });
                eprintln!("importing Philharmonia from:");
                for i in &inputs {
                    eprintln!("  {}", i.display());
                }
                eprintln!("into {}", out.display());
                let t0 = std::time::Instant::now();
                match build::import_philharmonia(&build::ImportOptions {
                    inputs,
                    out: out.clone(),
                    jobs,
                }) {
                    Ok(s) => {
                        println!(
                            "✓ imported {} instruments, {} patches, {:.0} MB from {} mp3s ({} duplicates skipped) in {:.0}s → {}",
                            s.instruments.len(),
                            s.patches,
                            s.bytes as f64 / 1e6,
                            s.mp3s,
                            s.duplicates,
                            t0.elapsed().as_secs_f32(),
                            out.display()
                        );
                        println!("  {}", s.instruments.join(" "));
                        if !s.unknown.is_empty() {
                            println!("  skipped unknown instruments: {}", s.unknown.join(", "));
                        }
                    }
                    Err(e) => die(e),
                }
            }
            "info" => {
                let id = args.get(1).unwrap_or_else(|| {
                    die("usage: uwu library info <family/instrument[/articulation]>")
                });
                let p = Library::open()
                    .and_then(|l| l.get(id))
                    .unwrap_or_else(|e| die(e));
                let (lo, hi) = p.range();
                println!(
                    "{} — {} zones, keys {}–{}, dynamics [{}], attack {} release {}",
                    p.id,
                    p.zones.len(),
                    lo,
                    hi,
                    p.dynamics().join(" "),
                    p.attack,
                    p.release
                );
                println!("dir: {}", p.dir.display());
                if let Ok(lib) = Library::open() {
                    let roots: Vec<String> = lib
                        .roots()
                        .iter()
                        .map(|r| r.display().to_string())
                        .collect();
                    println!("roots: {}", roots.join("  >  "));
                }
            }
            "list" | "ls" => {
                let lib = Library::open().unwrap_or_else(|e| die(e));
                let filter = args.get(1).cloned().unwrap_or_default();
                for id in lib
                    .instruments()
                    .into_iter()
                    .filter(|i| i.contains(&filter))
                {
                    let mark = if lib.is_local(&id) { "[local]" } else { "" };
                    println!("{id:<34} {mark:<8} {}", lib.articulations(&id).join(" "));
                }
                println!("ir: {}", lib.irs().join(" "));
            }
            other => die(format!(
                "unknown library command {other:?} (build, import, list, info, index)"
            )),
        }
    }

    fn crate_dir() -> PathBuf {
        std::env::var_os("UWU_CRATE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
    }

    fn target_dir(krate: &Path) -> PathBuf {
        if let Some(t) = std::env::var_os("CARGO_TARGET_DIR") {
            return PathBuf::from(t);
        }
        for a in krate.ancestors() {
            let toml = a.join("Cargo.toml");
            if std::fs::read_to_string(&toml)
                .map(|s| s.contains("[workspace]"))
                .unwrap_or(false)
            {
                return a.join("target");
            }
        }
        krate.join("target")
    }

    fn examples(krate: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(krate.join("examples"))
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.extension().map(|e| e == "rs").unwrap_or(false))
                    .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    fn die(msg: impl std::fmt::Display) -> ! {
        eprintln!("uwu: {msg}");
        std::process::exit(1)
    }

    fn check_name(krate: &Path, name: &str) {
        if !krate.join("examples").join(format!("{name}.rs")).is_file() {
            let ex = examples(krate);
            die(format!(
                "no example {name:?} (have: {}); create it with `uwu new {name}`",
                ex.join(", ")
            ));
        }
    }

    /// `cargo build --release --example <name>`; returns the binary path on success.
    fn build(krate: &Path, name: &str) -> Option<PathBuf> {
        eprintln!("⚙ building {name}…");
        let t = Instant::now();
        let ok = Command::new("cargo")
            .args(["build", "--release", "--example", name])
            .current_dir(krate)
            .status()
            .map(|s| s.success())
            .unwrap_or_else(|e| die(format!("could not run cargo: {e}")));
        if !ok {
            return None;
        }
        eprintln!("✓ built in {:.1}s", t.elapsed().as_secs_f32());
        let exe = target_dir(krate)
            .join("release")
            .join("examples")
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        if exe.is_file() {
            Some(exe)
        } else {
            die(format!("built, but {} is missing", exe.display()))
        }
    }

    fn spawn(exe: &Path, args: &[String]) -> Option<Child> {
        match Command::new(exe).args(args).spawn() {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("uwu: could not start {}: {e}", exe.display());
                None
            }
        }
    }

    fn stop(child: &mut Option<Child>) {
        if let Some(mut c) = child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    fn template(name: &str) -> String {
        format!(
            r#"//! {name} — a song made with uwu-wave.
//!
//! cargo run --release --example {name} -- --play     (or: uwu live {name})
use uwu_wave::prelude::*;

fn main() {{
    let song = Song::new("{name}").bpm(100);
    let keys = song.track("keys", Sampler::piano().release(0.5).gain(-3.db()));
    let drums = song.track("drums", SampleKit::acoustic().gain(-6.db()));

    // one chord per bar: Cmaj7 – Am7 – Fmaj7 – G7
    let chords = [[48, 55, 64, 71], [45, 55, 60, 67], [41, 57, 60, 64], [43, 55, 59, 65]];
    let mut prog = Clip::new(16.beats());
    for (bar, chord) in chords.iter().enumerate() {{
        for (k, &p) in chord.iter().enumerate() {{
            // a gentle strum
            prog = prog.note(bar as f64 * 4.0 + 0.03 * k as f64, 3.8, p, 0.6);
        }}
    }}
    let beat = Clip::new(4.beats())
        .note(0, 0.25, 36, 1.0)
        .note(1, 0.25, 38, 0.8)
        .note(2, 0.25, 36, 0.9)
        .note(3, 0.25, 38, 0.8)
        .note(0.5, 0.25, 42, 0.5)
        .note(1.5, 0.25, 42, 0.5)
        .note(2.5, 0.25, 42, 0.5)
        .note(3.5, 0.25, 46, 0.6);

    song.section("intro", 4.bars(), |s| {{
        s.play(&keys, prog.clone());
    }});
    song.section("verse", 8.bars(), |s| {{
        s.play(&keys, prog.clone());
        s.play(&drums, beat.clone());
    }});
    song.run();
}}
"#
        )
    }

    pub fn main() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let krate = crate_dir();
        let cmd = args.first().map(String::as_str).unwrap_or("");
        let rest = |i: usize| -> Vec<String> {
            args.iter()
                .skip(i)
                .filter(|a| a.as_str() != "--")
                .cloned()
                .collect()
        };
        match cmd {
            "library" | "lib" => library(&rest(1)),
            "list" | "ls" => {
                for e in examples(&krate) {
                    println!("{e}");
                }
            }
            "new" => {
                let name = args.get(1).unwrap_or_else(|| die("usage: uwu new <name>"));
                if name.is_empty()
                    || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    || name.starts_with(|c: char| c.is_ascii_digit())
                {
                    die("song names must be snake_case identifiers (letters, digits, _)");
                }
                let p = krate.join("examples").join(format!("{name}.rs"));
                if p.exists() {
                    die(format!("{} already exists", p.display()));
                }
                std::fs::create_dir_all(p.parent().unwrap()).unwrap_or_else(|e| die(e));
                std::fs::write(&p, template(name)).unwrap_or_else(|e| die(e));
                println!("✨ created {}\n   try: uwu live {name}", p.display());
            }
            "play" | "render" => {
                let name = args
                    .get(1)
                    .unwrap_or_else(|| die(format!("usage: uwu {cmd} <name> [args...]")));
                check_name(&krate, name);
                let mut a = rest(2);
                if cmd == "play" && !a.iter().any(|x| x == "--play" || x == "-p") {
                    a.insert(0, "--play".into());
                }
                let Some(exe) = build(&krate, name) else {
                    std::process::exit(1)
                };
                let st = Command::new(exe)
                    .args(&a)
                    .status()
                    .unwrap_or_else(|e| die(e));
                std::process::exit(st.code().unwrap_or(1));
            }
            "live" => {
                let name = args
                    .get(1)
                    .unwrap_or_else(|| die("usage: uwu live <name> [args...]"))
                    .clone();
                check_name(&krate, &name);
                let mut a = rest(2);
                if !a.iter().any(|x| x == "--play" || x == "-p") {
                    a.insert(0, "--play".into());
                }
                live(&krate, &name, &a);
            }
            "" | "-h" | "--help" | "help" => println!("{USAGE}"),
            other => {
                eprintln!("uwu: unknown command {other:?}\n\n{USAGE}");
                std::process::exit(2);
            }
        }
    }

    fn live(krate: &Path, name: &str, args: &[String]) -> ! {
        let file = krate.join("examples").join(format!("{name}.rs"));
        let src = krate.join("src");
        let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
        let mut w = notify::recommended_watcher(tx)
            .unwrap_or_else(|e| die(format!("cannot watch files: {e}")));
        w.watch(&krate.join("examples"), RecursiveMode::NonRecursive)
            .unwrap_or_else(|e| die(e));
        if src.is_dir() {
            w.watch(&src, RecursiveMode::Recursive)
                .unwrap_or_else(|e| die(e));
        }
        let file_name = file.file_name().map(|f| f.to_os_string());
        let relevant = |ev: &notify::Event| {
            matches!(
                ev.kind,
                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
            ) && ev.paths.iter().any(|p| {
                p.extension().map(|e| e == "rs").unwrap_or(false)
                    && (p.starts_with(&src)
                        || p.file_name()
                            .map(|f| Some(f.to_os_string()) == file_name)
                            .unwrap_or(false))
            })
        };
        eprintln!(
            "👀 live: {} (+ src/) — save to restart, Ctrl-C to quit",
            file.display()
        );
        let mut child: Option<Child> = None;
        let start = |child: &mut Option<Child>| {
            stop(child);
            if let Some(exe) = build(krate, name) {
                *child = spawn(&exe, args);
            } else {
                eprintln!("✗ build failed — fix it and save again");
            }
        };
        start(&mut child);
        loop {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(Ok(ev)) if relevant(&ev) => {
                    // debounce editor bursts (write + rename + chmod...)
                    let until = Instant::now() + Duration::from_millis(250);
                    while let Some(left) = until.checked_duration_since(Instant::now()) {
                        if rx.recv_timeout(left).is_err() {
                            break;
                        }
                    }
                    eprintln!("\n↻ change detected");
                    start(&mut child);
                }
                Ok(Ok(_)) => {}
                Ok(Err(e)) => eprintln!("uwu: watch error: {e}"),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => die("file watcher stopped"),
            }
            if let Some(c) = child.as_mut()
                && let Ok(Some(st)) = c.try_wait()
            {
                child = None;
                eprintln!("■ finished ({st}) — waiting for changes");
            }
        }
    }
}
