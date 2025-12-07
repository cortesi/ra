// TODO: Remove allow(dead_code) once adaptive aggregation uses these functions
#![allow(dead_code)]

//! Chunk hierarchy utilities for ancestry detection.
//!
//! This module provides functions for working with chunk ID hierarchies,
//! particularly for detecting ancestor/descendant relationships between chunks.
//!
//! # Chunk ID Format
//!
//! Chunk IDs follow the format `{tree}:{path}#{slug}` where:
//! - `{tree}:{path}` is the document ID (same for all chunks in a file)
//! - `#{slug}` identifies the specific section within the document
//! - Nested slugs use `-` separators (e.g., `#error-handling-retry-logic`)
//!
//! # Ancestry
//!
//! A chunk is an ancestor of another if:
//! - They share the same document ID (`{tree}:{path}`)
//! - The ancestor's slug is a prefix of the descendant's slug
//!
//! The document node (ID without `#`) is an ancestor of all chunks in that document.

/// Checks if `ancestor_id` is an ancestor of `descendant_id`.
///
/// Returns `true` if:
/// - Both IDs refer to the same document (same `{tree}:{path}` prefix)
/// - AND either:
///   - `ancestor_id` is the document node (no `#`), OR
///   - `ancestor_id`'s slug is a prefix of `descendant_id`'s slug followed by `-`
///
/// # Examples
///
/// ```ignore
/// // Document node is ancestor of all chunks in that document
/// is_ancestor_of("local:doc.md", "local:doc.md#intro") // true
/// is_ancestor_of("local:doc.md", "local:doc.md#intro-details") // true
///
/// // Section is ancestor of nested sections
/// is_ancestor_of("local:doc.md#intro", "local:doc.md#intro-details") // true
/// is_ancestor_of("local:doc.md#intro", "local:doc.md#intro-details-more") // true
///
/// // Not ancestors
/// is_ancestor_of("local:doc.md#intro", "local:doc.md#intro") // false (same ID)
/// is_ancestor_of("local:doc.md#intro", "local:doc.md#other") // false (different branch)
/// is_ancestor_of("local:doc.md#intro-details", "local:doc.md#intro") // false (reversed)
/// is_ancestor_of("local:a.md", "local:b.md#intro") // false (different documents)
/// ```
pub fn is_ancestor_of(ancestor_id: &str, descendant_id: &str) -> bool {
    // Same ID is not an ancestor relationship
    if ancestor_id == descendant_id {
        return false;
    }

    // Split into document ID and slug parts
    let (ancestor_doc, ancestor_slug) = split_id(ancestor_id);
    let (descendant_doc, descendant_slug) = split_id(descendant_id);

    // Must be in the same document
    if ancestor_doc != descendant_doc {
        return false;
    }

    // If ancestor is the document node (no slug), it's an ancestor of all chunks
    let Some(ancestor_slug) = ancestor_slug else {
        return descendant_slug.is_some();
    };

    // Descendant must have a slug
    let Some(descendant_slug) = descendant_slug else {
        return false;
    };

    // Ancestor's slug must be a prefix of descendant's slug, followed by "-"
    // This ensures "intro" matches "intro-details" but not "introduction"
    if descendant_slug.len() > ancestor_slug.len() {
        descendant_slug.starts_with(ancestor_slug)
            && descendant_slug.as_bytes().get(ancestor_slug.len()) == Some(&b'-')
    } else {
        false
    }
}

/// Checks if `descendant_id` is a descendant of `ancestor_id`.
///
/// This is the inverse of [`is_ancestor_of`].
#[inline]
pub fn is_descendant_of(descendant_id: &str, ancestor_id: &str) -> bool {
    is_ancestor_of(ancestor_id, descendant_id)
}

