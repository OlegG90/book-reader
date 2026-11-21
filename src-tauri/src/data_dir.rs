use std::fs;
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "breader.json";
const APP_DIR: &str = "book-reader";

/// Picks where the state file lives: next to the exe when that folder is
/// writable (portable mode), otherwise `%APPDATA%\book-reader`.
pub fn state_file(exe_dir: Option<&Path>, app_data: Option<&Path>) -> PathBuf {
    match (exe_dir, app_data) {
        (Some(dir), _) if is_writable(dir) => dir.join(FILE_NAME),
        (_, Some(app_data)) => app_data.join(APP_DIR).join(FILE_NAME),
        (Some(dir), None) => dir.join(FILE_NAME),
        (None, None) => PathBuf::from(FILE_NAME),
    }
}

/// Resolves the state file: in `data_dir` when one is given (`--data-dir`),
/// otherwise next to the running exe or in %APPDATA%.
pub fn resolve_state_file(data_dir: Option<&Path>) -> PathBuf {
    if let Some(dir) = data_dir {
        return dir.join(FILE_NAME);
    }
    let app_data = std::env::var_os("APPDATA").map(PathBuf::from);
    state_file(exe_dir().as_deref(), app_data.as_deref())
}

/// The library folder offered on first run: `lib` next to the exe when that
/// folder is writable (portable mode), otherwise `Documents\Book Reader`.
pub fn default_library(exe_dir: Option<&Path>, home: Option<&Path>) -> PathBuf {
    match (exe_dir, home) {
        (Some(dir), _) if is_writable(dir) => dir.join("lib"),
        (_, Some(home)) => home.join("Documents").join("Book Reader"),
        (Some(dir), None) => dir.join("lib"),
        (None, None) => PathBuf::from("lib"),
    }
}

/// The first-run library folder for this exe and user.
pub fn resolve_default_library() -> PathBuf {
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
    default_library(exe_dir().as_deref(), home.as_deref())
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf))
}

fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".breader-probe-{}", std::process::id()));
    match fs::File::create(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writable_exe_dir_is_portable() {
        let exe = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let file = state_file(Some(exe.path()), Some(app_data.path()));
        assert_eq!(file, exe.path().join(FILE_NAME));
        assert_eq!(fs::read_dir(exe.path()).unwrap().count(), 0, "probe file left behind");
    }

    #[test]
    fn missing_exe_dir_falls_back_to_app_data() {
        let app_data = tempfile::tempdir().unwrap();
        let gone = app_data.path().join("does-not-exist");
        let file = state_file(Some(&gone), Some(app_data.path()));
        assert_eq!(file, app_data.path().join(APP_DIR).join(FILE_NAME));
    }

    #[test]
    fn library_defaults_to_lib_next_to_a_writable_exe() {
        let exe = tempfile::tempdir().unwrap();
        let home = Path::new(r"C:\Users\me");
        assert_eq!(default_library(Some(exe.path()), Some(home)), exe.path().join("lib"));
        let gone = exe.path().join("does-not-exist");
        assert_eq!(default_library(Some(&gone), Some(home)), home.join(r"Documents\Book Reader"));
    }

    #[test]
    fn no_app_data_keeps_exe_dir() {
        let gone = Path::new(r"Z:\does\not\exist");
        assert_eq!(state_file(Some(gone), None), gone.join(FILE_NAME));
    }
}
