mod app;
mod http;
mod kv;
mod openapi;
mod workspace;

use app::*;
use gpui_kit::*;
use std::path::PathBuf;

/// ⌘ on macOS, Ctrl elsewhere.
#[cfg(target_os = "macos")]
macro_rules! secondary {
    ($key:literal) => {
        concat!("cmd-", $key)
    };
}
#[cfg(not(target_os = "macos"))]
macro_rules! secondary {
    ($key:literal) => {
        concat!("ctrl-", $key)
    };
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    // `rocketman [FOLDER]` opens a folder of OpenAPI files (e.g. inside a repo).
    let dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .or_else(workspace::config::last_dir)
        .unwrap_or_else(workspace::config::default_dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("rocketman: cannot use {}: {e}", dir.display());
        std::process::exit(1);
    }
    let first_run = std::fs::read_dir(&dir).is_ok_and(|mut d| d.next().is_none());
    if first_run && dir == workspace::config::default_dir() {
        seed_example(&dir);
    }

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.bind_keys([
                KeyBinding::new(secondary!("enter"), Send, None),
                // Inputs bind ⌘↵ themselves; ours wins so it sends instead of inserting a newline.
                KeyBinding::new(secondary!("enter"), Send, Some("Input")),
                KeyBinding::new(secondary!("s"), Save, None),
                KeyBinding::new(secondary!("n"), NewRequest, None),
                KeyBinding::new(secondary!("shift-n"), NewCollection, None),
                KeyBinding::new(secondary!("o"), OpenFolder, None),
                KeyBinding::new(secondary!("r"), Reload, None),
                KeyBinding::new(secondary!("l"), FocusUrl, None),
                KeyBinding::new("escape", Cancel, Some("Rocketman")),
                KeyBinding::new(secondary!("q"), Quit, None),
            ]);
            cx.set_menus([
                Menu::new("Rocketman").items([MenuItem::action("Quit Rocketman", Quit)]),
                Menu::new("File").items([
                    MenuItem::action("New Request", NewRequest),
                    MenuItem::action("New Collection…", NewCollection),
                    MenuItem::separator(),
                    MenuItem::action("Open Folder…", OpenFolder),
                    MenuItem::action("Reload Collections", Reload),
                    MenuItem::separator(),
                    MenuItem::action("Save", Save),
                ]),
                Menu::new("Request").items([
                    MenuItem::action("Send", Send),
                    MenuItem::action("Cancel", Cancel),
                    MenuItem::action("Focus URL", FocusUrl),
                ]),
            ]);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Rocketman".into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(720.), px(480.))),
                ..Default::default()
            };
            let opened = gpui_kit::open_window(options, cx, |window, cx| {
                let view = cx.new(|cx| RocketApp::new(dir.clone(), window, cx));
                view.read(cx).focus_handle(cx).focus(window, cx);
                view
            });
            if let Err(e) = opened {
                eprintln!("rocketman: failed to open window: {e:#}");
                cx.quit();
            }
            cx.activate(true);
        });
}

/// Gives a first-time user something to click.
fn seed_example(dir: &std::path::Path) {
    let mut c = openapi::Collection::new(dir.join("httpbin.yaml"), "httpbin");
    let examples = [
        ("Get anything", openapi::Method::Get, "https://httpbin.org/get?hello=world", "", ""),
        (
            "Post JSON",
            openapi::Method::Post,
            "https://httpbin.org/post",
            "application/json",
            "{\n  \"name\": \"rocketman\",\n  \"fast\": true\n}",
        ),
        ("Put", openapi::Method::Put, "https://httpbin.org/put", "text/plain", "hello"),
        ("Delete", openapi::Method::Delete, "https://httpbin.org/delete", "", ""),
        ("Status code", openapi::Method::Get, "https://httpbin.org/status/{code}", "", ""),
    ];
    for (name, method, url, ct, body) in examples {
        let mut spec = openapi::RequestSpec {
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
