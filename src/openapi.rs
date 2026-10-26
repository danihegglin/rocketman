//! Lossless mapping between OpenAPI documents and editable requests.
//!
//! A collection is an OpenAPI 3.x document kept as a `serde_json::Value` (with
//! insertion order preserved). We only ever touch the fields we manage — servers,
//! summary, parameters' examples and the request body example — so everything
//! else a hand-written spec contains (schemas, responses, descriptions, tags,
//! extensions, ...) survives a round trip untouched.

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Map, Value, json};
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
    Trace,
}

impl Method {
    /// Methods offered in the method picker.
    pub const PICKER: [Method; 5] = [
        Method::Get,
        Method::Post,
        Method::Put,
        Method::Patch,
        Method::Delete,
    ];
    /// Every method OpenAPI allows as a path item key, in spec order.
    pub const ALL: [Method; 8] = [
        Method::Get,
        Method::Put,
        Method::Post,
        Method::Delete,
        Method::Options,
        Method::Head,
        Method::Patch,
        Method::Trace,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
            Method::Head => "HEAD",
            Method::Options => "OPTIONS",
            Method::Trace => "TRACE",
        }
    }

    /// The lowercase key used inside an OpenAPI path item.
    pub fn key(self) -> &'static str {
        match self {
            Method::Get => "get",
            Method::Post => "post",
            Method::Put => "put",
            Method::Patch => "patch",
            Method::Delete => "delete",
            Method::Head => "head",
            Method::Options => "options",
            Method::Trace => "trace",
        }
    }

    pub fn from_key(key: &str) -> Option<Method> {
        Method::ALL.into_iter().find(|m| m.key() == key)
    }

    pub fn has_body(self) -> bool {
        !matches!(self, Method::Get | Method::Head | Method::Options | Method::Trace)
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub value: String,
}

impl Param {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// Everything the editor shows for one request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RequestSpec {
    pub name: String,
    pub method: Method,
    /// Absolute (or server-relative) URL without query string. May contain `{var}`
    /// path templates.
    pub url: String,
    pub path_params: Vec<Param>,
    pub query: Vec<Param>,
    pub headers: Vec<Param>,
    /// Media type of the body; empty means "no body".
    pub content_type: String,
    pub body: String,
}

impl Default for Method {
    fn default() -> Self {
        Method::Get
    }
}

impl RequestSpec {
    /// Moves a query string typed into the URL into the query parameter list, so
    /// the URL field and the storage never disagree about where a parameter lives.
    pub fn normalized(&self) -> RequestSpec {
        let mut spec = self.clone();
        spec.url = spec.url.trim().to_string();
        if let Some(hash) = spec.url.find('#') {
            spec.url.truncate(hash);
        }
        if let Some(q) = spec.url.find('?') {
            let query = spec.url[q + 1..].to_string();
            spec.url.truncate(q);
            let mut parsed: Vec<Param> = query
                .split('&')
                .filter(|s| !s.is_empty())
                .map(|pair| match pair.split_once('=') {
                    Some((k, v)) => Param::new(percent_decode(k), percent_decode(v)),
                    None => Param::new(percent_decode(pair), ""),
                })
                .collect();
            parsed.append(&mut spec.query);
            spec.query = parsed;
        }
        spec.query.retain(|p| !p.name.trim().is_empty());
        spec.headers.retain(|p| !p.name.trim().is_empty());
        // Path params are exactly the `{templates}` in the URL, keeping known values.
        let old = std::mem::take(&mut spec.path_params);
        spec.path_params = path_templates(&spec.url)
            .into_iter()
            .map(|name| {
                let value = old
                    .iter()
                    .find(|p| p.name == name)
                    .map(|p| p.value.clone())
                    .unwrap_or_default();
                Param::new(name, value)
            })
            .collect();
        spec
    }
}

