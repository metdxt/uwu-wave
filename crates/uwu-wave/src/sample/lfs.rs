//! Transparent git-LFS pointer resolution.
//!
//! The `assets/wavebase` folder is a copy of the [cluesurf/wave](https://github.com/cluesurf/wave)
//! repository whose WAVs are stored as git-LFS *pointer files* (tiny text files containing an
//! object id and a size). When such a pointer is loaded, the real file is downloaded once
//! (`curl -sSfL`) into `$XDG_CACHE_HOME/uwu-wave/lfs/<oid>.wav` (or `~/.cache/...`), verified
//! (size + SHA-256) and reused from then on.
//!
//! * `UWU_OFFLINE=1` disables downloading (loading an uncached pointer is then an error).
//! * `UWU_LFS_BASE=<url>` overrides the raw-download base URL
//!   (default `https://github.com/cluesurf/wave/raw/make/`); the path relative to the
//!   `wavebase` folder is appended.

use crate::Error;
use std::path::{Path, PathBuf};

/// Default download base for files below a `wavebase/` folder.
pub const DEFAULT_LFS_BASE: &str = "https://github.com/cluesurf/wave/raw/make/";

/// A parsed git-LFS pointer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LfsPointer {
    /// Hex SHA-256 of the real content.
    pub oid: String,
    /// Size in bytes of the real content.
    pub size: u64,
}

impl LfsPointer {
    /// Parse pointer text (returns `None` for anything else).
    pub fn parse(text: &str) -> Option<LfsPointer> {
        if !text.starts_with("version https://git-lfs") {
            return None;
        }
        let mut oid = None;
        let mut size = None;
        for line in text.lines() {
            if let Some(o) = line.strip_prefix("oid sha256:") {
                oid = Some(o.trim().to_string());
            } else if let Some(s) = line.strip_prefix("size ") {
                size = s.trim().parse().ok();
            }
        }
        let oid = oid?;
        if oid.len() != 64 || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        Some(LfsPointer {
            oid: oid.to_ascii_lowercase(),
            size: size?,
        })
    }

    /// Read `path` and parse it if it is a pointer file (cheap: only small files are read).
    pub fn read(path: &Path) -> Option<LfsPointer> {
        let meta = std::fs::metadata(path).ok()?;
        if meta.len() > 1024 {
            return None;
        }
        let text = std::fs::read_to_string(path).ok()?;
        LfsPointer::parse(&text)
    }
}

/// `$XDG_CACHE_HOME/uwu-wave` or `~/.cache/uwu-wave`.
pub fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("uwu-wave")
}

static QUIET: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// While the returned guard lives, per-file "fetching" notices are suppressed (batch loaders
/// print their own progress line).
pub fn quiet() -> QuietGuard {
    QUIET.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    QuietGuard(())
}

