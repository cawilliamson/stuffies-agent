//! theme engine: each theme owns the whole screen layout, not just its colours.
//!
//! every theme is one file under `themes/`. adding one is a file plus a line in the
//! registry macro; removing one is the reverse. nothing is loaded at runtime.

pub mod parts;

use ratatui::Frame;

use crate::app::App;

/// one complete screen layout.
pub trait Theme: Send + Sync {
    /// the id used by `/theme`.
    fn name(&self) -> &'static str;
    /// a one-line description for the theme list.
    fn blurb(&self) -> &'static str;
    /// draws the whole screen, returning where the text cursor belongs.
    fn render(&self, frame: &mut Frame<'_>, app: &mut App) -> Option<(u16, u16)>;

    /// rows this layout gives the transcript, used to pin the viewport.
    fn transcript_rows(&self, area: ratatui::layout::Rect) -> u16 {
        area.height.saturating_sub(6)
    }
}

/// everything a theme file needs.
pub mod prelude {
    pub use super::{parts, Theme};
    pub use crate::app::{self, App, Role};
    pub use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
    pub use ratatui::style::{Color, Modifier, Style};
    pub use ratatui::text::{Line, Span};
    pub use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};
    pub use ratatui::Frame;
}

macro_rules! registry {
    ($($module:ident),* $(,)?) => {
        /// one module per theme file.
        pub mod themes {
            $(pub mod $module;)*
        }
        /// every compiled-in theme, in listing order.
        pub static THEMES: &[&dyn Theme] = &[$(&themes::$module::Theme,)*];
    };
}

registry!(
    default,
    sidebar_left,
    sidebar_right,
    topbar,
    framed,
    split,
    minimal,
    panels,
    hud,
    crt,
    blueprint,
    double,
    stack,
    compact,
    inspector,
    centred,
    footer_grid,
    cards,
    matrix,
    paper,
    neon,
    quad,
    timeline
);

/// the default theme index.
#[must_use]
pub const fn default_index() -> usize {
    0
}

/// looks a theme up by name.
#[must_use]
pub fn index_of(name: &str) -> Option<usize> {
    THEMES.iter().position(|t| t.name() == name)
}

/// the theme for an index, falling back to the default.
#[must_use]
pub fn at(index: usize) -> &'static dyn Theme {
    THEMES
        .get(index)
        .copied()
        .unwrap_or(THEMES[default_index()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_are_at_least_twenty_themes() {
        assert!(THEMES.len() >= 20, "found {}", THEMES.len());
    }

    #[test]
    fn names_are_unique_and_every_theme_is_described() {
        let mut names: Vec<&str> = THEMES.iter().map(|t| t.name()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "theme names must be unique");
        for theme in THEMES {
            assert!(!theme.blurb().is_empty(), "{} has no blurb", theme.name());
        }
    }
}
