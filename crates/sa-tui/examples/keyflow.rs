//! feeds real terminal keys through the app's key handler and records what it decided.
//! no drawing, no daemon: this is the input path on its own.
#![allow(clippy::expect_used)]
use crossterm::event::{self, KeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
use std::io::Write as _;
use std::time::Duration;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/keyflow.log".to_owned());
    let mut log = std::fs::File::create(&path).expect("log");

    crossterm::terminal::enable_raw_mode().expect("raw mode");
    let pushed = crossterm::execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
        )
    )
    .is_ok();
    let _ = writeln!(log, "pushed={pushed}");

    let mut app = sa_tui::app::App::new("session-abcdef", "m");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        if !event::poll(Duration::from_millis(200)).unwrap_or(false) {
            continue;
        }
        let Ok(event::Event::Key(key)) = event::read() else {
            continue;
        };
        let action = app.on_key(key);
        let _ = writeln!(
            log,
            "{key:?} -> {action:?} input={:?}",
            app.tab().input_text()
        );
        let _ = log.flush();
    }
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::event::PopKeyboardEnhancementFlags
    );
    let _ = crossterm::terminal::disable_raw_mode();
}