/// See [`quiet`].
pub struct QuietGuard(());
impl Drop for QuietGuard {
    fn drop(&mut self) {
        QUIET.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Is downloading disabled (`UWU_OFFLINE=1`)?
pub fn offline() -> bool {
    std::env::var("UWU_OFFLINE")
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false)
}

/// Where the real content of a pointer is cached.
pub fn cached_path(ptr: &LfsPointer) -> PathBuf {
    cache_dir().join("lfs").join(format!("{}.wav", ptr.oid))
}

/// Is `path` a real file, or a pointer whose content is already cached?
pub fn is_available(path: &Path) -> bool {
    match LfsPointer::read(path) {
        None => path.is_file(),
        Some(ptr) => std::fs::metadata(cached_path(&ptr))
            .map(|m| m.len() == ptr.size)
            .unwrap_or(false),
    }
}

/// If `path` is an LFS pointer, return the path of the real (cached, downloaded on demand)
/// file; otherwise return `path` unchanged.
pub fn resolve(path: &Path) -> Result<PathBuf, Error> {
    let Some(ptr) = LfsPointer::read(path) else {
        return Ok(path.to_path_buf());
    };
    let cached = cached_path(&ptr);
    if let Ok(m) = std::fs::metadata(&cached)
        && m.len() == ptr.size
    {
        return Ok(cached);
    }
    if offline() {
        return Err(Error::Invalid(format!(
            "{} is a git-LFS pointer that is not cached yet and UWU_OFFLINE is set (unset it to download, or run `git lfs pull`)",
            path.display()
        )));
    }
    let url = url_for(path)?;
    fetch(&url, ptr.size, &ptr.oid, &rel_display(path))
}

/// Download `url` (expected `size` bytes, SHA-256 `oid`) into the LFS cache — or return the
/// cached copy. Used for git-LFS pointers and for remote zones of library manifests
/// (`url=… size=… sha256=…`).
pub fn fetch(url: &str, size: u64, oid: &str, label: &str) -> Result<PathBuf, Error> {
    let ptr = LfsPointer {
        oid: oid.to_ascii_lowercase(),
        size,
    };
    let cached = cached_path(&ptr);
    if let Ok(m) = std::fs::metadata(&cached)
        && m.len() == ptr.size
    {
        return Ok(cached);
    }
    if offline() {
        return Err(Error::Invalid(format!(
            "{label} is remote ({url}), not cached yet, and UWU_OFFLINE is set"
        )));
    }
    let dir = cached.parent().map(Path::to_path_buf).unwrap_or_default();
    std::fs::create_dir_all(&dir)?;
    if QUIET.load(std::sync::atomic::Ordering::Relaxed) == 0 {
        eprintln!("⇣ fetching {label} ({:.1} MB)", ptr.size as f64 / 1e6);
    }
    static PART: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let part = dir.join(format!(
        "{}.{}.{}.part",
        ptr.oid,
        std::process::id(),
        PART.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let status = std::process::Command::new("curl")
        .arg("-sSfL")
        .arg("--retry")
        .arg("2")
        .arg("-o")
        .arg(&part)
        .arg(url)
        .status()
        .map_err(|e| Error::Other(format!("could not run curl to fetch {url}: {e}")))?;
    if !status.success() {
        let _ = std::fs::remove_file(&part);
        return Err(Error::Other(format!("download failed ({status}): {url}")));
    }
    let bytes = std::fs::read(&part)?;
    if bytes.len() as u64 != ptr.size {
        let _ = std::fs::remove_file(&part);
        return Err(Error::Other(format!(
            "download of {url}: expected {} bytes, got {}",
            ptr.size,
            bytes.len()
        )));
    }
    let hash = hex(&sha256(&bytes));
    if hash != ptr.oid {
        let _ = std::fs::remove_file(&part);
        return Err(Error::Other(format!(
            "download of {url}: sha256 mismatch ({hash} ≠ {})",
            ptr.oid
        )));
    }
    std::fs::rename(&part, &cached)?;
    Ok(cached)
}

/// Is a remote file already in the cache?
pub fn is_cached(size: u64, oid: &str) -> bool {
    let p = cached_path(&LfsPointer {
        oid: oid.to_ascii_lowercase(),
        size,
    });
    std::fs::metadata(p)
        .map(|m| m.len() == size)
        .unwrap_or(false)
}

fn rel_display(path: &Path) -> String {
    let s = path.to_string_lossy();
    match s.find("wavebase/") {
        Some(i) => s[i + "wavebase/".len()..].to_string(),
        None => s.into_owned(),
    }
}

/// Download URL: the base URL + the path relative to the closest `wavebase` ancestor.
pub fn url_for(path: &Path) -> Result<String, Error> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let comps: Vec<String> = abs
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let Some(i) = comps.iter().rposition(|c| c == "wavebase") else {
        return Err(Error::Invalid(format!(
            "{} is a git-LFS pointer outside a `wavebase` folder; don't know where to download it from (run `git lfs pull`)",
            path.display()
        )));
    };
    let rel: Vec<String> = comps[i + 1..].iter().map(|c| url_escape(c)).collect();
    let base = std::env::var("UWU_LFS_BASE")
        .ok()
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| DEFAULT_LFS_BASE.to_string());
    let sep = if base.ends_with('/') { "" } else { "/" };
    Ok(format!("{base}{sep}{}", rel.join("/")))
}

fn url_escape(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Minimal SHA-256 (FIPS 180-4), used to verify downloads.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let full = data.len() / 64 * 64;
    let mut tail = Vec::with_capacity(128);
    tail.extend_from_slice(&data[full..]);
    tail.push(0x80);
    while tail.len() % 64 != 56 {
        tail.push(0);
    }
    tail.extend_from_slice(&bit_len.to_be_bytes());
    let mut w = [0u32; 64];
    let (body, _) = data[..full].as_chunks::<64>();
    let (pad, _) = tail.as_chunks::<64>();
    for block in body.iter().chain(pad.iter()) {
        let (words, _) = block.as_chunks::<4>();
        for (i, c) in words.iter().enumerate() {
            w[i] = u32::from_be_bytes(*c);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vectors() {
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let long = vec![b'a'; 1000];
        assert_eq!(
            hex(&sha256(&long)),
            "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
        );
    }

    #[test]
    fn parses_pointer() {
        let p = LfsPointer::parse(
            "version https://git-lfs.github.com/spec/v1\noid sha256:0d8e1a7c4cfb96473038448734943c8c569772686cbaf170dff3854101f711c9\nsize 97406\n",
        )
        .unwrap();
        assert_eq!(p.size, 97406);
        assert!(LfsPointer::parse("RIFF....").is_none());
    }

    #[test]
    fn url_is_relative_to_wavebase() {
        let u = url_for(Path::new(
            "/x/assets/wavebase/base/bass/string-1-A-as-A2.wav",
        ))
        .unwrap();
        assert!(
            u.ends_with("/raw/make/base/bass/string-1-A-as-A2.wav"),
            "{u}"
        );
    }
}