/// Names of the `{templates}` in a URL path, in order, without duplicates.
pub fn path_templates(url: &str) -> Vec<String> {
    let path = match url.find("://") {
        Some(i) => match url[i + 3..].find('/') {
            Some(j) => &url[i + 3 + j..],
            None => "",
        },
        None => url,
    };
    let mut out: Vec<String> = Vec::new();
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else {
            break;
        };
        let name = rest[open + 1..open + close].trim();
        if !name.is_empty() && !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
        rest = &rest[open + close + 1..];
    }
    out
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = bytes
                    .get(i + 1..i + 3)
                    .and_then(|h| std::str::from_utf8(h).ok())
                    .and_then(|h| u8::from_str_radix(h, 16).ok());
                match hex {
                    Some(b) => {
                        out.push(b);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Yaml,
    Json,
}

impl Format {
    pub fn from_path(path: &Path) -> Option<Format> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "yaml" | "yml" => Some(Format::Yaml),
            "json" => Some(Format::Json),
            _ => None,
        }
    }
}

/// Identifies an operation inside a document.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OpKey {
    pub path: String,
    pub method: Method,
}

#[derive(Clone, Debug)]
pub struct OpSummary {
    pub key: OpKey,
    pub name: String,
}

pub struct Collection {
    pub file: PathBuf,
    pub format: Format,
    pub doc: Value,
}

impl Collection {
    pub fn new(file: PathBuf, title: &str) -> Collection {
        let format = Format::from_path(&file).unwrap_or(Format::Yaml);
        Collection {
            file,
            format,
            doc: json!({
                "openapi": "3.1.0",
                "info": { "title": title, "version": "1.0.0" },
                "paths": {},
            }),
        }
    }

    pub fn load(file: &Path) -> Result<Collection> {
        let format = Format::from_path(file).ok_or_else(|| anyhow!("not a .yaml/.json file"))?;
        let text = std::fs::read_to_string(file)
            .with_context(|| format!("reading {}", file.display()))?;
        Self::parse(file.to_path_buf(), format, &text)
    }

    pub fn parse(file: PathBuf, format: Format, text: &str) -> Result<Collection> {
        let doc: Value = match format {
            Format::Yaml => serde_yaml_ng::from_str(text)?,
            Format::Json => serde_json::from_str(text)?,
        };
        let version = doc.get("openapi").and_then(Value::as_str).unwrap_or("");
        if !version.starts_with("3.") {
            bail!("not an OpenAPI 3.x document");
        }
        Ok(Collection { file, format, doc })
    }

    pub fn serialize(&self) -> Result<String> {
        Ok(match self.format {
            Format::Yaml => serde_yaml_ng::to_string(&self.doc)?,
            Format::Json => {
                let mut s = serde_json::to_string_pretty(&self.doc)?;
                s.push('\n');
                s
            }
        })
    }

