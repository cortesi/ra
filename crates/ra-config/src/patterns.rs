//! Include/exclude pattern compilation and matching.
//!
//! Compiles glob patterns from tree configuration into efficient matchers
//! for determining which files to index from each tree.

use std::{collections::HashMap, path::Path};

use globset::{Glob, GlobMatcher, GlobSet, GlobSetBuilder};

use crate::{ConfigError, ContextSettings, Tree};

/// Compiled glob patterns for efficient file matching.
///
/// Patterns are organized per-tree, allowing quick lookup of whether
/// a file path should be included for indexing in a given tree.
#[derive(Debug)]
pub struct CompiledPatterns {
    /// Compiled include patterns per tree name.
    include_patterns: HashMap<String, GlobSet>,
    /// Compiled exclude patterns per tree name.
    exclude_patterns: HashMap<String, GlobSet>,
}

impl CompiledPatterns {
    /// Compiles include/exclude patterns from trees into efficient matchers.
    pub fn compile(trees: &[Tree]) -> Result<Self, ConfigError> {
        let mut include_patterns: HashMap<String, GlobSet> = HashMap::new();
        let mut exclude_patterns: HashMap<String, GlobSet> = HashMap::new();

        for tree in trees {
            // Build include patterns
            let mut include_builder = GlobSetBuilder::new();
            for pattern in &tree.include {
                include_builder.add(compile_glob(pattern)?);
            }
            let include_set = include_builder
                .build()
                .map_err(|e| ConfigError::InvalidPattern {
                    pattern: format!("<combined include patterns for {}>", tree.name),
                    source: e,
                })?;
            include_patterns.insert(tree.name.clone(), include_set);

            // Build exclude patterns
            let mut exclude_builder = GlobSetBuilder::new();
            for pattern in &tree.exclude {
                exclude_builder.add(compile_glob(pattern)?);
            }
            let exclude_set = exclude_builder
                .build()
                .map_err(|e| ConfigError::InvalidPattern {
                    pattern: format!("<combined exclude patterns for {}>", tree.name),
                    source: e,
                })?;
            exclude_patterns.insert(tree.name.clone(), exclude_set);
        }

        Ok(Self {
            include_patterns,
            exclude_patterns,
        })
    }

    /// Checks if a path matches the patterns for a given tree.
    ///
    /// A file matches if it matches at least one include pattern
    /// and does not match any exclude pattern.
    ///
    /// The path should be relative to the tree root.
    /// Returns `false` if the tree has no patterns defined.
    pub fn matches(&self, tree: &str, path: &Path) -> bool {
        let includes = self
            .include_patterns
            .get(tree)
            .is_some_and(|p| p.is_match(path));
        let excludes = self
            .exclude_patterns
            .get(tree)
            .is_some_and(|p| p.is_match(path));

        includes && !excludes
    }

    /// Returns the names of all trees that have patterns.
    pub fn trees(&self) -> impl Iterator<Item = &str> {
        self.include_patterns.keys().map(String::as_str)
    }
}

/// Compiles a single glob pattern.
fn compile_glob(pattern: &str) -> Result<Glob, ConfigError> {
    Glob::new(pattern).map_err(|e| ConfigError::InvalidPattern {
        pattern: pattern.to_string(),
        source: e,
    })
}

/// A compiled context rule ready for matching.
#[derive(Debug)]
struct CompiledRule {
    /// Compiled glob matchers for this rule.
    matchers: Vec<GlobMatcher>,
    /// Trees to limit search to (empty means all trees).
    trees: Vec<String>,
    /// Terms to inject into the search query.
    terms: Vec<String>,
    /// Files to always include (tree:path format).
    include: Vec<String>,
}

/// Result of matching context rules against a file path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MatchedRules {
    /// Merged terms from all matching rules.
    pub terms: Vec<String>,
    /// Merged tree filter (intersection of all rules' trees; empty means all).
    pub trees: Vec<String>,
    /// Merged include paths from all matching rules.
    pub include: Vec<String>,
}

