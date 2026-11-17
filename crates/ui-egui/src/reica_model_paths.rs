//! Portable model storage for RE:ICA image AI tools.
//! Both SkyTNT and Manga OCR use the directory NEXT TO photocraft-reica.exe,
//! not the current working directory, the selected Python interpreter's folder,
//! or the user's global Hugging Face cache.
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};

#[cfg(not(target_arch = "wasm32"))]
fn next_to_executable(executable: &Path) -> Result<PathBuf, String> {
    let parent = executable.parent().filter(|p| !p.as_os_str().is_empty()).ok_or_else(|| "Could not resolve PhotoCraft executable folder".to_owned())?;
    Ok(parent.join("models"))
}

/// Resolve the portable models folder. Calling this does not download or install anything.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn root() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|err| format!("Cannot locate PhotoCraft executable: {err}"))?;
    next_to_executable(&executable)
}

/// Create the requested local model/cache directory on demand, without silently
/// falling back to %APPDATA% or another drive when the portable folder is read-only.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn prepare(name: &str) -> Result<PathBuf, String> {
    if !matches!(name, "skytnt" | "manga-ocr") {
        return Err("Unknown RE:ICA model subdirectory".into());
    }
    let path = root()?.join(name);
    std::fs::create_dir_all(&path).map_err(|err| {
        format!("Cannot write models next to PhotoCraft: {} ({err}). Move the portable app to a writable folder outside Program Files.", path.display())
    })?;
    Ok(path)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn models_are_anchored_to_exe_not_python_or_current_directory() {
        let root = next_to_executable(Path::new("portable/photocraft-reica.exe")).unwrap();
        assert_eq!(root, PathBuf::from("portable").join("models"));
        assert_eq!(root.join("skytnt"), PathBuf::from("portable/models/skytnt"));
        assert_eq!(root.join("manga-ocr"), PathBuf::from("portable/models/manga-ocr"));
    }

    #[test]
    fn invalid_executable_path_and_arbitrary_subfolder_are_rejected() {
        assert!(next_to_executable(Path::new("photocraft-reica.exe")).is_err());
        assert!(prepare("../../escape").is_err());
    }
}
