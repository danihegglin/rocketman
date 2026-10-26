//! The editing session shared by every frontend: the open workspace, which
//! collections are collapsed, unsaved drafts and the operations the UI offers
//! on collections and requests. Frontends own only presentation and the
//! request currently in the editor.

use crate::openapi::{self, Method, OpKey, RequestSpec, is_json_type};
use crate::workspace::{Workspace, config};
use anyhow::{Result, anyhow};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Body content types offered in the editor, as (label, media type).
pub const BODY_TYPES: [(&str, &str); 5] = [
    ("JSON", "application/json"),
    ("Text", "text/plain"),
    ("XML", "application/xml"),
    ("Form", "application/x-www-form-urlencoded"),
    ("HTML", "text/html"),
];

/// Short label for a content type: "None", a [`BODY_TYPES`] label, or the type itself.
pub fn content_type_label(ct: &str) -> String {
    if ct.is_empty() {
        return "None".into();
    }
    BODY_TYPES
        .iter()
        .find(|(_, v)| *v == ct)
        .map(|(l, _)| l.to_string())
        .unwrap_or_else(|| ct.to_string())
}

/// Highlighting language for a request or response body.
pub fn body_language(content_type: &str) -> &'static str {
    if is_json_type(content_type) { "json" } else { "" }
}

/// A request inside a collection: its file plus the operation key.
pub type DraftKey = (PathBuf, OpKey);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Collection {
        file: PathBuf,
        title: String,
        count: usize,
        collapsed: bool,
    },
    Request {
        file: PathBuf,
        key: OpKey,
        label: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Success,
    Warning,
    Error,
}

/// A message for the user, shown as a toast or status line.
#[derive(Clone, Debug)]
pub struct Notice {
    pub level: Level,
    pub title: Option<String>,
    pub text: String,
}

impl Notice {
    pub fn success(text: impl Into<String>) -> Self {
        Self { level: Level::Success, title: None, text: text.into() }
    }
    pub fn warning(text: impl Into<String>) -> Self {
        Self { level: Level::Warning, title: None, text: text.into() }
    }
    pub fn error(text: impl Into<String>) -> Self {
        Self { level: Level::Error, title: None, text: text.into() }
    }
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }
}

pub struct Session {
    pub workspace: Workspace,
    pub collapsed: HashSet<PathBuf>,
    /// Unsaved edits per request, kept while the user switches around.
    pub drafts: HashMap<DraftKey, RequestSpec>,
}

