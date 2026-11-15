//! Command-line runner (placeholder; extended by the io module).

use crate::song::Song;

/// Render the song to `out/<name>.wav` and print a report.
pub fn run(song: Song) {
    let name = crate::engine::sanitize(&song.name());
    let path = format!("out/{name}.wav");
    match song.render_wav(&path) {
        Ok(r) => println!("{}\n→ {path}", r.report),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