    /// Writes atomically: a crash mid-write never leaves a truncated collection.
    pub fn save(&self) -> Result<()> {
        let text = self.serialize()?;
        let dir = self.file.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let name = self
            .file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("collection");
        let tmp = dir.join(format!(".{name}.tmp"));
        std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.file)
            .with_context(|| format!("replacing {}", self.file.display()))?;
        Ok(())
    }

    pub fn title(&self) -> String {
        self.doc
            .pointer("/info/title")
            .and_then(Value::as_str)
            .filter(|t| !t.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                self.file
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
    }

    pub fn set_title(&mut self, title: &str) {
        let info = obj_entry(root_obj(&mut self.doc), "info");
        info.insert("title".into(), Value::String(title.to_string()));
        if !info.contains_key("version") {
            info.insert("version".into(), Value::String("1.0.0".into()));
        }
    }

    pub fn operations(&self) -> Vec<OpSummary> {
        let Some(paths) = self.doc.get("paths").and_then(Value::as_object) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (path, item) in paths {
            let Some(item) = item.as_object() else { continue };
            for (key, op) in item {
                let Some(method) = Method::from_key(key) else { continue };
                let name = op_name(op);
                out.push(OpSummary {
                    key: OpKey {
                        path: path.clone(),
                        method,
                    },
                    name,
                });
            }
        }
        out
    }

    pub fn contains(&self, key: &OpKey) -> bool {
        self.op(key).is_some()
    }

    fn op(&self, key: &OpKey) -> Option<&Value> {
        self.doc
            .get("paths")?
            .get(&key.path)?
            .get(key.method.key())
    }

    fn resolve<'a>(&'a self, v: &'a Value) -> &'a Value {
        // Follow local `$ref`s (bounded, so a cyclic reference cannot hang us).
        let mut v = v;
        for _ in 0..16 {
            match v.get("$ref").and_then(Value::as_str) {
                Some(r) if r.starts_with("#/") => match self.doc.pointer(&r[1..]) {
                    Some(target) => v = target,
                    None => break,
                },
                _ => break,
            }
        }
        v
    }

    fn servers_of(&self, v: Option<&Value>) -> Vec<String> {
        v.and_then(|v| v.get("servers"))
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(server_url).collect())
            .unwrap_or_default()
    }

    /// Server URL in effect for an operation (operation > path item > document).
    fn effective_server(&self, key: &OpKey) -> String {
        let item = self.doc.get("paths").and_then(|p| p.get(&key.path));
        let op = item.and_then(|i| i.get(key.method.key()));
        for level in [op, item, Some(&self.doc)] {
            if let Some(s) = self.servers_of(level).into_iter().next() {
                return s;
            }
        }
        String::new()
    }

    pub fn load_request(&self, key: &OpKey) -> Option<RequestSpec> {
        let op = self.op(key)?;
        let item = self.doc.get("paths")?.get(&key.path)?;
        let server = self.effective_server(key);
        let mut spec = RequestSpec {
            name: op_name(op),
            method: key.method,
            url: join_url(&server, &key.path),
            ..Default::default()
        };

        // Path-level parameters apply unless the operation overrides them.
        let mut params: Vec<&Value> = Vec::new();
        for list in [item.get("parameters"), op.get("parameters")] {
            for p in list.and_then(Value::as_array).into_iter().flatten() {
                let p = self.resolve(p);
                let (name, loc) = param_id(p);
                params.retain(|q| param_id(q) != (name, loc));
                params.push(p);
            }
        }
        for p in params {
            let (name, loc) = param_id(p);
            let param = Param::new(name, self.example_text(p));
            match loc {
                "path" => spec.path_params.push(param),
                "query" => spec.query.push(param),
                "header" => spec.headers.push(param),
                _ => {}
            }
        }

        if let Some(body) = op.get("requestBody").map(|b| self.resolve(b)) {
            if let Some(content) = body.get("content").and_then(Value::as_object) {
                // Prefer the media type that actually carries an example.
                let pick = content
                    .iter()
                    .find(|(_, m)| m.get("example").is_some() || m.get("examples").is_some())
                    .or_else(|| content.iter().next());
                if let Some((ct, media)) = pick {
                    spec.content_type = ct.clone();
                    spec.body = self.example_text(media);
                }
            }
        }
        Some(spec.normalized())
    }

    /// The example value of a parameter or media type object, as editable text.
    fn example_text(&self, v: &Value) -> String {
        let example = v
            .get("example")
            .or_else(|| {
                let first = v.get("examples")?.as_object()?.values().next()?;
                self.resolve(first).get("value")
            })
            .or_else(|| self.resolve(v.get("schema")?).get("example"))
            .or_else(|| self.resolve(v.get("schema")?).get("default"));
        match example {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(s)) => s.clone(),
            Some(v @ (Value::Object(_) | Value::Array(_))) => {
                serde_json::to_string_pretty(v).unwrap_or_default()
            }
            Some(v) => v.to_string(),
        }
    }

    /// Persists `spec`, either as a new operation (`original == None`) or by
    /// updating/moving the operation at `original`. Returns the operation's key.
    pub fn save_request(&mut self, original: Option<&OpKey>, spec: &RequestSpec) -> Result<OpKey> {
        let spec = spec.normalized();
        let (server, mut path) = self.split_url(original, &spec.url);
        if !path.starts_with('/') {
            path.insert(0, '/');
        }
        let key = OpKey {
            path,
            method: spec.method,
        };
        if original != Some(&key) && self.contains(&key) {
            bail!(
                "{} {} already exists in this collection",
                key.method,
                key.path
            );
        }

        // Where the original path item sits, so a moved request keeps its place
        // (and diffs of the file stay small).
        let original_index = original.and_then(|orig| {
            let paths = self.doc.get("paths")?.as_object()?;
            paths.keys().position(|p| *p == orig.path)
        });
        let mut op = match original {
            Some(orig) => self.take_op(orig).unwrap_or_else(|| Value::Object(Map::new())),
            None => Value::Object(Map::new()),
        };
        let original_index = original_index.filter(|_| {
            let orig = original.expect("index implies original");
            self.doc.get("paths").and_then(|p| p.get(&orig.path)).is_none()
        });
        let resolved_params = self.resolved_op_params(&op);
        let resolved_body = op.get("requestBody").map(|b| self.resolve(b).clone());
        let is_30 = self
            .doc
            .get("openapi")
            .and_then(Value::as_str)
            .is_some_and(|v| v.starts_with("3.0"));
        let path_level = self.path_level_params(&key.path);

        let obj = ensure_obj(&mut op);
        if spec.name.trim().is_empty() {
            obj.shift_remove("summary");
        } else {
            obj.insert("summary".into(), Value::String(spec.name.trim().to_string()));
        }
        write_params(obj, &resolved_params, &path_level, &spec);
        write_body(obj, resolved_body, &spec);
        if is_30 && !obj.contains_key("responses") {
            obj.insert(
                "responses".into(),
                json!({ "default": { "description": "Response" } }),
            );
        }
        // Clear any operation-level server; it is re-added below only if needed.
        obj.shift_remove("servers");

        let paths = obj_entry(root_obj(&mut self.doc), "paths");
        if let Some(ix) = original_index.filter(|_| !paths.contains_key(&key.path)) {
            let ix = ix.min(paths.len());
            paths.shift_insert(ix, key.path.clone(), Value::Object(Map::new()));
        }
        let item = obj_entry(paths, &key.path);
        item.insert(key.method.key().to_string(), op);

        if !server.is_empty() && self.effective_server(&key) != server {
            let doc_has_servers = !self.servers_of(Some(&self.doc)).is_empty();
            if !doc_has_servers {
                // Conventional position: right before `paths`.
                let root = root_obj(&mut self.doc);
                let ix = root.keys().position(|k| k == "paths").unwrap_or(root.len());
                root.shift_insert(ix, "servers".into(), json!([{ "url": server }]));
            }
            if self.effective_server(&key) != server {
                if let Some(op) = self.op_mut(&key) {
                    ensure_obj(op).insert("servers".into(), json!([{ "url": server }]));
                }
            }
        }
        Ok(key)
    }

    /// Splits a URL into (server, path). Prefers servers the document already
    /// knows so requests stay portable across environments.
    fn split_url(&self, original: Option<&OpKey>, url: &str) -> (String, String) {
        let mut candidates = Vec::new();
        if let Some(orig) = original {
            candidates.push(self.effective_server(orig));
        }
        candidates.extend(self.servers_of(Some(&self.doc)));
        let mut best: Option<String> = None;
        for c in candidates {
            let c = c.trim_end_matches('/').to_string();
            if c.is_empty() || !url.starts_with(&c) {
                continue;
            }
            let rest = &url[c.len()..];
            if (rest.is_empty() || rest.starts_with('/'))
                && best.as_ref().is_none_or(|b| c.len() > b.len())
            {
                best = Some(c);
            }
        }
        if let Some(server) = best {
            let path = url[server.len()..].to_string();
            return (server, path);
        }
        if let Some(scheme_end) = url.find("://") {
            let after = &url[scheme_end + 3..];
            let split = after.find('/').map(|i| scheme_end + 3 + i).unwrap_or(url.len());
            return (url[..split].to_string(), url[split..].to_string());
        }
        (String::new(), url.to_string())
    }

    fn op_mut(&mut self, key: &OpKey) -> Option<&mut Value> {
        self.doc
            .get_mut("paths")?
            .get_mut(&key.path)?
            .get_mut(key.method.key())
    }

    fn take_op(&mut self, key: &OpKey) -> Option<Value> {
        let paths = self.doc.get_mut("paths")?.as_object_mut()?;
        let item = paths.get_mut(&key.path)?.as_object_mut()?;
        let op = item.shift_remove(key.method.key())?;
        if !item.keys().any(|k| Method::from_key(k).is_some()) {
            paths.shift_remove(&key.path);
        }
        Some(op)
    }

    /// Removes the operation; on a move this is where the old one goes away.
    pub fn delete_request(&mut self, key: &OpKey) -> bool {
        self.take_op(key).is_some()
    }

    /// Returns a key for a fresh, unused operation (`/new-request`, `/new-request-2`, ...).
    pub fn unique_key(&self, base: &str, method: Method) -> OpKey {
        let mut n = 1;
        loop {
            let path = if n == 1 {
                base.to_string()
            } else {
                format!("{base}-{n}")
            };
            let key = OpKey { path, method };
            if !self.contains(&key) {
                return key;
            }
            n += 1;
        }
    }

    pub fn duplicate_request(&mut self, key: &OpKey) -> Result<OpKey> {
        let mut spec = self
            .load_request(key)
            .ok_or_else(|| anyhow!("request not found"))?;
        let server = self.effective_server(key);
        let new_key = self.unique_key(&format!("{}-copy", key.path.trim_end_matches('/')), key.method);
        spec.url = join_url(&server, &new_key.path);
        spec.path_params.clear();
        if !spec.name.is_empty() {
            spec.name.push_str(" (copy)");
        }
        // Keep the original operation's extra fields (responses, docs, ...).
        let op = self.op(key).cloned();
        let saved = self.save_request(None, &spec)?;
        if let (Some(op), Some(new_op)) = (op, self.op_mut(&saved)) {
            let new_obj = ensure_obj(new_op);
            for (k, v) in op.as_object().into_iter().flatten() {
                if !matches!(k.as_str(), "operationId" | "servers") {
                    new_obj.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
        }
        Ok(saved)
    }

    fn resolved_op_params(&self, op: &Value) -> Vec<(Value, Value)> {
        op.get("parameters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|p| (p.clone(), self.resolve(p).clone()))
            .collect()
    }

    fn path_level_params(&self, path: &str) -> Vec<Value> {
        self.doc
            .pointer(&format!("/paths/{}/parameters", escape_pointer(path)))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|p| self.resolve(p).clone())
            .collect()
    }
}

fn write_params(
    op: &mut Map<String, Value>,
    existing: &[(Value, Value)],
    path_level: &[Value],
    spec: &RequestSpec,
) {
    let wanted: Vec<(&str, &Param)> = spec
        .path_params
        .iter()
        .map(|p| ("path", p))
        .chain(spec.query.iter().map(|p| ("query", p)))
        .chain(spec.headers.iter().map(|p| ("header", p)))
        .collect();

    let mut out: Vec<Value> = Vec::new();
    let mut used = vec![false; wanted.len()];
    // Keep the original order and any parameter we don't edit (e.g. cookies).
    for (raw, resolved) in existing {
        let (name, loc) = param_id(resolved);
        if !matches!(loc, "path" | "query" | "header") {
            out.push(raw.clone());
            continue;
        }
        let Some(i) = wanted
            .iter()
            .enumerate()
            .position(|(i, (l, p))| !used[i] && *l == loc && p.name == name)
        else {
            continue; // removed in the editor
        };
        used[i] = true;
        let value = &wanted[i].1.value;
        if example_matches(resolved, value) {
            out.push(raw.clone()); // unchanged: keep `$ref`s and formatting intact
        } else {
            let mut p = resolved.clone();
            set_example(ensure_obj(&mut p), value);
            out.push(p);
        }
    }
    for (i, (loc, param)) in wanted.iter().enumerate() {
        if used[i] {
            continue;
        }
        // A path-level parameter already provides this value.
        if path_level
            .iter()
            .any(|p| param_id(p) == (param.name.as_str(), *loc) && example_matches(p, &param.value))
        {
            continue;
        }
        let mut p = Map::new();
        p.insert("name".into(), Value::String(param.name.clone()));
        p.insert("in".into(), Value::String((*loc).into()));
        if *loc == "path" {
            p.insert("required".into(), Value::Bool(true));
        }
        p.insert("schema".into(), json!({ "type": "string" }));
        if !param.value.is_empty() || *loc == "path" {
            p.insert("example".into(), Value::String(param.value.clone()));
        }
        out.push(Value::Object(p));
    }
    if out.is_empty() {
        op.shift_remove("parameters");
    } else {
        op.insert("parameters".into(), Value::Array(out));
    }
}

fn write_body(op: &mut Map<String, Value>, existing: Option<Value>, spec: &RequestSpec) {
    let ct = spec.content_type.trim();
    if ct.is_empty() {
        op.shift_remove("requestBody");
        return;
    }
    let mut body = existing.unwrap_or_else(|| json!({ "content": {} }));
    let body_obj = ensure_obj(&mut body);
    let content = obj_entry(body_obj, "content");
    // Changing the media type renames the entry the editor was showing.
    if !content.contains_key(ct) {
        let shown = content
            .iter()
            .find(|(_, m)| m.get("example").is_some() || m.get("examples").is_some())
            .map(|(k, _)| k.clone())
            .or_else(|| content.keys().next().cloned());
        let media = shown
            .and_then(|k| content.shift_remove(&k))
            .unwrap_or_else(|| Value::Object(Map::new()));
        content.insert(ct.to_string(), media);
    }
    let media = ensure_obj(content.get_mut(ct).expect("inserted above"));
    let example = if spec.body.is_empty() {
        None
    } else if is_json_type(ct) {
        Some(serde_json::from_str(&spec.body).unwrap_or_else(|_| Value::String(spec.body.clone())))
    } else {
        Some(Value::String(spec.body.clone()))
    };
    write_example_value(media, example);
    op.insert("requestBody".into(), body);
}

fn write_example_value(obj: &mut Map<String, Value>, example: Option<Value>) {
    // `example` and `examples` are mutually exclusive; update whichever is in use.
    if let Some(first) = obj
        .get_mut("examples")
        .and_then(Value::as_object_mut)
        .and_then(|e| e.values_mut().next())
        .filter(|e| e.get("$ref").is_none())
    {
        let first = ensure_obj(first);
        match example {
            Some(v) => {
                first.insert("value".into(), v);
            }
            None => {
                first.shift_remove("value");
            }
        }
        return;
    }
    obj.shift_remove("examples");
    match example {
        Some(v) => {
            obj.insert("example".into(), v);
        }
        None => {
            obj.shift_remove("example");
        }
    }
}

fn set_example(param: &mut Map<String, Value>, value: &str) {
    let ty = param
        .get("schema")
        .and_then(|s| s.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("string");
    let typed = match ty {
        "integer" => value.parse::<i64>().ok().map(Value::from),
        "number" => value.parse::<f64>().ok().and_then(|f| serde_json::Number::from_f64(f).map(Value::Number)),
        "boolean" => value.parse::<bool>().ok().map(Value::Bool),
        _ => None,
    };
    let example = if value.is_empty() {
        None
    } else {
        Some(typed.unwrap_or_else(|| Value::String(value.to_string())))
    };
    write_example_value(param, example);
}

fn example_matches(param: &Value, value: &str) -> bool {
    let current = match param.get("example").or_else(|| {
        param
            .get("examples")?
            .as_object()?
            .values()
            .next()?
            .get("value")
    }) {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
    };
    current == value
}

pub fn is_json_type(ct: &str) -> bool {
    let ct = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    ct == "application/json" || ct.ends_with("+json")
}

fn op_name(op: &Value) -> String {
    op.get("summary")
        .or_else(|| op.get("operationId"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn param_id(p: &Value) -> (&str, &str) {
    (
        p.get("name").and_then(Value::as_str).unwrap_or(""),
        p.get("in").and_then(Value::as_str).unwrap_or(""),
    )
}

/// A server object's URL with its variables replaced by their defaults.
fn server_url(server: &Value) -> Option<String> {
    let mut url = server.get("url")?.as_str()?.to_string();
    if let Some(vars) = server.get("variables").and_then(Value::as_object) {
        for (name, var) in vars {
            if let Some(default) = var.get("default").and_then(Value::as_str) {
                url = url.replace(&format!("{{{name}}}"), default);
            }
        }
    }
    Some(url.trim_end_matches('/').to_string())
}

fn join_url(server: &str, path: &str) -> String {
    let server = server.trim_end_matches('/');
    if server.is_empty() {
        path.to_string()
    } else if path == "/" {
        format!("{server}/")
    } else {
        format!("{server}{path}")
    }
}

fn escape_pointer(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}

fn ensure_obj(v: &mut Value) -> &mut Map<String, Value> {
    if !v.is_object() {
        *v = Value::Object(Map::new());
    }
    v.as_object_mut().expect("just made an object")
}

fn root_obj(doc: &mut Value) -> &mut Map<String, Value> {
    ensure_obj(doc)
}

fn obj_entry<'a>(map: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    let v = map
        .entry(key.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    ensure_obj(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coll(yaml: &str) -> Collection {
        Collection::parse(PathBuf::from("t.yaml"), Format::Yaml, yaml).unwrap()
    }

    const PETSTORE: &str = r#"
openapi: 3.0.3
info:
  title: Pets
  version: "1"
  x-custom: keep-me
servers:
  - url: https://{env}.example.com/v1
    variables:
      env:
        default: api
paths:
  /pets/{petId}:
    parameters:
      - $ref: '#/components/parameters/PetId'
    get:
      summary: Get pet
      operationId: getPet
      tags: [pets]
      parameters:
        - name: verbose
          in: query
          schema: { type: boolean }
          example: true
        - name: session
          in: cookie
          schema: { type: string }
      responses:
        "200":
          description: ok
    put:
      summary: Update pet
      requestBody:
        content:
          application/json:
            schema: { type: object }
            examples:
              basic:
                value: { name: Rex }
      responses:
        "200": { description: ok }
components:
  parameters:
    PetId:
      name: petId
      in: path
      required: true
      schema: { type: integer }
      example: 42
"#;

    #[test]
    fn loads_resolved_request() {
        let c = coll(PETSTORE);
        let key = OpKey {
            path: "/pets/{petId}".into(),
            method: Method::Get,
        };
        let spec = c.load_request(&key).unwrap();
        assert_eq!(spec.name, "Get pet");
        assert_eq!(spec.url, "https://api.example.com/v1/pets/{petId}");
        assert_eq!(spec.path_params, vec![Param::new("petId", "42")]);
        assert_eq!(spec.query, vec![Param::new("verbose", "true")]);

        let put = c
            .load_request(&OpKey {
                path: "/pets/{petId}".into(),
                method: Method::Put,
            })
            .unwrap();
        assert_eq!(put.content_type, "application/json");
        assert!(put.body.contains("\"Rex\""));
    }

    #[test]
    fn unchanged_save_is_lossless() {
        let mut c = coll(PETSTORE);
        let before = c.doc.clone();
        for op in c.operations() {
            let spec = c.load_request(&op.key).unwrap();
            let key = c.save_request(Some(&op.key), &spec).unwrap();
            assert_eq!(key, op.key);
        }
        assert_eq!(c.doc, before);
    }

    #[test]
    fn edits_preserve_unmanaged_fields() {
        let mut c = coll(PETSTORE);
        let key = OpKey {
            path: "/pets/{petId}".into(),
            method: Method::Get,
        };
        let mut spec = c.load_request(&key).unwrap();
        spec.query[0].value = "false".into();
        spec.headers.push(Param::new("X-Trace", "1"));
        c.save_request(Some(&key), &spec).unwrap();
        let op = c.op(&key).unwrap();
        assert_eq!(op["tags"], json!(["pets"]));
        assert_eq!(op["operationId"], "getPet");
        let params = op["parameters"].as_array().unwrap();
        assert_eq!(params[0]["example"], json!(false)); // typed by schema
        assert_eq!(params[1]["in"], "cookie"); // untouched
        assert_eq!(params[2]["name"], "X-Trace");
        assert_eq!(c.doc["info"]["x-custom"], "keep-me");

        let mut put_spec = c
            .load_request(&OpKey {
                path: "/pets/{petId}".into(),
                method: Method::Put,
            })
            .unwrap();
        put_spec.body = r#"{"name":"Max"}"#.into();
        c.save_request(
            Some(&OpKey {
                path: "/pets/{petId}".into(),
                method: Method::Put,
            }),
            &put_spec,
        )
        .unwrap();
        let media = &c.doc["paths"]["/pets/{petId}"]["put"]["requestBody"]["content"]["application/json"];
        assert_eq!(media["examples"]["basic"]["value"], json!({"name": "Max"}));
        assert!(media.get("example").is_none());
        assert_eq!(media["schema"], json!({"type": "object"}));
    }

    #[test]
    fn new_request_sets_servers_and_moves() {
        let mut c = Collection::new(PathBuf::from("x.yaml"), "X");
        let spec = RequestSpec {
            name: "List".into(),
            method: Method::Get,
            url: "https://httpbin.org/anything/{id}?a=1&b=two%20words".into(),
            path_params: vec![Param::new("id", "7")],
            ..Default::default()
        };
        let key = c.save_request(None, &spec).unwrap();
        assert_eq!(key.path, "/anything/{id}");
        assert_eq!(c.doc["servers"][0]["url"], "https://httpbin.org");
        let loaded = c.load_request(&key).unwrap();
        assert_eq!(loaded.url, "https://httpbin.org/anything/{id}");
        assert_eq!(
            loaded.query,
            vec![Param::new("a", "1"), Param::new("b", "two words")]
        );
        assert_eq!(loaded.path_params, vec![Param::new("id", "7")]);

        // A different host becomes an operation-level server.
        let mut moved = loaded.clone();
        moved.url = "http://localhost:8080/other".into();
        moved.method = Method::Post;
        let key2 = c.save_request(Some(&key), &moved).unwrap();
        assert!(!c.contains(&key));
        assert!(c.doc["paths"].get("/anything/{id}").is_none());
        assert_eq!(c.doc["paths"].as_object().unwrap().keys().next().unwrap(), "/other");
        let mut c2 = coll(PETSTORE);
        let get = OpKey { path: "/pets/{petId}".into(), method: Method::Get };
        c2.save_request(None, &RequestSpec { url: "/zzz".into(), ..Default::default() }).unwrap();
        let mut moved_spec = c2.load_request(&get).unwrap();
        moved_spec.url = "https://api.example.com/v1/animals/{petId}".into();
        c2.save_request(Some(&get), &moved_spec).unwrap();
        // /pets/{petId} still holds PUT, so the moved GET gets a new item; /zzz stays last.
        let keys: Vec<_> = c2.doc["paths"].as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["/pets/{petId}", "/zzz", "/animals/{petId}"]);
        assert_eq!(c.load_request(&key2).unwrap().url, "http://localhost:8080/other");
        assert_eq!(c.doc["servers"][0]["url"], "https://httpbin.org");

        // Collisions are rejected instead of silently overwriting.
        let dup = RequestSpec {
            url: "http://localhost:8080/other".into(),
            method: Method::Post,
            ..Default::default()
        };
        assert!(c.save_request(None, &dup).is_err());
    }

    #[test]
    fn yaml_roundtrip_and_duplicate() {
        let mut c = coll(PETSTORE);
        let text = c.serialize().unwrap();
        let again = Collection::parse(PathBuf::from("t.yaml"), Format::Yaml, &text).unwrap();
        assert_eq!(again.doc, c.doc);
        let key = OpKey {
            path: "/pets/{petId}".into(),
            method: Method::Get,
        };
        let copy = c.duplicate_request(&key).unwrap();
        assert_eq!(copy.path, "/pets/{petId}-copy");
        assert_eq!(c.op(&copy).unwrap()["tags"], json!(["pets"]));
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("a%20b+c"), "a b c");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("%C3%A9"), "é");
    }
}
