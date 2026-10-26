//! A workspace is a folder; every OpenAPI 3.x file in it is a collection.

use crate::openapi::{Collection, Format};
use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::Value;
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
                    && Format::from_path(p).is_some()
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

    /// Copies an OpenAPI file into the workspace as a new collection. The file
    /// is copied byte-for-byte (comments and formatting survive until the
    /// first save) and validated before anything is written.
    pub fn import(&mut self, src: &Path) -> Result<Imported> {
        let format = Format::from_path(src)
            .ok_or_else(|| anyhow!("{}: not a .yaml, .yml or .json file", display_name(src)))?;
        let text = std::fs::read_to_string(src)
            .with_context(|| format!("reading {}", src.display()))?;
        let parsed = Collection::parse(src.to_path_buf(), format, &text)
            .with_context(|| display_name(src))?;

        let src_dir = src.parent().and_then(|d| d.canonicalize().ok());
        if src_dir.is_some() && src_dir == self.dir.canonicalize().ok() {
            bail!("{} is already in this folder", display_name(src));
        }

        let stem = src
            .file_stem()
            .map(|s| slugify(&s.to_string_lossy()))
            .unwrap_or_else(|| slugify(&parsed.title()));
        let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("yaml");
        let mut file = self.dir.join(format!("{stem}.{ext}"));
        let mut n = 2;
        while file.exists() {
            file = self.dir.join(format!("{stem}-{n}.{ext}"));
            n += 1;
        }
        std::fs::create_dir_all(&self.dir)?;
        // Write via temp + rename so a failure never leaves a half-written collection.
        let tmp = self.dir.join(format!(".{stem}.import.tmp"));
        std::fs::write(&tmp, &text).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &file).with_context(|| format!("creating {}", file.display()))?;

        let external_refs = has_external_refs(&parsed.doc);
        let collection = Collection {
            file: file.clone(),
            ..parsed
        };
        self.collections.push(collection);
        self.collections.sort_by_key(|c| c.title().to_lowercase());
        Ok(Imported {
            file,
            external_refs,
        })
    }

    pub fn delete_collection(&mut self, file: &Path) -> Result<()> {
        std::fs::remove_file(file)?;
        self.collections.retain(|c| c.file != file);
        Ok(())
    }
}

#[derive(Debug)]
pub struct Imported {
    pub file: PathBuf,
    /// The document references other files (`other.yaml#/...`) that were not copied.
    pub external_refs: bool,
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn has_external_refs(v: &Value) -> bool {
    match v {
        Value::Object(map) => map.iter().any(|(k, v)| {
            (k == "$ref" && v.as_str().is_some_and(|r| !r.starts_with('#')))
                || has_external_refs(v)
        }),
        Value::Array(items) => items.iter().any(has_external_refs),
        _ => false,
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

#[cfg(test)]
mod import_tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rocketman-test-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const SPEC: &str = "# keep this comment\nopenapi: 3.0.3\ninfo: {title: Pets, version: '1'}\npaths:\n  /pets:\n    get:\n      responses:\n        '200': {$ref: 'common.yaml#/ok'}\n";

    #[test]
    fn imports_verbatim_with_unique_names() {
        let src_dir = temp_dir("src");
        let ws_dir = temp_dir("ws");
        let src = src_dir.join("Pet Store.yaml");
        std::fs::write(&src, SPEC).unwrap();

        let mut ws = Workspace::load(&ws_dir);
        let first = ws.import(&src).unwrap();
        assert_eq!(first.file, ws_dir.join("pet-store.yaml"));
        assert!(first.external_refs);
        assert_eq!(std::fs::read_to_string(&first.file).unwrap(), SPEC);

        let second = ws.import(&src).unwrap();
        assert_eq!(second.file, ws_dir.join("pet-store-2.yaml"));
        assert_eq!(ws.collections.len(), 2);
        assert_eq!(ws.find(&first.file).unwrap().operations().len(), 1);

        // Survives a reload from disk.
        assert_eq!(Workspace::load(&ws_dir).collections.len(), 2);

        // Importing a file that already lives in the workspace is refused.
        assert!(ws.import(&first.file).is_err());
        let _ = std::fs::remove_dir_all(&src_dir);
        let _ = std::fs::remove_dir_all(&ws_dir);
    }

    #[test]
    fn rejects_non_openapi() {
        let src_dir = temp_dir("bad");
        let ws_dir = temp_dir("ws-bad");
        let swagger = src_dir.join("old.json");
        std::fs::write(&swagger, r#"{"swagger":"2.0","info":{},"paths":{}}"#).unwrap();
        let txt = src_dir.join("notes.txt");
        std::fs::write(&txt, "hi").unwrap();

        let mut ws = Workspace::load(&ws_dir);
        let err = format!("{:#}", ws.import(&swagger).unwrap_err());
        assert!(err.contains("Swagger 2.0"), "{err}");
        assert!(ws.import(&txt).is_err());
        assert!(ws.collections.is_empty());
        assert_eq!(std::fs::read_dir(&ws_dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&src_dir);
        let _ = std::fs::remove_dir_all(&ws_dir);
    }
}
