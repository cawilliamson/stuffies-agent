//! theme engine: each theme owns its whole layout, not just its colours.
//!
//! every theme lives in its own file under `themes/`. adding one is a file plus a line in
//! the registry macro below; removing one is the reverse. nothing is loaded at runtime.

pub mod parts;

use ratatui::Frame;

use crate::app::App;

/// one complete screen layout.
pub trait Theme: Send + Sync {
    /// the id used by `/theme`.
    fn name(&self) -> &'static str;
    /// a one-line description for the theme list.
    fn blurb(&self) -> &'static str;
    /// draws the whole screen. returns where the text cursor belongs, if anywhere.
    fn render(&self, frame: &mut Frame<'_>, app: &App) -> Option<(u16, u16)>;

    /// how many rows of the screen this layout gives to the transcript. used to pin the
    /// viewport to the newest content; the default covers the usual chrome budget.
    fn transcript_rows(&self, area: ratatui::layout::Rect) -> u16 {
        area.height.saturating_sub(4)
    }
}

/// everything a theme file needs.
pub mod prelude {
    pub use super::{parts, Theme};
    pub use crate::app::App;
    pub use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
    pub use ratatui::style::{Color, Modifier, Style};
    pub use ratatui::text::{Line, Span};
    pub use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
    pub use ratatui::Frame;
}

macro_rules! registry {
    ($($module:ident),* $(,)?) => {
        /// one module per theme file. removing a file and its entry here removes the theme.
        pub mod themes {
            $(pub mod $module;)*
        }
        /// every compiled-in theme, in listing order.
        pub static THEMES: &[&dyn Theme] = &[$(&themes::$module::Theme,)*];
    };
}

registry!(
    classic,
    sidebar_left,
    sidebar_right,
    topbar,
    framed,
    split,
    minimal,
    panels,
    hud,
    crt_green,
    crt_amber,
    blueprint,
    double,
    tabs,
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
pub fn default_index() -> usize {
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
    fn every_theme_has_a_unique_name_and_blurb() {
        let mut names: Vec<&str> = THEMES.iter().map(|t| t.name()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "theme names must be unique");
        for theme in THEMES {
            assert!(!theme.blurb().is_empty(), "{} needs a blurb", theme.name());
        }
    }

    #[test]
    fn there_are_at_least_twenty_themes() {
        assert!(
            THEMES.len() >= 20,
            "expected at least 20 themes, found {}",
            THEMES.len()
        );
    }
}
