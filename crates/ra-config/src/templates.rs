//! Configuration templates for `ra init`.
//!
//! Templates are stored as valid TOML files and returned as commented-out
//! example configurations.

/// Default local configuration template (valid TOML).
const LOCAL_TEMPLATE: &str = include_str!("../templates/config.toml");

/// Global configuration template (valid TOML).
const GLOBAL_TEMPLATE: &str = include_str!("../templates/config-global.toml");

/// Returns the local configuration template as a commented-out example.
pub fn local_template() -> String {
    comment_template(LOCAL_TEMPLATE)
}

/// Returns the global configuration template as a commented-out example.
pub fn global_template() -> String {
    comment_template(GLOBAL_TEMPLATE)
}

/// Converts a valid TOML template into a commented-out example config.
///
/// Lines that are already comments are preserved as-is. Non-comment, non-empty
/// lines get a "# " prefix. Empty lines are preserved.
fn comment_template(template: &str) -> String {
    let mut result = String::with_capacity(template.len() + template.lines().count() * 2);
    for line in template.lines() {
        if !line.is_empty() && !line.starts_with('#') {
            result.push_str("# ");
        }
        result.push_str(line);
        result.push('\n');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_config;

    #[test]
    fn local_template_parses_as_valid_toml() {
        let result = parse_config(LOCAL_TEMPLATE);
        assert!(result.is_ok(), "local template failed to parse: {result:?}");
    }

    #[test]
    fn global_template_parses_as_valid_toml() {
        let result = parse_config(GLOBAL_TEMPLATE);
        assert!(
            result.is_ok(),
            "global template failed to parse: {result:?}"
        );
    }

    #[test]
    fn comment_template_preserves_existing_comments() {
        let input = "# This is a comment\nkey = \"value\"\n";
        let result = comment_template(input);
        assert_eq!(result, "# This is a comment\n# key = \"value\"\n");
    }

    #[test]
    fn comment_template_preserves_empty_lines() {
        let input = "key1 = \"a\"\n\nkey2 = \"b\"\n";
        let result = comment_template(input);
        assert_eq!(result, "# key1 = \"a\"\n\n# key2 = \"b\"\n");
    }

    #[test]
    fn comment_template_handles_section_headers() {
        let input = "[section]\nkey = \"value\"\n";
        let result = comment_template(input);
        assert_eq!(result, "# [section]\n# key = \"value\"\n");
    }
}