impl MatchedRules {
    /// Returns true if no rules matched.
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.trees.is_empty() && self.include.is_empty()
    }
}

/// Compiled context rules for file-pattern-based search customization.
///
/// When a file path matches rules, their effects are merged:
/// - **terms**: concatenated from all matching rules
/// - **trees**: intersection (if any rule specifies trees, only the intersection is searched)
/// - **include**: concatenated from all matching rules
#[derive(Debug)]
pub struct CompiledContextRules {
    /// Compiled rules in precedence order (higher precedence first).
    rules: Vec<CompiledRule>,
}

impl CompiledContextRules {
    /// Compiles context rules from settings.
    pub fn compile(settings: &ContextSettings) -> Result<Self, ConfigError> {
        let mut rules = Vec::with_capacity(settings.rules.len());

        for rule in &settings.rules {
            let matchers: Vec<GlobMatcher> = rule
                .patterns
                .iter()
                .map(|p| compile_glob(p).map(|g| g.compile_matcher()))
                .collect::<Result<Vec<_>, _>>()?;

            rules.push(CompiledRule {
                matchers,
                trees: rule.trees.clone(),
                terms: rule.terms.clone(),
                include: rule.include.clone(),
            });
        }

        Ok(Self { rules })
    }

    /// Matches a file path against all rules and returns merged results.
    ///
    /// Effects from all matching rules are merged:
    /// - terms: concatenated
    /// - trees: intersected (if any rule specifies trees)
    /// - include: concatenated
    pub fn match_rules(&self, path: &Path) -> MatchedRules {
        let mut terms = Vec::new();
        let mut include = Vec::new();
        let mut trees_sets: Vec<&[String]> = Vec::new();

        for rule in &self.rules {
            // A rule matches if any of its patterns match
            if rule.matchers.iter().any(|m| m.is_match(path)) {
                terms.extend(rule.terms.iter().cloned());
                include.extend(rule.include.iter().cloned());

                // Collect non-empty tree restrictions for intersection
                if !rule.trees.is_empty() {
                    trees_sets.push(&rule.trees);
                }
            }
        }

        // Compute tree intersection
        let trees = if trees_sets.is_empty() {
            // No rules specified trees - search all
            Vec::new()
        } else {
            // Start with the first set and intersect with the rest
            let mut result: Vec<String> = trees_sets[0].to_vec();
            for set in &trees_sets[1..] {
                result.retain(|t| set.contains(t));
            }
            result
        };

        MatchedRules {
            terms,
            trees,
            include,
        }
    }

    /// Returns all search terms that match a given file path.
    ///
    /// This is a convenience method that only returns terms.
    pub fn match_terms(&self, path: &Path) -> Vec<String> {
        self.match_rules(path).terms
    }

    /// Returns true if no rules are configured.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

/// Compiled glob patterns for context analysis.
///
/// Maps glob patterns to search terms for the `ra context` command.
/// When a file path matches a pattern, the associated terms are added
/// to the context search query.
///
/// **Deprecated**: Use `CompiledContextRules` instead for full rule support.
#[derive(Debug)]
pub struct CompiledContextPatterns {
    /// The underlying compiled rules.
    inner: CompiledContextRules,
}

impl CompiledContextPatterns {
    /// Compiles context patterns from settings.
    pub fn compile(settings: &ContextSettings) -> Result<Self, ConfigError> {
        Ok(Self {
            inner: CompiledContextRules::compile(settings)?,
        })
    }

    /// Returns all search terms that match a given file path.
    ///
    /// The path can be relative or absolute; matching is based on the filename
    /// and path components.
    pub fn match_terms(&self, path: &Path) -> Vec<String> {
        self.inner.match_terms(path)
    }

