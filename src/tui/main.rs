//! rocketman in the terminal: the same collections, editor and request
//! runner as the desktop app, drawn with ratatui.

mod app;
mod text;
mod ui;

use app::App;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::{execute, terminal};
use std::io::Write as _;
use std::time::Duration;

fn main() {
    // Logs would scribble over the screen; they only go out when asked for.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("off")).init();

    let arg = std::env::args_os().nth(1);
    if arg.as_deref().is_some_and(|a| a == "-h" || a == "--help") {
        println!("Usage: rocketman-tui [FOLDER]\n\nOpens a folder of OpenAPI files (default: the last used folder).\nPress F1 inside for keys.");
        return;
    }
    // `rocketman-tui [FOLDER]` opens a folder of OpenAPI files (e.g. inside a repo).
    let dir = match rocketman::session::startup_dir(arg) {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("rocketman: {e:#}");
            std::process::exit(1);
        }
    };

    let mut terminal = ratatui::init();
    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, EnableBracketedPaste);
    // Lets terminals that support it report Ctrl-Enter and Ctrl-I distinctly.
    let enhanced = terminal::supports_keyboard_enhancement().unwrap_or(false)
        && execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok();

    let result = run(&mut terminal, App::new(&dir));

    if enhanced {
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(stdout, DisableBracketedPaste);
    ratatui::restore();
    if let Err(e) = result {
        eprintln!("rocketman: {e}");
        std::process::exit(1);
    }
}

fn run(terminal: &mut ratatui::DefaultTerminal, mut app: App) -> std::io::Result<()> {
    while !app.quit {
        app.tick();
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        if let Some(text) = app.clipboard.take() {
            copy_to_clipboard(&text)?;
        }
        // Redraw often while a request is in flight so the timer ticks.
        let timeout = if app.is_loading() || app.notice.is_some() {
            Duration::from_millis(100)
        } else {
            Duration::from_millis(500)
        };
        if !event::poll(timeout)? {
            continue;
        }
        // Handle everything that's queued before drawing again (fast typing, pastes).
        loop {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => app.handle_key(key),
                Event::Paste(text) => app.paste(&text),
                _ => {}
            }
            if app.quit || !event::poll(Duration::ZERO)? {
                break;
            }
        }
    }
    Ok(())
}

/// Copies through the terminal (OSC 52), which also works over SSH.
fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    let mut out = std::io::stdout();
    write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()))?;
    out.flush()
}

fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64() {
        assert_eq!(super::base64(b"rocketman"), "cm9ja2V0bWFu");
        assert_eq!(super::base64(b"hi"), "aGk=");
        assert_eq!(super::base64(b"h"), "aA==");
    }
}
