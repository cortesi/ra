//! Syntax highlighting and terminal colors for ra.
//!
//! This crate provides utilities for syntax-highlighted output of code and configuration,
//! as well as styled terminal output for headers and status messages.
//!
//! # Terminal Styling
//!
//! The [`Style`] struct provides RGB color support with hex color parsing. Use the semantic
//! theme constants in the [`theme`] module for consistent styling across the application.

#![warn(missing_docs)]

use std::ops::Range;

use syntect::{
    easy::HighlightLines,
    highlighting::Style as SyntectStyle,
    parsing::SyntaxSet,
    util::{LinesWithEndings, as_24_bit_terminal_escaped},
};
use two_face::{
    syntax::extra_newlines as extra_syntaxes,
    theme::{EmbeddedLazyThemeSet, EmbeddedThemeName, extra as extra_themes},
};

/// RGB color for terminal output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    /// Red component (0-255).
    pub r: u8,
    /// Green component (0-255).
    pub g: u8,
    /// Blue component (0-255).
    pub b: u8,
}

impl Rgb {
    /// Creates a new RGB color from components.
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Parses an RGB color from a hex string (e.g., "#ff5500" or "ff5500").
    ///
    /// Returns `None` if the string is not a valid 6-digit hex color.
    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.strip_prefix('#').unwrap_or(hex);
        if hex.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some(Self { r, g, b })
    }

    /// Returns the ANSI escape sequence for this color as foreground.
    fn fg_escape(&self) -> String {
        format!("\x1b[38;2;{};{};{}m", self.r, self.g, self.b)
    }
}

/// Terminal text style with optional foreground color and bold.
#[derive(Debug, Clone, Copy, Default)]
pub struct Style {
    /// Foreground color.
    pub fg: Option<Rgb>,
    /// Whether text should be bold.
    pub bold: bool,
    /// Whether text should be dimmed.
    pub dim: bool,
}

impl Style {
    /// Creates a new style with the given foreground color.
    pub const fn fg(color: Rgb) -> Self {
        Self {
            fg: Some(color),
            bold: false,
            dim: false,
        }
    }

    /// Creates a style from a hex color string.
    ///
    /// # Panics
    /// Panics if the hex string is invalid. Use this only with compile-time constants.
    pub fn from_hex(hex: &str) -> Self {
        Self::fg(Rgb::from_hex(hex).expect("invalid hex color"))
    }

    /// Returns this style with bold enabled.
    pub const fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    /// Returns this style with dim enabled.
    pub const fn dim(mut self) -> Self {
        self.dim = true;
        self
    }

    /// Applies this style to text, returning styled text with ANSI escapes.
    pub fn apply(&self, text: &str) -> String {
        let mut prefix = String::new();
        if self.bold {
            prefix.push_str("\x1b[1m");
        }
        if self.dim {
            prefix.push_str("\x1b[2m");
        }
        if let Some(color) = &self.fg {
            prefix.push_str(&color.fg_escape());
        }
        if prefix.is_empty() {
            text.to_string()
        } else {
            format!("{prefix}{text}\x1b[0m")
        }
    }

    /// Returns the ANSI escape prefix for this style.
    pub fn prefix(&self) -> String {
        let mut s = String::new();
        if self.bold {
            s.push_str("\x1b[1m");
        }
        if self.dim {
            s.push_str("\x1b[2m");
        }
        if let Some(color) = &self.fg {
            s.push_str(&color.fg_escape());
        }
        s
    }

    /// Returns the ANSI escape suffix to reset styling.
    pub fn suffix(&self) -> &'static str {
        "\x1b[0m"
    }
}

/// Semantic color theme for terminal output.
///
/// These constants define the visual appearance of different UI elements.
/// Colors are specified as hex RGB values for modern 24-bit terminal support.
pub mod theme {
    use super::{Rgb, Style};

    /// Header/title style (bold cyan).
    pub const HEADER: Style = Style::fg(Rgb::new(0x56, 0xb6, 0xc2)).bold();