    /// Returns true if no patterns are configured.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn make_tree(name: &str, include: Vec<&str>, exclude: Vec<&str>) -> Tree {
        Tree {
            name: name.to_string(),
            path: PathBuf::from("/dummy"),
            is_global: false,
            include: include.into_iter().map(String::from).collect(),
            exclude: exclude.into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn test_compile_empty_patterns() {
        let patterns = CompiledPatterns::compile(&[]).unwrap();
        assert_eq!(patterns.trees().count(), 0);
    }

    #[test]
    fn test_compile_single_pattern() {
        let trees = vec![make_tree("docs", vec!["**/*.md"], vec![])];
        let patterns = CompiledPatterns::compile(&trees).unwrap();

        assert!(patterns.matches("docs", Path::new("readme.md")));
        assert!(patterns.matches("docs", Path::new("guide/intro.md")));
        assert!(!patterns.matches("docs", Path::new("code.rs")));
    }

    #[test]
    fn test_compile_multiple_include_patterns() {
        let trees = vec![make_tree("docs", vec!["**/*.md", "**/*.txt"], vec![])];
        let patterns = CompiledPatterns::compile(&trees).unwrap();

        assert!(patterns.matches("docs", Path::new("readme.md")));
        assert!(patterns.matches("docs", Path::new("notes.txt")));
        assert!(!patterns.matches("docs", Path::new("code.rs")));
    }

    #[test]
    fn test_compile_patterns_multiple_trees() {
        let trees = vec![
            make_tree("global", vec!["**/rust/**"], vec![]),
            make_tree("local", vec!["**/*.md"], vec![]),
        ];
        let patterns = CompiledPatterns::compile(&trees).unwrap();

        assert!(patterns.matches("global", Path::new("rust/guide.md")));
        assert!(patterns.matches("global", Path::new("docs/rust/errors.txt")));
        assert!(!patterns.matches("global", Path::new("python/guide.md")));

        assert!(patterns.matches("local", Path::new("readme.md")));
        assert!(!patterns.matches("local", Path::new("readme.txt")));
    }

    #[test]
    fn test_exclude_patterns() {
        let trees = vec![make_tree("docs", vec!["**/*.md"], vec!["**/drafts/**"])];
        let patterns = CompiledPatterns::compile(&trees).unwrap();

        assert!(patterns.matches("docs", Path::new("readme.md")));
        assert!(patterns.matches("docs", Path::new("guide/intro.md")));
        // Excluded by drafts pattern
        assert!(!patterns.matches("docs", Path::new("drafts/wip.md")));
        assert!(!patterns.matches("docs", Path::new("docs/drafts/new.md")));
    }

    #[test]
    fn test_exclude_takes_precedence() {
        // File matches both include and exclude - exclude wins
        let trees = vec![make_tree("docs", vec!["**/*.md"], vec!["secret.md"])];
        let patterns = CompiledPatterns::compile(&trees).unwrap();

        assert!(patterns.matches("docs", Path::new("readme.md")));
        assert!(!patterns.matches("docs", Path::new("secret.md")));
    }

    #[test]
    fn test_matches_unknown_tree() {
        let trees = vec![make_tree("docs", vec!["**/*.md"], vec![])];
        let patterns = CompiledPatterns::compile(&trees).unwrap();
        assert!(!patterns.matches("unknown", Path::new("readme.md")));
    }

    #[test]
    fn test_invalid_pattern_error() {
        let trees = vec![make_tree("docs", vec!["[invalid"], vec![])];
        let result = CompiledPatterns::compile(&trees);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigError::InvalidPattern { .. }
        ));
    }

    #[test]
    fn test_pattern_with_extensions() {
        let trees = vec![make_tree("docs", vec!["**/*.{md,txt,rst}"], vec![])];
        let patterns = CompiledPatterns::compile(&trees).unwrap();

        assert!(patterns.matches("docs", Path::new("readme.md")));
        assert!(patterns.matches("docs", Path::new("notes.txt")));
        assert!(patterns.matches("docs", Path::new("guide.rst")));
        assert!(!patterns.matches("docs", Path::new("code.rs")));
    }

