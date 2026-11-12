//! Where yopes keeps the files it makes while it works.
//!
//! On Windows the scratch files live in `%USERPROFILE%\Temp`, which is where
//! the user expects to find them. Everywhere else the platform's temporary
//! directory is used, honouring `TMPDIR` when it is set.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// The directory yopes writes its temporary files to.
///
/// The directory is created when it does not exist yet, so a caller can write
/// into it straight away.
pub fn temp_root() -> PathBuf {
    let root = platform_temp_root();
    // A directory that cannot be created is not worth failing over: the
    // caller falls back to writing wherever it can.
    let _ = fs::create_dir_all(&root);
    root
}

/// The directory yopes writes its temporary files to, without creating it.
pub fn platform_temp_root() -> PathBuf {
    if cfg!(windows) {
        if let Some(profile) = env::var_os("USERPROFILE").filter(|value| !value.is_empty()) {
            return PathBuf::from(profile).join("Temp");
        }
    }
    env::temp_dir()
}

/// A file name unique to this process and this moment, under the temp root.
///
/// The name carries the process id, so two runs never share a file, and the
/// timestamp, so one run never reuses a file.
pub fn unique_path(prefix: &str, extension: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    temp_root().join(format!(
        "{prefix}-{}-{nanos}.{extension}",
        std::process::id()
    ))
}

/// A directory of this process's own, under the temp root.
///
/// The caller removes it when it is done with it.
pub fn unique_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let path = temp_root().join(format!("{prefix}-{}-{nanos}", std::process::id()));
    let _ = fs::create_dir_all(&path);
    path
}

/// Remove a directory and everything in it, ignoring what is already gone.
pub fn remove_dir_all(path: &Path) {
    let _ = fs::remove_dir_all(path);
}

#[cfg(test)]
mod tests {
    use super::{platform_temp_root, unique_dir, unique_path};

    #[test]
    fn the_temp_root_is_the_user_profile_temp_on_windows() {
        let root = platform_temp_root();
        assert!(!root.as_os_str().is_empty());
        if cfg!(windows) {
            assert!(
                root.ends_with("Temp"),
                "expected a Temp folder, got {}",
                root.display()
            );
        }
    }

    #[test]
    fn unique_names_do_not_collide() {
        let first = unique_path("yopes-test", "log");
        let second = unique_path("yopes-test", "log");
        assert_ne!(first, second);
        assert!(first.to_string_lossy().contains("yopes-test"));
    }

    #[test]
    fn a_unique_dir_is_created() {
        let dir = unique_dir("yopes-test");
        assert!(dir.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
