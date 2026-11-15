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

song args are passed through, e.g.  uwu live my_song --from chorus --loop
environment: UWU_CRATE=<path to crates/uwu-wave> to use another checkout";

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
            if let Some(c) = child.as_mut() {
                if let Ok(Some(st)) = c.try_wait() {
                    child = None;
                    eprintln!("■ finished ({st}) — waiting for changes");
                }
            }
        }
    }
}