    #[test]
    fn test_trees_iterator() {
        let trees = vec![
            make_tree("alpha", vec!["**/*.md"], vec![]),
            make_tree("beta", vec!["**/*.txt"], vec![]),
        ];
        let patterns = CompiledPatterns::compile(&trees).unwrap();

        let mut tree_names: Vec<_> = patterns.trees().collect();
        tree_names.sort();
        assert_eq!(tree_names, vec!["alpha", "beta"]);
    }

    mod context_patterns {
        use super::*;

        fn make_context_settings(rules: Vec<(Vec<&str>, Vec<&str>)>) -> crate::ContextSettings {
            let mut settings = crate::ContextSettings::default();
            for (patterns, terms) in rules {
                settings.rules.push(crate::ContextRule {
                    patterns: patterns.into_iter().map(String::from).collect(),
                    trees: Vec::new(),
                    terms: terms.into_iter().map(String::from).collect(),
                    include: Vec::new(),
                });
            }
            settings
        }

        #[test]
        fn test_compile_empty() {
            let settings = crate::ContextSettings::default();
            let patterns = CompiledContextPatterns::compile(&settings).unwrap();
            assert!(patterns.is_empty());
        }

        #[test]
        fn test_match_single_pattern() {
            let settings = make_context_settings(vec![(vec!["*.rs"], vec!["rust"])]);
            let patterns = CompiledContextPatterns::compile(&settings).unwrap();

            let terms = patterns.match_terms(Path::new("src/main.rs"));
            assert_eq!(terms, vec!["rust"]);
        }

        #[test]
        fn test_match_multiple_terms() {
            let settings =
                make_context_settings(vec![(vec!["*.tsx"], vec!["typescript", "react"])]);
            let patterns = CompiledContextPatterns::compile(&settings).unwrap();

            let terms = patterns.match_terms(Path::new("components/Button.tsx"));
            assert!(terms.contains(&"typescript".to_string()));
            assert!(terms.contains(&"react".to_string()));
        }

        #[test]
        fn test_match_multiple_rules() {
            let settings = make_context_settings(vec![
                (vec!["*.rs"], vec!["rust"]),
                (vec!["src/api/**"], vec!["http", "handlers"]),
            ]);
            let patterns = CompiledContextPatterns::compile(&settings).unwrap();

            // Matches only first rule
            let terms = patterns.match_terms(Path::new("lib.rs"));
            assert_eq!(terms, vec!["rust"]);

            // Matches both rules
            let terms = patterns.match_terms(Path::new("src/api/handlers.rs"));
            assert!(terms.contains(&"rust".to_string()));
            assert!(terms.contains(&"http".to_string()));
            assert!(terms.contains(&"handlers".to_string()));
        }

        #[test]
        fn test_match_multiple_patterns_in_rule() {
            // A single rule with multiple match patterns
            let settings = make_context_settings(vec![(vec!["*.tsx", "*.jsx"], vec!["react"])]);
            let patterns = CompiledContextPatterns::compile(&settings).unwrap();

            let terms = patterns.match_terms(Path::new("Button.tsx"));
            assert_eq!(terms, vec!["react"]);

            let terms = patterns.match_terms(Path::new("Button.jsx"));
            assert_eq!(terms, vec!["react"]);

            let terms = patterns.match_terms(Path::new("Button.ts"));
            assert!(terms.is_empty());
        }

        #[test]
        fn test_no_match() {
            let settings = make_context_settings(vec![(vec!["*.rs"], vec!["rust"])]);
            let patterns = CompiledContextPatterns::compile(&settings).unwrap();

            let terms = patterns.match_terms(Path::new("main.py"));
            assert!(terms.is_empty());
        }

        #[test]
        fn test_invalid_pattern() {
            let settings = make_context_settings(vec![(vec!["[invalid"], vec!["test"])]);
            let result = CompiledContextPatterns::compile(&settings);
            assert!(result.is_err());
        }
    }

    mod context_rules {
        use super::*;