    /// Breadcrumb/path style (purple).
    pub const BREADCRUMB: Style = Style::fg(Rgb::new(0xc6, 0x78, 0xdd));

    /// Search match highlight style (bold yellow).
    pub const MATCH: Style = Style::fg(Rgb::new(0xe5, 0xc0, 0x7b)).bold();

    /// Success message style (green).
    pub const SUCCESS: Style = Style::fg(Rgb::new(0x98, 0xc3, 0x79));

    /// Warning message style (orange).
    pub const WARNING: Style = Style::fg(Rgb::new(0xe5, 0xc0, 0x7b));

    /// Error message style (red).
    pub const ERROR: Style = Style::fg(Rgb::new(0xe0, 0x6c, 0x75));

    /// Dimmed/secondary text style.
    pub const DIM: Style = Style {
        fg: None,
        bold: false,
        dim: true,
    };

    /// Subheader style (bold, no color).
    pub const SUBHEADER: Style = Style {
        fg: None,
        bold: true,
        dim: false,
    };
}

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
            let ranges: Vec<(SyntectStyle, &str)> = highlighter
                .highlight_line(line, &self.syntax_set)
                .unwrap_or_else(|_| vec![(SyntectStyle::default(), line)]);
            let escaped = as_24_bit_terminal_escaped(&ranges[..], false);
            output.push_str(&escaped);
        }
        // Reset terminal colors at the end
        output.push_str("\x1b[0m");
        output
    }
}

/// Formats a header with the theme header style.
pub fn header(text: &str) -> String {
    theme::HEADER.apply(text)
}

/// Formats text as a subheader (bold).
pub fn subheader(text: &str) -> String {
    theme::SUBHEADER.apply(text)
}

/// Formats text as dimmed/less important.
pub fn dim(text: &str) -> String {
    theme::DIM.apply(text)
}

/// Formats text as a success message.
pub fn success(text: &str) -> String {
    theme::SUCCESS.apply(text)
}

/// Formats text as a warning.
pub fn warning(text: &str) -> String {
    theme::WARNING.apply(text)
}

/// Formats text as an error.
pub fn error(text: &str) -> String {
    theme::ERROR.apply(text)
}

/// Formats text as a breadcrumb/path.
pub fn breadcrumb(text: &str) -> String {
    theme::BREADCRUMB.apply(text)
}

/// Returns a dimmed horizontal rule for visual separation.
pub fn rule(width: usize) -> String {
    dim(&"─".repeat(width))
}

/// Applies ANSI highlighting to text at the specified byte ranges.
///
/// Inserts the given prefix before each highlighted region and suffix after.
/// Ranges must be valid UTF-8 byte offsets within the text. Overlapping or
/// out-of-order ranges are handled by sorting and merging.
///
/// # Arguments
/// * `text` - The text to highlight
/// * `ranges` - Byte ranges to highlight (must be valid UTF-8 boundaries)
/// * `prefix` - ANSI escape sequence to insert before each highlighted region
/// * `suffix` - ANSI escape sequence to insert after each highlighted region
pub fn highlight_ranges(text: &str, ranges: &[Range<usize>], prefix: &str, suffix: &str) -> String {
    if ranges.is_empty() {
        return text.to_string();
    }

    // Sort ranges by start position and merge overlapping ones
    let mut sorted: Vec<Range<usize>> = ranges.to_vec();
    sorted.sort_by_key(|r| r.start);

    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in sorted {
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            // Overlapping or adjacent, extend the last range
            last.end = last.end.max(range.end);
            continue;
        }
        merged.push(range);
    }

    // Build output with highlights inserted
    let mut output =
        String::with_capacity(text.len() + merged.len() * (prefix.len() + suffix.len()));
    let mut pos = 0;

    for range in merged {
        // Clamp range to text bounds
        let start = range.start.min(text.len());
        let end = range.end.min(text.len());

        if start > pos {
            output.push_str(&text[pos..start]);
        }
        if start < end {
            output.push_str(prefix);
            output.push_str(&text[start..end]);
            output.push_str(suffix);
        }
        pos = end;
    }

    // Append remaining text
    if pos < text.len() {
        output.push_str(&text[pos..]);
    }

    output
}

