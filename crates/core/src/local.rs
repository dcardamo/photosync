use crate::{ignore::IgnoreSet, normalize_relative, Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalFile {
    /// Path relative to the Syncthing folder root, using `/`.
    pub path: String,
    pub size: u64,
    pub mtime_ns: i64,
}

pub fn safe_join(root: &Path, relative: &str) -> Result<PathBuf> {
    let relative = normalize_relative(relative)?;
    let joined = root.join(&relative);
    if !joined.starts_with(root) {
        return Err(Error::InvalidPath(format!(
            "{relative} escapes the folder root"
        )));
    }
    Ok(joined)
}

/// Walk a project on the device's disk. Ignored base patterns are skipped;
/// symlinks are rejected because they make the release gate ambiguous.
pub fn walk_files(root: &Path, relative: &str, ignores: &IgnoreSet) -> Result<Vec<LocalFile>> {
    let relative = normalize_relative(relative)?;
    let dir = safe_join(root, &relative)?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let meta = fs::symlink_metadata(&dir)?;
    if meta.file_type().is_symlink() {
        return Err(Error::Release(format!("{relative} is a symlink")));
    }
    if !meta.is_dir() {
        return Err(Error::Release(format!("{relative} is not a directory")));
    }
    let mut files = Vec::new();
    let mut walker = WalkDir::new(&dir)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter();
    while let Some(entry) = walker.next() {
        let entry = entry.map_err(|e| Error::InvalidPath(format!("walking {relative}: {e}")))?;
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .map_err(|_| Error::InvalidPath(format!("{} is outside the folder", path.display())))?
            .to_string_lossy()
            .replace('\\', "/");
        let file_type = entry.file_type();
        if file_type.is_symlink() {
            return Err(Error::Release(format!("symlink inside project: {rel}")));
        }
        if file_type.is_dir() {
            if ignores.is_ignored(&rel, true) {
                walker.skip_current_dir();
            }
            continue;
        }
        if !file_type.is_file() || ignores.is_ignored(&rel, false) {
            continue;
        }
        let meta = entry
            .metadata()
            .map_err(|e| Error::InvalidPath(format!("metadata for {rel}: {e}")))?;
        files.push(LocalFile {
            path: rel,
            size: meta.len(),
            mtime_ns: mtime_ns(&meta)?,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

#[cfg(unix)]
fn mtime_ns(meta: &fs::Metadata) -> Result<i64> {
    use std::os::unix::fs::MetadataExt;
    let seconds = meta.mtime();
    let nanos = meta.mtime_nsec();
    seconds
        .checked_mul(1_000_000_000)
        .and_then(|v| v.checked_add(nanos))
        .ok_or_else(|| Error::Release("mtime out of range".into()))
}

#[cfg(not(unix))]
fn mtime_ns(meta: &fs::Metadata) -> Result<i64> {
    let duration = meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| Error::Release("mtime before the epoch".into()))?;
    i64::try_from(duration.as_nanos()).map_err(|_| Error::Release("mtime out of range".into()))
}

/// Remove a project path from the device. Missing paths are fine.
pub fn remove_project_path(root: &Path, relative: &str) -> Result<()> {
    let path = safe_join(root, relative)?;
    let meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    if meta.file_type().is_symlink() || meta.file_type().is_file() {
        fs::remove_file(&path)?;
    } else if meta.is_dir() {
        fs::remove_dir_all(&path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::File::create(path).unwrap().write_all(bytes).unwrap();
    }

    #[test]
    fn walks_files_and_skips_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("originals/Projects/A/a.jpg"), b"one");
        write(&tmp.path().join("originals/Projects/A/.DS_Store"), b"x");
        write(&tmp.path().join("originals/Projects/A/sub/b.jpg"), b"two");
        let ignores = IgnoreSet::from_text("(?d).DS_Store\n").unwrap();
        let files = walk_files(tmp.path(), "originals/Projects/A", &ignores).unwrap();
        assert_eq!(
            files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            vec![
                "originals/Projects/A/a.jpg",
                "originals/Projects/A/sub/b.jpg"
            ]
        );
        assert_eq!(files[0].size, 3);
    }

    #[test]
    fn missing_project_is_empty_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let files =
            walk_files(tmp.path(), "originals/Projects/Nope", &IgnoreSet::default()).unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn rejects_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("p")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/passwd", tmp.path().join("p/link")).unwrap();
        #[cfg(unix)]
        assert!(walk_files(tmp.path(), "p", &IgnoreSet::default()).is_err());
    }

    #[test]
    fn removes_only_the_requested_tree() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("p/a"), b"a");
        write(&tmp.path().join("keep/b"), b"b");
        remove_project_path(tmp.path(), "p").unwrap();
        assert!(!tmp.path().join("p").exists());
        assert!(tmp.path().join("keep/b").exists());
        remove_project_path(tmp.path(), "p").unwrap();
    }
}
