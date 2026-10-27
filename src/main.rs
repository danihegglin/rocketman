mod app;
mod kv;

use app::*;
use gpui_kit::*;
use gpui_kit::component::TitleBar;

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
    let dir = match rocketman::session::startup_dir(std::env::args_os().nth(1)) {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("rocketman: {e:#}");
            std::process::exit(1);
        }
    };

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
                KeyBinding::new(secondary!("i"), ImportOpenApi, None),
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
                    MenuItem::action("Import OpenAPI File…", ImportOpenApi),
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
                    ..TitleBar::title_bar_options()
                }),
                // The app renders its own `TitleBar` (drag, double-click, window
                // controls), so ask Linux for client-side decorations everywhere
                // instead of relying on a compositor that may draw none.
                window_decorations: Some(WindowDecorations::Client),
                app_id: Some("rocketman".into()),
                window_min_size: Some(size(px(720.), px(480.))),
                ..TitleBar::window_options()
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