/// Highlights text ranges with the search match style.
pub fn highlight_matches(text: &str, ranges: &[Range<usize>]) -> String {
    highlight_ranges(text, ranges, &theme::MATCH.prefix(), theme::MATCH.suffix())
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
        // Should contain 24-bit ANSI escape codes and reset
        assert!(h.contains("\x1b["));
        assert!(h.contains("\x1b[0m"));
        assert!(h.contains("Test"));
    }

    #[test]
    fn test_dim_formatting() {
        let d = dim("faint");
        assert!(d.contains("\x1b[2m")); // dim escape
        assert!(d.contains("\x1b[0m")); // reset
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

    #[test]
    fn test_highlight_ranges_empty() {
        let text = "hello world";
        let result = highlight_ranges(text, &[], "[", "]");
        assert_eq!(result, "hello world");
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn test_highlight_ranges_single() {
        let text = "hello world";
        let ranges = vec![0..5];
        let result = highlight_ranges(text, &ranges, "[", "]");
        assert_eq!(result, "[hello] world");
    }

    #[test]
    fn test_highlight_ranges_multiple() {
        let text = "hello world";
        let result = highlight_ranges(text, &[0..5, 6..11], "[", "]");
        assert_eq!(result, "[hello] [world]");
    }

    #[test]
    fn test_highlight_ranges_overlapping() {
        let text = "hello world";
        // Overlapping ranges should be merged
        let result = highlight_ranges(text, &[0..7, 4..11], "[", "]");
        assert_eq!(result, "[hello world]");
    }

    #[test]
    fn test_highlight_ranges_unsorted() {
        let text = "hello world";
        // Ranges given out of order should still work
        let result = highlight_ranges(text, &[6..11, 0..5], "[", "]");
        assert_eq!(result, "[hello] [world]");
    }

    #[test]
    fn test_highlight_ranges_adjacent() {
        let text = "hello world";
        // Adjacent ranges should be merged
        let result = highlight_ranges(text, &[0..5, 5..6], "[", "]");
        assert_eq!(result, "[hello ]world");
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn test_highlight_matches_uses_ansi() {
        let text = "hello";
        let ranges = vec![0..5];
        let result = highlight_matches(text, &ranges);
        // Should contain ANSI escape codes and reset
        assert!(result.contains("\x1b["));
        assert!(result.contains("\x1b[0m"));
        assert!(result.contains("hello"));
    }

    #[test]
    fn test_rgb_from_hex() {
        let color = Rgb::from_hex("#ff5500").unwrap();
        assert_eq!(color.r, 0xff);
        assert_eq!(color.g, 0x55);
        assert_eq!(color.b, 0x00);

        // Without hash prefix
        let color2 = Rgb::from_hex("aabbcc").unwrap();
        assert_eq!(color2.r, 0xaa);
        assert_eq!(color2.g, 0xbb);
        assert_eq!(color2.b, 0xcc);

        // Invalid inputs
        assert!(Rgb::from_hex("fff").is_none()); // too short
        assert!(Rgb::from_hex("gggggg").is_none()); // invalid hex
    }

    #[test]
    fn test_style_apply() {
        let style = Style::from_hex("#ff0000").bold();
        let styled = style.apply("red");
        assert!(styled.contains("\x1b[1m")); // bold
        assert!(styled.contains("\x1b[38;2;255;0;0m")); // RGB foreground
        assert!(styled.contains("\x1b[0m")); // reset
        assert!(styled.contains("red"));
    }

    #[test]
    fn test_theme_constants() {
        // Verify theme constants produce valid ANSI output
        let h = theme::HEADER.apply("test");
        assert!(h.contains("\x1b["));
        assert!(h.ends_with("\x1b[0m"));

        let b = theme::BREADCRUMB.apply("path");
        assert!(b.contains("\x1b[38;2;")); // RGB prefix
    }
}