        /// Helper to create a full context rule with all fields.
        fn make_rule(
            patterns: Vec<&str>,
            trees: Vec<&str>,
            terms: Vec<&str>,
            include: Vec<&str>,
        ) -> crate::ContextRule {
            crate::ContextRule {
                patterns: patterns.into_iter().map(String::from).collect(),
                trees: trees.into_iter().map(String::from).collect(),
                terms: terms.into_iter().map(String::from).collect(),
                include: include.into_iter().map(String::from).collect(),
            }
        }

        fn make_settings(rules: Vec<crate::ContextRule>) -> crate::ContextSettings {
            crate::ContextSettings {
                rules,
                ..Default::default()
            }
        }

        #[test]
        fn test_compile_empty() {
            let settings = crate::ContextSettings::default();
            let rules = CompiledContextRules::compile(&settings).unwrap();
            assert!(rules.is_empty());
        }

        #[test]
        fn test_match_terms_only() {
            let settings =
                make_settings(vec![make_rule(vec!["*.rs"], vec![], vec!["rust"], vec![])]);
            let rules = CompiledContextRules::compile(&settings).unwrap();

            let result = rules.match_rules(Path::new("main.rs"));
            assert_eq!(result.terms, vec!["rust"]);
            assert!(result.trees.is_empty());
            assert!(result.include.is_empty());
        }

        #[test]
        fn test_match_trees_only() {
            let settings =
                make_settings(vec![make_rule(vec!["*.rs"], vec!["docs"], vec![], vec![])]);
            let rules = CompiledContextRules::compile(&settings).unwrap();

            let result = rules.match_rules(Path::new("main.rs"));
            assert!(result.terms.is_empty());
            assert_eq!(result.trees, vec!["docs"]);
            assert!(result.include.is_empty());
        }

        #[test]
        fn test_match_include_only() {
            let settings = make_settings(vec![make_rule(
                vec!["*.rs"],
                vec![],
                vec![],
                vec!["docs:api/overview.md"],
            )]);
            let rules = CompiledContextRules::compile(&settings).unwrap();

            let result = rules.match_rules(Path::new("main.rs"));
            assert!(result.terms.is_empty());
            assert!(result.trees.is_empty());
            assert_eq!(result.include, vec!["docs:api/overview.md"]);
        }

        #[test]
        fn test_match_all_fields() {
            let settings = make_settings(vec![make_rule(
                vec!["*.rs"],
                vec!["docs", "examples"],
                vec!["rust", "systems"],
                vec!["docs:rust/overview.md"],
            )]);
            let rules = CompiledContextRules::compile(&settings).unwrap();

            let result = rules.match_rules(Path::new("main.rs"));
            assert_eq!(result.terms, vec!["rust", "systems"]);
            assert_eq!(result.trees, vec!["docs", "examples"]);
            assert_eq!(result.include, vec!["docs:rust/overview.md"]);
        }

        #[test]
        fn test_no_match() {
            let settings = make_settings(vec![make_rule(
                vec!["*.rs"],
                vec!["docs"],
                vec!["rust"],
                vec![],
            )]);
            let rules = CompiledContextRules::compile(&settings).unwrap();

            let result = rules.match_rules(Path::new("main.py"));
            assert!(result.is_empty());
        }

        #[test]
        fn test_merge_terms_concatenate() {
            let settings = make_settings(vec![
                make_rule(vec!["*.rs"], vec![], vec!["rust"], vec![]),
                make_rule(vec!["src/api/**"], vec![], vec!["http", "handlers"], vec![]),
            ]);
            let rules = CompiledContextRules::compile(&settings).unwrap();

            let result = rules.match_rules(Path::new("src/api/handler.rs"));
            assert_eq!(result.terms, vec!["rust", "http", "handlers"]);
        }

        #[test]
        fn test_merge_include_concatenate() {
            let settings = make_settings(vec![
                make_rule(vec!["*.rs"], vec![], vec![], vec!["docs:rust/guide.md"]),
                make_rule(
                    vec!["src/api/**"],
                    vec![],
                    vec![],
                    vec!["docs:api/overview.md"],
                ),
            ]);
            let rules = CompiledContextRules::compile(&settings).unwrap();

            let result = rules.match_rules(Path::new("src/api/handler.rs"));
            assert_eq!(
                result.include,
                vec!["docs:rust/guide.md", "docs:api/overview.md"]
            );
        }

