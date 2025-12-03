//! Syntax highlighting and terminal colors for ra.
//!
//! This crate provides utilities for syntax-highlighted output of code and configuration,
//! as well as styled terminal output for headers and status messages.

#![warn(missing_docs)]

use syntect::{
    easy::HighlightLines,
    highlighting::Style,
    parsing::SyntaxSet,
    util::{LinesWithEndings, as_24_bit_terminal_escaped},
};
use two_face::{
    syntax::extra_newlines as extra_syntaxes,
    theme::{EmbeddedLazyThemeSet, EmbeddedThemeName, extra as extra_themes},
};

/// A syntax highlighter that can highlight code for terminal output.
pub struct Highlighter {
    /// The syntax set containing language definitions (including TOML, TypeScript, etc.).
    syntax_set: SyntaxSet,
    /// The theme set containing color themes.
    theme_set: EmbeddedLazyThemeSet,
    /// The theme to use.
    theme: EmbeddedThemeName,
}

impl Default for Highlighter {
    fn default() -> Self {
        Self::new()
    }
}

impl Highlighter {
    /// Creates a new highlighter with the default theme (Dracula).
    pub fn new() -> Self {
        Self {
            syntax_set: extra_syntaxes(),
            theme_set: extra_themes(),
            theme: EmbeddedThemeName::Dracula,
        }
    }

    /// Highlights TOML content for terminal output.
    pub fn highlight_toml(&self, content: &str) -> String {
        self.highlight(content, "toml")
    }

    /// Highlights Markdown content for terminal output.
    pub fn highlight_markdown(&self, content: &str) -> String {
        self.highlight(content, "md")
    }

    /// Highlights content with the specified syntax for terminal output.
    ///
    /// If the syntax is not found, returns the content unchanged.
    pub fn highlight(&self, content: &str, syntax_name: &str) -> String {
        let syntax = self
            .syntax_set
            .find_syntax_by_extension(syntax_name)
            .or_else(|| self.syntax_set.find_syntax_by_name(syntax_name))
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());

        let theme = self.theme_set.get(self.theme);
        let mut highlighter = HighlightLines::new(syntax, theme);

        let mut output = String::new();
        for line in LinesWithEndings::from(content) {
            let ranges: Vec<(Style, &str)> = highlighter
                .highlight_line(line, &self.syntax_set)
                .unwrap_or_else(|_| vec![(Style::default(), line)]);
            let escaped = as_24_bit_terminal_escaped(&ranges[..], false);
            output.push_str(&escaped);
        }
        // Reset terminal colors at the end
        output.push_str("\x1b[0m");
        output
    }
}

/// ANSI color codes for terminal output.
pub mod colors {
    /// Bold text.
    pub const BOLD: &str = "\x1b[1m";
    /// Cyan text (for headers).
    pub const CYAN: &str = "\x1b[36m";
    /// Green text (for success).
    pub const GREEN: &str = "\x1b[32m";
    /// Yellow text (for warnings).
    pub const YELLOW: &str = "\x1b[33m";
    /// Red text (for errors).
    pub const RED: &str = "\x1b[31m";
    /// Dim/gray text (for less important info).
    pub const DIM: &str = "\x1b[2m";
    /// Reset all formatting.
    pub const RESET: &str = "\x1b[0m";
}

/// Formats a header with bold cyan styling.
pub fn header(text: &str) -> String {
    format!("{}{}{}{}", colors::BOLD, colors::CYAN, text, colors::RESET)
}

/// Formats text as a subheader (bold).
pub fn subheader(text: &str) -> String {
    format!("{}{}{}", colors::BOLD, text, colors::RESET)
}

/// Formats text as dimmed/less important.
pub fn dim(text: &str) -> String {
    format!("{}{}{}", colors::DIM, text, colors::RESET)
}

/// Formats text as a success message (green).
pub fn success(text: &str) -> String {
    format!("{}{}{}", colors::GREEN, text, colors::RESET)
}

/// Formats text as a warning (yellow).
pub fn warning(text: &str) -> String {
    format!("{}{}{}", colors::YELLOW, text, colors::RESET)
}

/// Formats text as an error (red).
pub fn error(text: &str) -> String {
    format!("{}{}{}", colors::RED, text, colors::RESET)
}

/// Returns a dimmed horizontal rule for visual separation.
pub fn rule(width: usize) -> String {
    dim(&"─".repeat(width))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_highlighter_toml() {
        let hl = Highlighter::new();
        let toml = r#"[settings]
default_limit = 5
"#;
        let output = hl.highlight_toml(toml);
        // Should contain ANSI escape codes
        assert!(output.contains("\x1b["));
        // Should end with reset
        assert!(output.ends_with("\x1b[0m"));
    }

    #[test]
    fn test_highlighter_markdown() {
        let hl = Highlighter::new();
        let md = "# Header\n\nSome **bold** text.\n";
        let output = hl.highlight_markdown(md);
        assert!(output.contains("\x1b["));
        assert!(output.ends_with("\x1b[0m"));
    }

    #[test]
    fn test_header_formatting() {
        let h = header("Test");
        assert!(h.contains(colors::BOLD));
        assert!(h.contains(colors::CYAN));
        assert!(h.contains(colors::RESET));
        assert!(h.contains("Test"));
    }

    #[test]
    fn test_dim_formatting() {
        let d = dim("faint");
        assert!(d.contains(colors::DIM));
        assert!(d.contains(colors::RESET));
    }

    #[test]
    fn test_available_themes() {
        use two_face::theme::EmbeddedLazyThemeSet;
        let themes = EmbeddedLazyThemeSet::theme_names();
        // Should have several themes available
        assert!(!themes.is_empty());
    }

    #[test]
    fn test_toml_syntax_available() {
        let ss = extra_syntaxes();
        // Should have TOML (from two-face extras)
        assert!(
            ss.find_syntax_by_extension("toml").is_some(),
            "TOML syntax should be available"
        );
        // Should have Markdown
        assert!(
            ss.find_syntax_by_extension("md").is_some(),
            "Markdown syntax should be available"
        );
    }
}
