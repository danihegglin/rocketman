//! A workspace is a folder; every OpenAPI 3.x file in it is a collection.

use crate::openapi::Collection;
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

pub struct Workspace {
    pub dir: PathBuf,
    pub collections: Vec<Collection>,
    /// Files that look like OpenAPI documents but failed to load.
    pub errors: Vec<(PathBuf, String)>,
}

impl Workspace {
    pub fn load(dir: &Path) -> Workspace {
        let mut ws = Workspace {
            dir: dir.to_path_buf(),
            collections: Vec::new(),
            errors: Vec::new(),
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return ws;
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && crate::openapi::Format::from_path(p).is_some()
                    && !p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with('.'))
            })
            .collect();
        files.sort();
        for file in files {
            match Collection::load(&file) {
                Ok(c) => ws.collections.push(c),
                Err(e) => {
                    // Unrelated YAML/JSON files (package.json, CI configs) are ignored.
                    let mentions_openapi = std::fs::read_to_string(&file)
                        .map(|t| t.contains("openapi"))
                        .unwrap_or(false);
                    if mentions_openapi {
                        ws.errors.push((file, format!("{e:#}")));
                    }
                }
            }
        }
        ws.collections
            .sort_by_key(|c| c.title().to_lowercase());
        ws
    }

    pub fn find(&self, file: &Path) -> Option<&Collection> {
        self.collections.iter().find(|c| c.file == file)
    }

    pub fn find_mut(&mut self, file: &Path) -> Option<&mut Collection> {
        self.collections.iter_mut().find(|c| c.file == file)
    }

    pub fn create_collection(&mut self, title: &str) -> Result<PathBuf> {
        let title = title.trim();
        if title.is_empty() {
            bail!("Name must not be empty");
        }
        let slug = slugify(title);
        let mut file = self.dir.join(format!("{slug}.yaml"));
        let mut n = 2;
        while file.exists() {
            file = self.dir.join(format!("{slug}-{n}.yaml"));
            n += 1;
        }
        let c = Collection::new(file.clone(), title);
        c.save()?;
        self.collections.push(c);
        self.collections.sort_by_key(|c| c.title().to_lowercase());
        Ok(file)
    }

    pub fn delete_collection(&mut self, file: &Path) -> Result<()> {
        std::fs::remove_file(file)?;
        self.collections.retain(|c| c.file != file);
        Ok(())
    }
}

fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "collection".into()
    } else {
        out
    }
}

/// Persistent app settings (just the last opened folder).
pub mod config {
    use std::path::PathBuf;

    fn config_file() -> Option<PathBuf> {
        Some(dirs::config_dir()?.join("rocketman").join("config.json"))
    }

    pub fn default_dir() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("rocketman")
            .join("collections")
    }

    pub fn last_dir() -> Option<PathBuf> {
        let text = std::fs::read_to_string(config_file()?).ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        let dir = PathBuf::from(v.get("workspace")?.as_str()?);
        dir.is_dir().then_some(dir)
    }

    pub fn remember_dir(dir: &std::path::Path) {
        let Some(file) = config_file() else { return };
        let text = serde_json::json!({ "workspace": dir.to_string_lossy() }).to_string();
        if let Some(parent) = file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&file, text) {
            log::warn!("could not write {}: {e}", file.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slugify("My API — v2!"), "my-api-v2");
        assert_eq!(slugify("***"), "collection");
    }
}
