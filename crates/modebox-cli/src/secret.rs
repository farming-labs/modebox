//! The key that seals continuation tokens.
//!
//! An approval can arrive after the process that asked for it has exited
//! (MCP `input_required`, `modebox resume`), so the key must outlive the
//! process. Unless `MODEBOX_SECRET` is set, one is generated on first use
//! and kept in a file only the user can read.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// `$MODEBOX_SECRET_FILE`, else `$XDG_CONFIG_HOME/modebox/secret`, else
/// `~/.config/modebox/secret`.
pub fn default_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("MODEBOX_SECRET_FILE") {
        return Some(p.into());
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("modebox").join("secret"))
}

/// Read the key at `path`, creating it (owner-only) if it doesn't exist.
pub fn load_or_create(path: &Path) -> Result<Vec<u8>> {
    match std::fs::read_to_string(path) {
        Ok(s) => return check(path, s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    }
    if let Some(dir) = path.parent() {
        let mut b = std::fs::DirBuilder::new();
        b.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
        b.create(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let key: String = rand::random::<[u8; 32]>().iter().map(|b| format!("{b:02x}")).collect();
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    match opts.open(path) {
        Ok(mut f) => {
            writeln!(f, "{key}").with_context(|| format!("writing {}", path.display()))?;
            Ok(key.into_bytes())
        }
        // Another process created it first: use theirs.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            check(path, std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?)
        }
        Err(e) => Err(e).with_context(|| format!("creating {}", path.display())),
    }
}

fn check(path: &Path, contents: String) -> Result<Vec<u8>> {
    let key = contents.trim();
    if key.len() < 32 {
        bail!("{} holds a key shorter than 32 characters; delete it to generate a new one", path.display());
    }
    #[cfg(unix)]
    if let Ok(meta) = std::fs::metadata(path) {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            eprintln!("modebox: warning: {} is readable by other users; run `chmod 600` on it", path.display());
        }
    }
    Ok(key.as_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("modebox-secret-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("nested").join("secret")
    }

    #[test]
    fn creates_once_then_reuses() {
        let p = temp("reuse");
        let a = load_or_create(&p).unwrap();
        let b = load_or_create(&p).unwrap();
        assert_eq!(a, b, "the same key on every start");
        assert_eq!(a.len(), 64);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(p.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        }
    }

    #[test]
    fn rejects_a_weak_key() {
        let p = temp("weak");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "short").unwrap();
        assert!(load_or_create(&p).is_err());
    }
}
