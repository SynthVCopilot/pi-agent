use crate::error::{ComponentError, Result};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ComponentPaths {
    pub data_root: PathBuf,
    pub component_root: PathBuf,
    pub downloads_dir: PathBuf,
    pub output_dir: PathBuf,
}

impl ComponentPaths {
    pub fn from_environment() -> Self {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        Self::from_data_root(home.join(".SynthVcopilot"))
    }

    pub fn from_data_root(data_root: PathBuf) -> Self {
        Self {
            component_root: data_root.join("components").join("ffmpeg"),
            downloads_dir: data_root.join("downloads").join("ffmpeg"),
            output_dir: data_root.join("output").join("ffmpeg"),
            data_root,
        }
    }

    pub fn current_pointer(&self) -> PathBuf {
        self.component_root.join("current.json")
    }
    pub fn staging_root(&self) -> PathBuf {
        self.component_root.join(".staging")
    }

    pub fn validate_component_root(&self) -> Result<()> {
        ensure_inside(&self.data_root, &self.component_root)
    }

    pub fn validate_component_path(&self, candidate: &Path) -> Result<()> {
        self.validate_component_root()?;
        ensure_inside(&self.component_root, candidate)
    }

    pub fn validate_downloads_dir(&self) -> Result<()> {
        ensure_inside(&self.data_root, &self.downloads_dir)
    }
}

pub fn safe_relative_path(value: &Path) -> Result<PathBuf> {
    if value.as_os_str().is_empty() {
        return Err(ComponentError::InvalidInput("empty relative path".into()));
    }
    let mut out = PathBuf::new();
    for component in value.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ComponentError::InvalidInput(format!(
                    "unsafe relative path: {}",
                    value.display()
                )));
            }
        }
    }
    Ok(out)
}

pub fn ensure_inside(root: &Path, candidate: &Path) -> Result<()> {
    let root = canonicalize_with_missing_tail(root)?;
    let candidate = canonicalize_with_missing_tail(candidate)?;
    if candidate.starts_with(&root) {
        Ok(())
    } else {
        Err(ComponentError::InvalidInput(format!(
            "path escapes managed root: {}",
            candidate.display()
        )))
    }
}

fn canonicalize_with_missing_tail(path: &Path) -> Result<PathBuf> {
    if let Ok(canonical) = path.canonicalize() {
        return Ok(canonical);
    }
    let mut cursor = path;
    let mut tail = Vec::new();
    while match std::fs::symlink_metadata(cursor) {
        Ok(_) => false,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => return Err(ComponentError::Io(error)),
    } {
        let name = cursor.file_name().ok_or_else(|| {
            ComponentError::InvalidInput(format!("cannot normalize path: {}", path.display()))
        })?;
        tail.push(name.to_os_string());
        cursor = cursor.parent().ok_or_else(|| {
            ComponentError::InvalidInput(format!("cannot normalize path: {}", path.display()))
        })?;
    }
    let mut canonical = cursor.canonicalize()?;
    for name in tail.into_iter().rev() {
        canonical.push(name);
    }
    Ok(canonical)
}

pub fn validate_local_file(path: &Path) -> Result<PathBuf> {
    let text = path.to_string_lossy();
    let lower = text.to_ascii_lowercase();
    if lower.starts_with("http:") || lower.starts_with("https:") || lower.contains("://") {
        return Err(ComponentError::InvalidInput(
            "URLs are not accepted as audio input".into(),
        ));
    }
    if is_network_or_device_path(path) {
        return Err(ComponentError::InvalidInput(
            "network and device paths are not accepted as audio input".into(),
        ));
    }
    if !path.is_absolute() {
        return Err(ComponentError::InvalidInput(
            "audio input must be an absolute local path".into(),
        ));
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| ComponentError::NotFound(path.display().to_string()))?;
    if is_network_or_device_path(&canonical) {
        return Err(ComponentError::InvalidInput(
            "network and device paths are not accepted as audio input".into(),
        ));
    }
    if !canonical.is_file() {
        return Err(ComponentError::InvalidInput(
            "audio input must be a regular file".into(),
        ));
    }
    Ok(canonical)
}

fn is_network_or_device_path(path: &Path) -> bool {
    let lower = path.to_string_lossy().to_ascii_lowercase();
    let extended_drive = lower.starts_with(r"\\?\")
        && lower.as_bytes().get(4).is_some_and(u8::is_ascii_alphabetic)
        && lower.as_bytes().get(5) == Some(&b':')
        && matches!(lower.as_bytes().get(6), Some(b'\\' | b'/'));
    lower.starts_with(r"\\.\")
        || lower.starts_with(r"\\?\unc\")
        || (lower.starts_with(r"\\") && !extended_drive)
        || lower.starts_with("/dev/")
}

pub fn safe_output_name(name: &str, extension: &str) -> Result<String> {
    let path = Path::new(name);
    let has_windows_metacharacter = name
        .chars()
        .any(|character| character < ' ' || r#"<>:"/\|?*"#.contains(character));
    if path.components().count() != 1
        || path.file_name().is_none()
        || name.trim().is_empty()
        || has_windows_metacharacter
    {
        return Err(ComponentError::InvalidInput(
            "output name must be a plain file name".into(),
        ));
    }
    if path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case(extension))
        != Some(true)
    {
        return Err(ComponentError::InvalidInput(format!(
            "output name must end in .{extension}"
        )));
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relative_paths_reject_escape() {
        assert!(safe_relative_path(Path::new("bin/ffmpeg.exe")).is_ok());
        assert!(safe_relative_path(Path::new("../ffmpeg.exe")).is_err());
        assert!(safe_relative_path(Path::new("C:\\Windows\\x")).is_err());
    }
    #[test]
    fn output_name_has_no_directory() {
        assert!(safe_output_name("voice.wav", "wav").is_ok());
        assert!(safe_output_name("a/voice.wav", "wav").is_err());
        assert!(safe_output_name("voice.mp3", "wav").is_err());
        assert!(safe_output_name("voice.wav:alternate.wav", "wav").is_err());
    }
    #[test]
    fn unc_network_inputs_are_rejected() {
        assert!(validate_local_file(Path::new(r"\\server\share\audio.wav")).is_err());
        assert!(is_network_or_device_path(Path::new(
            r"\\?\UNC\server\share\audio.wav"
        )));
    }
    #[test]
    fn containment_normalizes_nonexistent_children() {
        let root = std::env::temp_dir().join(format!("pi-path-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert!(ensure_inside(&root, &root.join("future").join("file.wav")).is_ok());
        assert!(ensure_inside(&root, &root.with_file_name("outside").join("file.wav")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
