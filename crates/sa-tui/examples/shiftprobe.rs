//! logs the key events a real terminal delivers to a file, so what it sends can be read
//! back without watching the screen.
#![allow(clippy::expect_used)]
use crossterm::event::{EventStream, KeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
use futures_util::StreamExt as _;
use std::io::Write as _;

#[tokio::main]
async fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/shiftprobe.log".to_owned());
    let mut file = std::fs::File::create(&path).expect("log file");

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
    let _ = writeln!(file, "pushed={pushed}");

    let mut stream = EventStream::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(25);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(event))) =
            tokio::time::timeout(std::time::Duration::from_millis(200), stream.next()).await
        {
            let _ = writeln!(file, "{event:?}");
            let _ = file.flush();
        }
    }

    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::event::PopKeyboardEnhancementFlags
    );
    let _ = crossterm::terminal::disable_raw_mode();
}