/// Splits a chunk ID into document ID and optional slug.
///
/// Returns `(doc_id, Some(slug))` for chunk IDs with `#`, or `(id, None)` for document IDs.
fn split_id(id: &str) -> (&str, Option<&str>) {
    if let Some(hash_pos) = id.find('#') {
        (&id[..hash_pos], Some(&id[hash_pos + 1..]))
    } else {
        (id, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_is_ancestor_of_chunks() {
        assert!(is_ancestor_of("local:doc.md", "local:doc.md#intro"));
        assert!(is_ancestor_of("local:doc.md", "local:doc.md#intro-details"));
        assert!(is_ancestor_of("local:doc.md", "local:doc.md#other-section"));
    }

    #[test]
    fn section_is_ancestor_of_nested_sections() {
        assert!(is_ancestor_of(
            "local:doc.md#intro",
            "local:doc.md#intro-details"
        ));
        assert!(is_ancestor_of(
            "local:doc.md#intro",
            "local:doc.md#intro-details-more"
        ));
        assert!(is_ancestor_of(
            "local:doc.md#intro-details",
            "local:doc.md#intro-details-more"
        ));
    }

    #[test]
    fn same_id_is_not_ancestor() {
        assert!(!is_ancestor_of("local:doc.md", "local:doc.md"));
        assert!(!is_ancestor_of("local:doc.md#intro", "local:doc.md#intro"));
    }

    #[test]
    fn different_documents_not_ancestors() {
        assert!(!is_ancestor_of("local:a.md", "local:b.md#intro"));
        assert!(!is_ancestor_of("local:a.md#intro", "local:b.md#intro"));
        assert!(!is_ancestor_of("tree-a:doc.md", "tree-b:doc.md#intro"));
    }

    #[test]
    fn different_branches_not_ancestors() {
        assert!(!is_ancestor_of("local:doc.md#intro", "local:doc.md#other"));
        assert!(!is_ancestor_of(
            "local:doc.md#intro",
            "local:doc.md#other-section"
        ));
        assert!(!is_ancestor_of(
            "local:doc.md#intro-a",
            "local:doc.md#intro-b"
        ));
    }

    #[test]
    fn reversed_order_not_ancestor() {
        assert!(!is_ancestor_of(
            "local:doc.md#intro-details",
            "local:doc.md#intro"
        ));
        assert!(!is_ancestor_of("local:doc.md#intro", "local:doc.md"));
    }

    #[test]
    fn partial_slug_match_not_ancestor() {
        // "intro" should not match "introduction" (no dash separator)
        assert!(!is_ancestor_of(
            "local:doc.md#intro",
            "local:doc.md#introduction"
        ));
        // "err" should not match "error" (no dash separator)
        assert!(!is_ancestor_of("local:doc.md#err", "local:doc.md#error"));
        // But "error" should match "error-handling" since there's a dash
        assert!(is_ancestor_of(
            "local:doc.md#error",
            "local:doc.md#error-handling"
        ));
    }

    #[test]
    fn is_descendant_of_inverse() {
        assert!(is_descendant_of(
            "local:doc.md#intro-details",
            "local:doc.md#intro"
        ));
        assert!(is_descendant_of("local:doc.md#intro", "local:doc.md"));
        assert!(!is_descendant_of(
            "local:doc.md#intro",
            "local:doc.md#intro"
        ));
        assert!(!is_descendant_of(
            "local:doc.md#intro",
            "local:doc.md#intro-details"
        ));
    }

    #[test]
    fn complex_paths() {
        assert!(is_ancestor_of(
            "docs:api/handlers.md",
            "docs:api/handlers.md#error-handling"
        ));
        assert!(is_ancestor_of(
            "docs:api/handlers.md#error-handling",
            "docs:api/handlers.md#error-handling-retry"
        ));
    }

    #[test]
    fn empty_slug() {
        // Edge case: ID ending with # (empty slug) - treated as having a slug
        // Document is ancestor of chunk with empty slug (it has a slug, just empty)
        assert!(is_ancestor_of("local:doc.md", "local:doc.md#"));
        // Empty slug is not ancestor of other chunks (empty string is not prefix of "intro")
        assert!(!is_ancestor_of("local:doc.md#", "local:doc.md#intro"));
    }

    #[test]
    fn split_id_works() {
        assert_eq!(split_id("local:doc.md"), ("local:doc.md", None));
        assert_eq!(
            split_id("local:doc.md#intro"),
            ("local:doc.md", Some("intro"))
        );
        assert_eq!(
            split_id("local:doc.md#intro-details"),
            ("local:doc.md", Some("intro-details"))
        );
        assert_eq!(split_id("local:doc.md#"), ("local:doc.md", Some("")));
    }
}
