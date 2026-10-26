//! Request execution. Everything here runs off the UI thread.

use crate::openapi::{Method, RequestSpec, is_json_type};
use anyhow::{Result, anyhow, bail};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

/// Bodies larger than this are not read into memory.
const MAX_BODY: u64 = 512 * 1024 * 1024;
/// Text beyond this is not put into the editor (the raw body can still be saved).
const DISPLAY_LIMIT: usize = 8 * 1024 * 1024;
/// Syntax highlighting is skipped above this size to keep the UI snappy.
const HIGHLIGHT_LIMIT: usize = 2 * 1024 * 1024;

static AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(300)))
        .max_redirects(10)
        .user_agent(concat!("rocketman/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
});

#[derive(Debug, Clone)]
pub struct PreparedRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

pub struct Response {
    pub status: u16,
    pub reason: String,
    pub elapsed: Duration,
    pub headers: Vec<(String, String)>,
    pub body: Arc<Vec<u8>>,
    /// Display text: pretty-printed JSON, decoded text or a binary notice.
    pub text: String,
    /// Language for highlighting, empty for none.
    pub language: &'static str,
    pub note: Option<String>,
}

/// Turns the editor's spec into a concrete request: substitutes path variables,
/// appends query parameters and adds the body's content type.
pub fn prepare(spec: &RequestSpec) -> Result<PreparedRequest> {
    let spec = spec.normalized();
    let mut url = spec.url.trim().to_string();
    if url.is_empty() {
        bail!("Enter a URL");
    }
    if !url.contains("://") {
        url.insert_str(0, "http://");
    }
    for p in &spec.path_params {
        url = url.replace(&format!("{{{}}}", p.name), &encode(&p.value, false));
    }
    if !spec.query.is_empty() {
        let mut first = true;
        for p in &spec.query {
            url.push(if first { '?' } else { '&' });
            first = false;
            url.push_str(&encode(&p.name, true));
            url.push('=');
            url.push_str(&encode(&p.value, true));
        }
    }
    url.parse::<ureq::http::Uri>()
        .map_err(|e| anyhow!("Invalid URL: {e}"))?;

    let mut headers: Vec<(String, String)> = spec
        .headers
        .iter()
        .map(|h| (h.name.trim().to_string(), h.value.clone()))
        .collect();
    let has_body = !spec.content_type.trim().is_empty();
    if has_body
        && !headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
    {
        headers.push(("Content-Type".into(), spec.content_type.trim().to_string()));
    }
    Ok(PreparedRequest {
        method: spec.method,
        url,
        headers,
        body: has_body.then(|| spec.body.into_bytes()),
    })
}

/// Blocking. Call from a worker thread.
pub fn execute(req: PreparedRequest) -> Result<Response> {
    let agent = &*AGENT;
    let url = req.url.as_str();
    macro_rules! with_headers {
        ($builder:expr) => {{
            let mut b = $builder;
            for (k, v) in &req.headers {
                b = b.header(k.as_str(), v.as_str());
            }
            b
        }};
    }
    let started = Instant::now();
    let result = match (req.method, req.body) {
        (Method::Post, body) => send(with_headers!(agent.post(url)), body),
        (Method::Put, body) => send(with_headers!(agent.put(url)), body),
        (Method::Patch, body) => send(with_headers!(agent.patch(url)), body),
        (m, None) => {
            let b = match m {
                Method::Get => agent.get(url),
                Method::Delete => agent.delete(url),
                Method::Head => agent.head(url),
                Method::Options => agent.options(url),
                _ => agent.trace(url),
            };
            with_headers!(b).call()
        }
        (m, Some(body)) => {
            let b = match m {
                Method::Get => agent.get(url),
                Method::Delete => agent.delete(url),
                Method::Head => agent.head(url),
                Method::Options => agent.options(url),
                _ => agent.trace(url),
            };
            with_headers!(b).force_send_body().send(body)
        }
    };
    let mut resp = result.map_err(describe_error)?;
    let status = resp.status();
    let headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
        .collect();
    let body = resp
        .body_mut()
        .with_config()
        .limit(MAX_BODY)
        .read_to_vec()
        .map_err(describe_error)?;
    let elapsed = started.elapsed();

    let content_type = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    let (text, language, note) = render_body(&body, content_type);
    Ok(Response {
        status: status.as_u16(),
        reason: status.canonical_reason().unwrap_or("").to_string(),
        elapsed,
        headers,
        body: Arc::new(body),
        text,
        language,
        note,
    })
}

fn send(
    b: ureq::RequestBuilder<ureq::typestate::WithBody>,
    body: Option<Vec<u8>>,
) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    match body {
        Some(body) => b.send(body),
        None => b.send_empty(),
    }
}

fn describe_error(e: ureq::Error) -> anyhow::Error {
    use ureq::Error as E;
    match e {
        E::Timeout(t) => anyhow!("Request timed out ({t})"),
        E::HostNotFound => anyhow!("Could not resolve host"),
        E::Io(io) if io.kind() == std::io::ErrorKind::ConnectionRefused => {
            anyhow!("Connection refused")
        }
        E::BodyExceedsLimit(limit) => anyhow!("Response body is larger than {limit} bytes"),
        other => anyhow!("{other}"),
    }
}

