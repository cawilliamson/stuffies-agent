//! reads keys the way the tui does and prints them, for checking what a terminal sends.
//!
//! run it in the terminal you use: `cargo run -p sa-tui --example keyprobe`, then press
//! shift+enter. a `modifiers: SHIFT` line means the terminal reports it; a bare enter means
//! it cannot, and ctrl+j is the newline that always works.
#![allow(clippy::expect_used, clippy::print_stderr)]
use crossterm::event::{Event, KeyboardEnhancementFlags, PushKeyboardEnhancementFlags};

fn main() {
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
    eprintln!("pushed={pushed}");
    for _ in 0..6 {
        match crossterm::event::read() {
            Ok(Event::Key(key)) => eprintln!("KEY {key:?}"),
            Ok(other) => eprintln!("OTHER {other:?}"),
            Err(err) => {
                eprintln!("ERR {err}");
                break;
            }
        }
    }
    let _ = crossterm::terminal::disable_raw_mode();
}