impl Session {
    pub fn open(dir: &Path) -> Session {
        Session {
            workspace: Workspace::load(dir),
            collapsed: HashSet::new(),
            drafts: HashMap::new(),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.workspace.dir
    }

    /// Collections that failed to load, as notices.
    pub fn load_errors(&self) -> Vec<Notice> {
        self.workspace
            .errors
            .iter()
            .map(|(file, err)| {
                let name = file.file_name().unwrap_or_default().to_string_lossy();
                Notice::error(format!("{name}: {err}")).title("Could not load collection")
            })
            .collect()
    }

    /// The sidebar: collections and, unless collapsed, their requests. A
    /// non-empty filter matches name, path or method and expands everything.
    pub fn rows(&self, filter: &str) -> Vec<Row> {
        let filter = filter.trim().to_lowercase();
        let mut rows = Vec::new();
        for c in &self.workspace.collections {
            let ops = c.operations();
            let matching: Vec<_> = ops
                .iter()
                .filter(|op| {
                    filter.is_empty()
                        || op.name.to_lowercase().contains(&filter)
                        || op.key.path.to_lowercase().contains(&filter)
                        || op.key.method.as_str().eq_ignore_ascii_case(&filter)
                })
                .collect();
            if !filter.is_empty() && matching.is_empty() {
                continue;
            }
            let collapsed = filter.is_empty() && self.collapsed.contains(&c.file);
            rows.push(Row::Collection {
                file: c.file.clone(),
                title: c.title(),
                count: ops.len(),
                collapsed,
            });
            if !collapsed {
                for op in matching {
                    let label = if op.name.is_empty() {
                        op.key.path.clone()
                    } else {
                        op.name.clone()
                    };
                    rows.push(Row::Request {
                        file: c.file.clone(),
                        key: op.key.clone(),
                        label,
                    });
                }
            }
        }
        rows
    }

    pub fn toggle_collapsed(&mut self, file: &Path) {
        if !self.collapsed.remove(file) {
            self.collapsed.insert(file.to_path_buf());
        }
    }

    /// The first request in sidebar order, to open on startup.
    pub fn first_request(&self) -> Option<DraftKey> {
        self.workspace.collections.iter().find_map(|c| {
            c.operations()
                .into_iter()
                .next()
                .map(|op| (c.file.clone(), op.key))
        })
    }

    /// The request's contents for the editor: its unsaved draft (taken out of
    /// the draft list, `true`) or what's stored in the collection (`false`).
    pub fn take_request(&mut self, target: &DraftKey) -> Option<(RequestSpec, bool)> {
        if let Some(draft) = self.drafts.remove(target) {
            return Some((draft, true));
        }
        self.workspace
            .find(&target.0)
            .and_then(|c| c.load_request(&target.1))
            .map(|spec| (spec, false))
    }

    /// The stored (saved) form of a request.
    pub fn stored_request(&self, file: &Path, key: &OpKey) -> Option<RequestSpec> {
        self.workspace.find(file).and_then(|c| c.load_request(key))
    }

    pub fn is_dirty(&self, target: &DraftKey) -> bool {
        self.drafts.contains_key(target)
    }

    /// Updates the document and writes it to disk; the in-memory document is
    /// rolled back if the write fails so memory and disk never diverge.
    pub fn save_request(&mut self, file: &Path, key: &OpKey, spec: &RequestSpec) -> Result<OpKey> {
        let coll = self
            .workspace
            .find_mut(file)
            .ok_or_else(|| anyhow!("collection no longer exists"))?;
        let backup = coll.doc.clone();
        let result = coll
            .save_request(Some(key), spec)
            .and_then(|new_key| coll.save().map(|_| new_key));
        if result.is_err() {
            coll.doc = backup;
        }
        result
    }

    /// Saves every draft. Drafts that fail stay in place; their errors are returned.
    pub fn save_drafts(&mut self) -> Vec<anyhow::Error> {
        let drafts: Vec<_> = self.drafts.drain().collect();
        let mut errors = Vec::new();
        for ((file, key), spec) in drafts {
            if let Err(e) = self.save_request(&file, &key, &spec) {
                errors.push(e);
                self.drafts.insert((file, key), spec);
            }
        }
        errors
    }

    /// Where "new request" goes when no collection is given: the active one or the first.
    pub fn default_collection(&self, active: Option<&DraftKey>) -> Option<PathBuf> {
        active
            .map(|(f, _)| f.clone())
            .or_else(|| self.workspace.collections.first().map(|c| c.file.clone()))
    }

    /// Adds a `GET /new-request` to the collection, using its server, and saves it.
    pub fn new_request(&mut self, file: &Path) -> Result<OpKey> {
        let coll = self
            .workspace
            .find_mut(file)
            .ok_or_else(|| anyhow!("collection no longer exists"))?;
        let key = coll.unique_key("/new-request", Method::Get);
        let server = coll
            .doc
            .pointer("/servers/0/url")
            .and_then(|v| v.as_str())
            .unwrap_or("http://localhost:8080")
            .trim_end_matches('/')
            .to_string();
        let spec = RequestSpec {
            name: "New request".into(),
            method: Method::Get,
            url: format!("{server}{}", key.path),
            ..Default::default()
        };
        let backup = coll.doc.clone();
        let result = coll
            .save_request(None, &spec)
            .and_then(|k| coll.save().map(|_| k));
        match result {
            Ok(key) => {
                self.collapsed.remove(file);
                Ok(key)
            }
            Err(e) => {
                coll.doc = backup;
                Err(e)
            }
        }
    }

    pub fn duplicate_request(&mut self, file: &Path, key: &OpKey) -> Result<OpKey> {
        let coll = self
            .workspace
            .find_mut(file)
            .ok_or_else(|| anyhow!("collection no longer exists"))?;
        let backup = coll.doc.clone();
        let result = coll
            .duplicate_request(key)
            .and_then(|k| coll.save().map(|_| k));
        if result.is_err() {
            coll.doc = backup;
        }
        result
    }

    /// Removes the operation from its file, along with any draft of it.
    pub fn delete_request(&mut self, file: &Path, key: &OpKey) -> Result<()> {
        let coll = self
            .workspace
            .find_mut(file)
            .ok_or_else(|| anyhow!("collection no longer exists"))?;
        let backup = coll.doc.clone();
        coll.delete_request(key);
        if let Err(e) = coll.save() {
            coll.doc = backup;
            return Err(e);
        }
        self.drafts.remove(&(file.to_path_buf(), key.clone()));
        Ok(())
    }

    pub fn create_collection(&mut self, title: &str) -> Result<PathBuf> {
        self.workspace.create_collection(title)
    }

    pub fn collection_title(&self, file: &Path) -> Option<String> {
        self.workspace.find(file).map(|c| c.title())
    }

    pub fn rename_collection(&mut self, file: &Path, title: &str) -> Result<()> {
        let coll = self
            .workspace
            .find_mut(file)
            .ok_or_else(|| anyhow!("collection no longer exists"))?;
        let backup = coll.doc.clone();
        coll.set_title(title);
        if let Err(e) = coll.save() {
            coll.doc = backup;
            return Err(e);
        }
        self.workspace
            .collections
            .sort_by_key(|c| c.title().to_lowercase());
        Ok(())
    }

    /// Deletes the collection's file and drops its drafts.
    pub fn delete_collection(&mut self, file: &Path) -> Result<()> {
        self.workspace.delete_collection(file)?;
        self.drafts.retain(|(f, _), _| f != file);
        self.collapsed.remove(file);
        Ok(())
    }

    /// Imports OpenAPI files as collections. Returns a notice per file and the
    /// first request of the first imported collection, to open.
    pub fn import_files(&mut self, paths: &[PathBuf]) -> (Vec<Notice>, Option<DraftKey>) {
        let mut notices = Vec::new();
        let mut first: Option<PathBuf> = None;
        for path in paths {
            match self.workspace.import(path) {
                Ok(imported) => {
                    let title = self.collection_title(&imported.file).unwrap_or_default();
                    notices.push(if imported.external_refs {
                        Notice::warning(format!(
                            "Imported \"{title}\", but it references other files that were not \
                             copied. Copy them next to {} for those $refs to resolve.",
                            imported.file.file_name().unwrap_or_default().to_string_lossy()
                        ))
                    } else {
                        Notice::success(format!("Imported \"{title}\""))
                    });
                    first.get_or_insert(imported.file);
                }
                Err(e) => notices.push(Notice::error(format!("{e:#}")).title("Import failed")),
            }
        }
        let open = first.and_then(|file| {
            self.collapsed.remove(&file);
            let key = self
                .workspace
                .find(&file)
                .and_then(|c| c.operations().into_iter().next())
                .map(|op| op.key)?;
            Some((file, key))
        });
        (notices, open)
    }

    /// Re-reads the folder. Drafts for requests that disappeared on disk are dropped.
    pub fn reload(&mut self) {
        self.workspace = Workspace::load(&self.workspace.dir.clone());
        let ws = &self.workspace;
        self.drafts
            .retain(|(f, k), _| ws.find(f).is_some_and(|c| c.contains(k)));
    }

    pub fn contains(&self, target: &DraftKey) -> bool {
        self.workspace
            .find(&target.0)
            .is_some_and(|c| c.contains(&target.1))
    }

    /// Opens another folder and remembers it for next time. Callers save or
    /// discard drafts first; any left are dropped.
    pub fn switch_workspace(&mut self, dir: &Path) {
        config::remember_dir(dir);
        *self = Session::open(dir);
    }
}

/// Resolves the folder to open: the command-line argument, the last used
/// folder, or the default one (seeded with an example on first run).
pub fn startup_dir(arg: Option<std::ffi::OsString>) -> Result<PathBuf> {
    let dir = arg
        .map(PathBuf::from)
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .or_else(config::last_dir)
        .unwrap_or_else(config::default_dir);
    std::fs::create_dir_all(&dir).map_err(|e| anyhow!("cannot use {}: {e}", dir.display()))?;
    let first_run = std::fs::read_dir(&dir).is_ok_and(|mut d| d.next().is_none());
    if first_run && dir == config::default_dir() {
        seed_example(&dir);
    }
    Ok(dir)
}

/// Gives a first-time user something to click.
fn seed_example(dir: &Path) {
    let mut c = openapi::Collection::new(dir.join("httpbin.yaml"), "httpbin");
    let examples = [
        ("Get anything", Method::Get, "https://httpbin.org/get?hello=world", "", ""),
        (
            "Post JSON",
            Method::Post,
            "https://httpbin.org/post",
            "application/json",
            "{\n  \"name\": \"rocketman\",\n  \"fast\": true\n}",
        ),
        ("Put", Method::Put, "https://httpbin.org/put", "text/plain", "hello"),
        ("Delete", Method::Delete, "https://httpbin.org/delete", "", ""),
        ("Status code", Method::Get, "https://httpbin.org/status/{code}", "", ""),
    ];
    for (name, method, url, ct, body) in examples {
        let mut spec = RequestSpec {
            name: name.into(),
            method,
            url: url.into(),
            content_type: ct.into(),
            body: body.into(),
            ..Default::default()
        };
        if url.contains("{code}") {
            spec.path_params = vec![openapi::Param::new("code", "418")];
        }
        if let Err(e) = c.save_request(None, &spec) {
            log::warn!("seeding example failed: {e:#}");
        }
    }
    if let Err(e) = c.save() {
        log::warn!("could not write example collection: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rocketman-session-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn request_lifecycle() {
        let dir = temp_dir("lifecycle");
        let mut s = Session::open(&dir);
        let file = s.create_collection("Pets").unwrap();
        let key = s.new_request(&file).unwrap();
        assert_eq!(key.path, "/new-request");
        let target = (file.clone(), key.clone());

        let (mut spec, dirty) = s.take_request(&target).unwrap();
        assert!(!dirty);
        spec.name = "List pets".into();
        s.drafts.insert(target.clone(), spec.clone());
        assert!(s.is_dirty(&target));
        assert_eq!(s.take_request(&target).unwrap(), (spec.clone(), true));
        s.drafts.insert(target.clone(), spec);
        assert!(s.save_drafts().is_empty());
        assert!(s.drafts.is_empty());
        assert_eq!(s.stored_request(&file, &key).unwrap().name, "List pets");

        let copy = s.duplicate_request(&file, &key).unwrap();
        assert_eq!(copy.path, "/new-request-copy");
        let labels: Vec<_> = s
            .rows("")
            .into_iter()
            .filter_map(|r| match r {
                Row::Request { label, .. } => Some(label),
                _ => None,
            })
            .collect();
        assert_eq!(labels, ["List pets", "List pets (copy)"]);
        assert_eq!(s.rows("copy").len(), 2);
        assert!(s.rows("nothing matches").is_empty());

        s.toggle_collapsed(&file);
        assert_eq!(s.rows("").len(), 1);

        s.delete_request(&file, &copy).unwrap();
        s.rename_collection(&file, "Animals").unwrap();
        s.reload();
        assert_eq!(s.collection_title(&file).unwrap(), "Animals");
        assert_eq!(s.workspace.find(&file).unwrap().operations().len(), 1);

        s.delete_collection(&file).unwrap();
        assert!(s.rows("").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn labels_content_types() {
        assert_eq!(content_type_label(""), "None");
        assert_eq!(content_type_label("application/json"), "JSON");
        assert_eq!(content_type_label("image/png"), "image/png");
    }
}
