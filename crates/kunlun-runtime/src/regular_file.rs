//! Capability-relative read-only opens with handle-based regular-file admission.
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
#[cfg(any(unix, windows))]
use cap_std::fs::OpenOptionsExt;
use cap_std::fs::{Dir, File, OpenOptions};
use std::io;
use std::path::Path;

/// `nofollow` rejects a final-component alias (artifact admission); ordinary
/// reads may follow aliases, but cap-std still confines resolution to `directory`.
pub(crate) fn open_regular_file(directory: &Dir, path: &Path, nofollow: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    if nofollow {
        // Raw O_NOFOLLOW is not enough: cap-std otherwise resolves the alias
        // itself and retries. Set its sandbox resolver's policy as well.
        options.follow(FollowSymlinks::No);
    }
    // Do not block on a FIFO raced into place before the handle metadata check.
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK | if nofollow { libc::O_NOFOLLOW } else { 0 });
    // cap-std's Windows open uses directory-relative handles. Opening the
    // final reparse point itself avoids following it before we inspect the
    // handle; unlike O_NOFOLLOW this flag alone does not report an error.
    #[cfg(windows)]
    if nofollow {
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = directory.open_with(path, &options)?;
    let metadata = file.metadata()?;
    #[cfg(windows)]
    {
        use cap_std::fs::MetadataExt;
        if metadata.file_attributes() & 0x0000_0400 != 0 {
            // Reject every reparse tag, not only those classified as symlinks.
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "filesystem reads reject reparse points",
            ));
        }
    }
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "filesystem reads require a regular file",
        ));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cap_std::ambient_authority;
    use std::io::Read;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(std::path::PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "kunlun-regular-file-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn directory(&self) -> Dir {
            Dir::open_ambient_dir(&self.0, ambient_authority()).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn admits_read_only_regular_files_and_rejects_directories_and_escapes() {
        let root = Fixture::new();
        let outside = Fixture::new();
        std::fs::write(root.0.join("data"), "contents").unwrap();
        std::fs::write(outside.0.join("secret"), "secret").unwrap();
        let directory = root.directory();
        for nofollow in [false, true] {
            let mut file = open_regular_file(&directory, Path::new("data"), nofollow).unwrap();
            let mut contents = String::new();
            file.read_to_string(&mut contents).unwrap();
            assert_eq!(contents, "contents");
            assert!(open_regular_file(&directory, Path::new("."), nofollow).is_err());
            assert!(open_regular_file(&directory, Path::new("../"), nofollow).is_err());
            assert!(open_regular_file(&directory, &outside.0.join("secret"), nofollow).is_err());
            assert_eq!(
                open_regular_file(&directory, Path::new("missing"), nofollow)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::NotFound
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn aliases_are_confined_and_admission_rejects_final_symlinks() {
        use std::os::unix::fs::symlink;
        let root = Fixture::new();
        let outside = Fixture::new();
        std::fs::write(root.0.join("data"), "contents").unwrap();
        std::fs::write(outside.0.join("secret"), "secret").unwrap();
        symlink("data", root.0.join("alias")).unwrap();
        symlink(outside.0.join("secret"), root.0.join("escape")).unwrap();
        let directory = root.directory();
        assert!(open_regular_file(&directory, Path::new("alias"), false).is_ok());
        assert!(open_regular_file(&directory, Path::new("alias"), true).is_err());
        for nofollow in [false, true] {
            assert!(open_regular_file(&directory, Path::new("escape"), nofollow).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_fifo_without_waiting_for_a_writer() {
        let root = Fixture::new();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(root.0.join("fifo"))
                .status()
                .unwrap()
                .success()
        );
        let directory = root.directory();
        for nofollow in [false, true] {
            assert!(open_regular_file(&directory, Path::new("fifo"), nofollow).is_err());
        }
    }

    #[cfg(windows)]
    #[test]
    fn junction_reparse_points_cannot_escape_the_directory() {
        // Junction creation does not require the symlink privilege or Developer
        // Mode, so this exercises native reparse handling on ordinary Windows CI.
        let root = Fixture::new();
        let outside = Fixture::new();
        std::fs::write(outside.0.join("secret"), "secret").unwrap();
        let junction = root.0.join("escape");
        assert!(
            std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&junction)
                .arg(&outside.0)
                .status()
                .unwrap()
                .success()
        );
        let directory = root.directory();
        for nofollow in [false, true] {
            assert!(open_regular_file(&directory, Path::new("escape"), nofollow).is_err());
            assert!(open_regular_file(&directory, Path::new("escape/secret"), nofollow).is_err());
        }
        // Explicitly remove the junction, not its destination.
        std::fs::remove_dir(junction).unwrap();
    }
}
