use std::path::{Path, PathBuf};

use crate::domain::error::UpdaterError;

pub struct UpdateCache {
    directory: PathBuf,
}

impl UpdateCache {
    pub fn at(directory: impl AsRef<Path>) -> Self {
        Self {
            directory: directory.as_ref().to_path_buf(),
        }
    }

    pub fn remove_stale_files(&self) -> Result<(), UpdaterError> {
        let entries = match std::fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(UpdaterError::ReadCacheDirectory {
                    path: self.directory.display().to_string(),
                    source,
                });
            }
        };

        let mut first_error = None;

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(source) => {
                    if first_error.is_none() {
                        first_error = Some(UpdaterError::ReadCacheEntry {
                            path: self.directory.display().to_string(),
                            source,
                        });
                    }
                    continue;
                }
            };
            let file = entry.path();
            if !file.is_file() {
                continue;
            }

            match std::fs::remove_file(&file) {
                Ok(()) => tracing::debug!("Cleaned up stale update file: {}", file.display()),
                Err(source) => {
                    if first_error.is_none() {
                        first_error = Some(UpdaterError::RemoveStaleUpdate {
                            path: file.display().to_string(),
                            source,
                        });
                    }
                }
            }
        }

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}