fn render_body(body: &[u8], content_type: &str) -> (String, &'static str, Option<String>) {
    if body.is_empty() {
        return (String::new(), "", None);
    }
    let ct = content_type.to_ascii_lowercase();
    let Ok(text) = std::str::from_utf8(body) else {
        return (
            String::new(),
            "",
            Some(format!(
                "Binary response ({}). Use \"Save\" to write it to a file.",
                format_size(body.len())
            )),
        );
    };
    let looks_json = is_json_type(&ct)
        || (ct.is_empty() || ct.starts_with("text/plain"))
            && matches!(text.trim_start().as_bytes().first(), Some(b'{' | b'['));
    // Never copy more than we can show.
    let head = |s: &str| {
        let mut cut = s.len().min(DISPLAY_LIMIT + 1);
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s[..cut].to_string()
    };
    let mut out = if looks_json {
        pretty_json(text).unwrap_or_else(|| head(text))
    } else {
        head(text)
    };
    let mut note = None;
    if out.len() > DISPLAY_LIMIT {
        let mut cut = DISPLAY_LIMIT;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        note = Some(format!(
            "Showing the first {} of {}. Use \"Save\" for the full body.",
            format_size(DISPLAY_LIMIT),
            format_size(body.len())
        ));
    }
    let language = if out.len() > HIGHLIGHT_LIMIT {
        ""
    } else if looks_json {
        "json"
    } else {
        ""
    };
    (out, language, note)
}

/// Re-indents JSON without parsing it into values, so numbers and key order
/// are shown exactly as the server sent them. Single pass, stops early once
/// the output exceeds the display limit. Returns `None` for unbalanced input.
fn pretty_json(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len().min(DISPLAY_LIMIT) + bytes.len().min(DISPLAY_LIMIT) / 2);
    let mut depth: usize = 0;
    let mut i = 0;
    let newline = |out: &mut Vec<u8>, depth: usize| {
        out.push(b'\n');
        out.extend(std::iter::repeat_n(b' ', depth * 2));
    };
    while i < bytes.len() {
        if out.len() > DISPLAY_LIMIT + 1024 {
            break;
        }
        let b = bytes[i];
        match b {
            b'"' => {
                let start = i;
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                let end = (i + 1).min(bytes.len());
                out.extend_from_slice(&bytes[start..end]);
            }
            b'{' | b'[' => {
                out.push(b);
                let close = if b == b'{' { b'}' } else { b']' };
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if bytes.get(j) == Some(&close) {
                    out.push(close);
                    i = j;
                } else {
                    depth += 1;
                    newline(&mut out, depth);
                }
            }
            b'}' | b']' => {
                depth = depth.checked_sub(1)?;
                newline(&mut out, depth);
                out.push(b);
            }
            b',' => {
                out.push(b',');
                newline(&mut out, depth);
            }
            b':' => out.extend_from_slice(b": "),
            b' ' | b'\t' | b'\n' | b'\r' => {}
            _ => out.push(b),
        }
        i += 1;
    }
    // Input was valid UTF-8 and we only split at ASCII bytes.
    String::from_utf8(out).ok()
}

/// Percent-encodes everything except RFC 3986 unreserved characters.
/// Path values keep `/`-free segments; query values also escape `&`, `=` and `+`.
fn encode(s: &str, query: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let keep = b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'.' | b'_' | b'~')
            || (!query && matches!(b, b':' | b'@' | b'!' | b'$' | b'\'' | b'(' | b')' | b'*' | b',' | b';'))
            || (query && matches!(b, b':' | b'@' | b'/' | b'?' | b'!' | b'$' | b'\'' | b'(' | b')' | b'*' | b',' | b';'));
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn format_size(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / KB / KB)
    } else {
        format!("{:.2} GB", b / KB / KB / KB)
    }
}

pub fn format_duration(d: Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.2} s", ms / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openapi::Param;

    #[test]
    fn prepares_url() {
        let spec = RequestSpec {
            method: Method::Get,
            url: "example.com/users/{id}?x=1".into(),
            path_params: vec![Param::new("id", "a b/c")],
            query: vec![Param::new("q", "rust & gpui"), Param::new("", "ignored")],
            ..Default::default()
        };
        let req = prepare(&spec).unwrap();
        assert_eq!(
            req.url,
            "http://example.com/users/a%20b%2Fc?x=1&q=rust%20%26%20gpui"
        );
        assert!(req.body.is_none());
    }

    #[test]
    fn adds_content_type() {
        let spec = RequestSpec {
            method: Method::Post,
            url: "https://x.dev".into(),
            content_type: "application/json".into(),
            body: "{}".into(),
            ..Default::default()
        };
        let req = prepare(&spec).unwrap();
        assert_eq!(
            req.headers,
            vec![("Content-Type".to_string(), "application/json".to_string())]
        );
        assert_eq!(req.body.as_deref(), Some(&b"{}"[..]));
    }

    #[test]
    fn pretty_json_is_verbatim() {
        let src = r#"{"big":12345678901234567890,"f":1.10,"s":"a\\\"b, {c}","e":[],"o":{ },"n":[1,{"x":null}]}"#;
        let out = pretty_json(src).unwrap();
        assert_eq!(
            out,
            "{\n  \"big\": 12345678901234567890,\n  \"f\": 1.10,\n  \"s\": \"a\\\\\\\"b, {c}\",\n  \"e\": [],\n  \"o\": {},\n  \"n\": [\n    1,\n    {\n      \"x\": null\n    }\n  ]\n}"
        );
        assert!(pretty_json("}").is_none());
    }

    #[test]
    fn renders_bodies() {
        let (t, lang, _) = render_body(br#"{"a":[1,2]}"#, "application/json; charset=utf-8");
        assert_eq!(lang, "json");
        assert!(t.contains("\n"));
        let (_, _, note) = render_body(&[0xff, 0xfe, 0x00], "application/octet-stream");
        assert!(note.unwrap().starts_with("Binary"));
    }
}