        #[test]
        fn test_merge_trees_intersect() {
            let settings = make_settings(vec![
                make_rule(vec!["*.rs"], vec!["docs", "examples"], vec![], vec![]),
                make_rule(vec!["src/api/**"], vec!["docs", "api-docs"], vec![], vec![]),
            ]);
            let rules = CompiledContextRules::compile(&settings).unwrap();

            let result = rules.match_rules(Path::new("src/api/handler.rs"));
            // Intersection: ["docs", "examples"] ∩ ["docs", "api-docs"] = ["docs"]
            assert_eq!(result.trees, vec!["docs"]);
        }

        #[test]
        fn test_merge_trees_no_intersection() {
            let settings = make_settings(vec![
                make_rule(vec!["*.rs"], vec!["examples"], vec![], vec![]),
                make_rule(vec!["src/api/**"], vec!["api-docs"], vec![], vec![]),
            ]);
            let rules = CompiledContextRules::compile(&settings).unwrap();

            let result = rules.match_rules(Path::new("src/api/handler.rs"));
            // No common trees
            assert!(result.trees.is_empty());
        }

        #[test]
        fn test_merge_trees_one_rule_no_trees() {
            // If one rule doesn't specify trees, only the other's trees apply
            let settings = make_settings(vec![
                make_rule(vec!["*.rs"], vec![], vec!["rust"], vec![]),
                make_rule(vec!["src/api/**"], vec!["docs"], vec!["http"], vec![]),
            ]);
            let rules = CompiledContextRules::compile(&settings).unwrap();

            let result = rules.match_rules(Path::new("src/api/handler.rs"));
            // Only src/api/** specifies trees, so ["docs"] applies
            assert_eq!(result.trees, vec!["docs"]);
            assert_eq!(result.terms, vec!["rust", "http"]);
        }

        #[test]
        fn test_merge_trees_neither_specifies() {
            let settings = make_settings(vec![
                make_rule(vec!["*.rs"], vec![], vec!["rust"], vec![]),
                make_rule(vec!["src/api/**"], vec![], vec!["http"], vec![]),
            ]);
            let rules = CompiledContextRules::compile(&settings).unwrap();

            let result = rules.match_rules(Path::new("src/api/handler.rs"));
            // Neither specifies trees, so search all (empty)
            assert!(result.trees.is_empty());
        }

        #[test]
        fn test_multiple_match_patterns() {
            let settings = make_settings(vec![make_rule(
                vec!["*.tsx", "*.jsx"],
                vec![],
                vec!["react"],
                vec![],
            )]);
            let rules = CompiledContextRules::compile(&settings).unwrap();

            let result1 = rules.match_rules(Path::new("Button.tsx"));
            assert_eq!(result1.terms, vec!["react"]);

            let result2 = rules.match_rules(Path::new("Button.jsx"));
            assert_eq!(result2.terms, vec!["react"]);

            let result3 = rules.match_rules(Path::new("Button.ts"));
            assert!(result3.is_empty());
        }

        #[test]
        fn test_match_terms_convenience() {
            let settings = make_settings(vec![make_rule(
                vec!["*.rs"],
                vec!["docs"],
                vec!["rust"],
                vec!["docs:x.md"],
            )]);
            let rules = CompiledContextRules::compile(&settings).unwrap();

            // match_terms only returns terms
            let terms = rules.match_terms(Path::new("main.rs"));
            assert_eq!(terms, vec!["rust"]);
        }

        #[test]
        fn test_invalid_pattern() {
            let settings = make_settings(vec![make_rule(
                vec!["[invalid"],
                vec![],
                vec!["test"],
                vec![],
            )]);
            let result = CompiledContextRules::compile(&settings);
            assert!(result.is_err());
        }
    }
}
