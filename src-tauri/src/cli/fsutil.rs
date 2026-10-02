//! Writing the files twapp keeps state in.

use std::path::Path;

/// Replace `path` with `contents` so that a reader, or a crash mid-write,
/// sees either the old file or the new one, never a truncated mix: the bytes
/// go to a temporary file beside it, which is then renamed over it.
pub fn write_atomic(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let tmp = dir.join(format!(".{}.{}.tmp", name, std::process::id()));
    let result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(contents.as_ref())?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Replace private state atomically, with owner-only permissions from file creation.
pub fn write_atomic_private(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let tmp = dir.join(format!(".twapp-private-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true)
            .mode(0o600).custom_flags(libc::O_NOFOLLOW).open(&tmp)?;
        file.write_all(contents.as_ref())?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() { let _ = std::fs::remove_file(&tmp); }
    result
}

/// Open an ordinary file without following a substituted symlink or blocking on a FIFO.
pub fn open_regular_file(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    if !std::fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(std::io::Error::other(
            "source must be a regular file, not a symlink",
        ));
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("source must be a regular file"));
    }
    Ok(file)
}

pub fn read_regular_file(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut file = open_regular_file(path)?;
    let mut content = Vec::new();
    file.read_to_end(&mut content)?;
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_atomic_writes_replace_redirected_destinations_without_exposing_contents() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("twapp-private-write-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let target = dir.join("unrelated.json");
        std::fs::write(&target, "unchanged").unwrap();
        let path = dir.join("notes.json");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        write_atomic_private(&path, "private copy").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "unchanged");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "private copy");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(std::fs::read_dir(&dir).unwrap().flatten().all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp")));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_write_replaces_the_file_and_leaves_no_temporary_behind() {
        let dir = std::env::temp_dir().join(format!("twapp-fsutil-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        std::fs::write(&path, "old").unwrap();
        write_atomic(&path, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(names.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
