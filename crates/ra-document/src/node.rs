//! Hierarchical node structures for the chunk tree.
//!
//! This module defines the core data structures for representing markdown documents as
//! hierarchical trees of nodes. Each node represents either the document itself or a
//! heading section, with parent-child relationships determined by heading depth.

use std::path::Path;

/// Distinguishes document-level nodes from heading nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// The root node representing the entire document.
    Document,
    /// A heading node (h1-h6) within the document.
    Heading,
}

/// Parameters for creating a heading node.
#[derive(Debug, Clone)]
pub struct HeadingParams {
    /// The parent node's ID.
    pub parent_id: String,
    /// Hierarchy depth (1-6 for h1-h6).
    pub depth: u8,
    /// The heading text.
    pub title: String,
    /// The slug for fragment identifiers.
    pub slug: String,
    /// Byte offset where the heading line starts.
    pub heading_line_start: usize,
    /// Byte offset where the content span starts (after heading line).
    pub byte_start: usize,
    /// Byte offset where the content span ends (exclusive).
    pub byte_end: usize,
}

/// A node in the hierarchical chunk tree.
///
/// Nodes form a tree structure where:
/// - The root is always a document node (depth 0)
/// - Heading nodes (h1-h6) are children of the nearest preceding heading with lower depth
/// - Byte spans define the content range, with body text derived by excluding child spans
#[derive(Debug, Clone)]
pub struct Node {
    /// Globally unique chunk identifier.
    /// - Document node: `{tree}:{path}`
    /// - Heading node: `{tree}:{path}#{slug}`
    pub id: String,

    /// Document identifier, same for all nodes in a file: `{tree}:{path}`.
    pub doc_id: String,

    /// The parent node's `id`, or `None` for the document node.
    pub parent_id: Option<String>,

    /// Hierarchy depth: 0 for document, 1-6 for h1-h6.
    pub depth: u8,

    /// Document order index (0-based), assigned via pre-order traversal.
    pub position: usize,

    /// The node title (document title or heading text).
    pub title: String,

    /// Fragment identifier for heading nodes, `None` for document nodes.
    pub slug: Option<String>,

    /// Byte offset where the heading line starts (for heading nodes).
    /// For document nodes, this is always 0.
    /// This is used to compute parent body boundaries (preamble ends here).
    pub heading_line_start: usize,

    /// Byte offset where this node's content span starts in the source document.
    /// For heading nodes, this is after the heading line.
    /// For document nodes, this is always 0.
    pub byte_start: usize,

    /// Byte offset where this node's span ends (exclusive) in the source document.
    pub byte_end: usize,

    /// Number of siblings including this node under the same parent.
    /// For document nodes, this is always 1.
    pub sibling_count: usize,

    /// The kind of node (document or heading).
    pub kind: NodeKind,

    /// Child nodes in document order.
    pub children: Vec<Self>,
}

impl Node {
    /// Creates a new document node (root of the tree).
    pub fn document(tree: &str, path: &Path, title: String, content_len: usize) -> Self {
        let doc_id = make_doc_id(tree, path);
        Self {
            id: doc_id.clone(),
            doc_id,
            parent_id: None,
            depth: 0,
            position: 0,
            title,
            slug: None,
            heading_line_start: 0,
            byte_start: 0,
            byte_end: content_len,
            sibling_count: 1,
            kind: NodeKind::Document,
            children: Vec::new(),
        }
    }

    /// Creates a new heading node.
    pub fn heading(tree: &str, path: &Path, params: HeadingParams) -> Self {
        let doc_id = make_doc_id(tree, path);
        let id = make_chunk_id(tree, path, Some(&params.slug));
        Self {
            id,
            doc_id,
            parent_id: Some(params.parent_id),
            depth: params.depth,
            position: 0, // Assigned later during tree construction
            title: params.title,
            slug: Some(params.slug),
            heading_line_start: params.heading_line_start,
            byte_start: params.byte_start,
            byte_end: params.byte_end,
            sibling_count: 0, // Assigned later during tree construction
            kind: NodeKind::Heading,
            children: Vec::new(),
        }
    }

    /// Returns an iterator over this node and all descendants in pre-order (depth-first).
    pub fn iter_preorder(&self) -> PreorderIter<'_> {
        PreorderIter { stack: vec![self] }
    }

    /// Returns an iterator over this node and all descendants in pre-order (depth-first),
    /// yielding mutable references.
    pub fn iter_preorder_mut(&mut self) -> PreorderIterMut<'_> {
        PreorderIterMut { stack: vec![self] }
    }

    /// Returns the total number of nodes in this subtree (including self).
    pub fn node_count(&self) -> usize {
        1 + self.children.iter().map(|c| c.node_count()).sum::<usize>()
    }

    /// Returns true if this node has no children.
    pub fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }
}

/// Iterator for pre-order traversal of nodes.
pub struct PreorderIter<'a> {
    /// Stack of nodes to visit (rightmost children pushed first).
    stack: Vec<&'a Node>,
}

impl<'a> Iterator for PreorderIter<'a> {
    type Item = &'a Node;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.stack.pop()?;
        // Push children in reverse order so leftmost child is processed first
        for child in node.children.iter().rev() {
            self.stack.push(child);
        }
        Some(node)
    }
}

/// Iterator for mutable pre-order traversal of nodes.
pub struct PreorderIterMut<'a> {
    /// Stack of nodes to visit (rightmost children pushed first).
    stack: Vec<&'a mut Node>,
}

impl<'a> Iterator for PreorderIterMut<'a> {
    type Item = &'a mut Node;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.stack.pop()?;
        // Safety: We need to get mutable references to children while holding a mutable ref
        // to the node. This is safe because we're moving the node reference out before
        // accessing children.
        let node_ptr = node as *mut Node;
        // Push children in reverse order so leftmost child is processed first
        unsafe {
            for child in (*node_ptr).children.iter_mut().rev() {
                self.stack.push(child);
            }
        }
        Some(node)
    }
}

/// Constructs a document ID from tree name and path.
///
/// Format: `{tree}:{path}`
pub fn make_doc_id(tree: &str, path: &Path) -> String {
    format!("{}:{}", tree, path.display())
}

/// Constructs a chunk ID from tree name, path, and optional slug.
///
/// - With slug: `{tree}:{path}#{slug}`
/// - Without slug: `{tree}:{path}`
pub fn make_chunk_id(tree: &str, path: &Path, slug: Option<&str>) -> String {
    match slug {
        Some(s) => format!("{}:{}#{}", tree, path.display(), s),
        None => make_doc_id(tree, path),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn heading_params(
        parent_id: &str,
        depth: u8,
        title: &str,
        slug: &str,
        heading_line_start: usize,
        byte_start: usize,
        byte_end: usize,
    ) -> HeadingParams {
        HeadingParams {
            parent_id: parent_id.to_string(),
            depth,
            title: title.to_string(),
            slug: slug.to_string(),
            heading_line_start,
            byte_start,
            byte_end,
        }
    }

    #[test]
    fn test_make_doc_id() {
        let path = PathBuf::from("guides/auth.md");
        assert_eq!(make_doc_id("docs", &path), "docs:guides/auth.md");
    }

    #[test]
    fn test_make_chunk_id_with_slug() {
        let path = PathBuf::from("guides/auth.md");
        assert_eq!(
            make_chunk_id("docs", &path, Some("oauth-setup")),
            "docs:guides/auth.md#oauth-setup"
        );
    }

    #[test]
    fn test_make_chunk_id_without_slug() {
        let path = PathBuf::from("guides/auth.md");
        assert_eq!(make_chunk_id("docs", &path, None), "docs:guides/auth.md");
    }

    #[test]
    fn test_document_node_creation() {
        let path = PathBuf::from("guide.md");
        let node = Node::document("docs", &path, "My Guide".to_string(), 1000);

        assert_eq!(node.id, "docs:guide.md");
        assert_eq!(node.doc_id, "docs:guide.md");
        assert!(node.parent_id.is_none());
        assert_eq!(node.depth, 0);
        assert_eq!(node.title, "My Guide");
        assert!(node.slug.is_none());
        assert_eq!(node.byte_start, 0);
        assert_eq!(node.byte_end, 1000);
        assert_eq!(node.sibling_count, 1);
        assert_eq!(node.kind, NodeKind::Document);
        assert!(node.children.is_empty());
    }

    #[test]
    fn test_heading_node_creation() {
        let path = PathBuf::from("guide.md");
        let node = Node::heading(
            "docs",
            &path,
            heading_params(
                "docs:guide.md",
                1,
                "Introduction",
                "introduction",
                40,
                50,
                200,
            ),
        );

        assert_eq!(node.id, "docs:guide.md#introduction");
        assert_eq!(node.doc_id, "docs:guide.md");
        assert_eq!(node.parent_id, Some("docs:guide.md".to_string()));
        assert_eq!(node.depth, 1);
        assert_eq!(node.title, "Introduction");
        assert_eq!(node.slug, Some("introduction".to_string()));
        assert_eq!(node.heading_line_start, 40);
        assert_eq!(node.byte_start, 50);
        assert_eq!(node.byte_end, 200);
        assert_eq!(node.kind, NodeKind::Heading);
    }

    #[test]
    fn test_preorder_traversal() {
        let path = PathBuf::from("doc.md");
        let mut root = Node::document("test", &path, "Doc".to_string(), 1000);

        let mut h1 = Node::heading(
            "test",
            &path,
            heading_params(&root.id, 1, "H1", "h1", 0, 10, 500),
        );

        let h2a = Node::heading(
            "test",
            &path,
            heading_params(&h1.id, 2, "H2a", "h2a", 10, 50, 200),
        );

        let h2b = Node::heading(
            "test",
            &path,
            heading_params(&h1.id, 2, "H2b", "h2b", 200, 210, 500),
        );

        h1.children.push(h2a);
        h1.children.push(h2b);
        root.children.push(h1);

        let titles: Vec<&str> = root.iter_preorder().map(|n| n.title.as_str()).collect();
        assert_eq!(titles, vec!["Doc", "H1", "H2a", "H2b"]);
    }

    #[test]
    fn test_node_count() {
        let path = PathBuf::from("doc.md");
        let mut root = Node::document("test", &path, "Doc".to_string(), 1000);

        let mut h1 = Node::heading(
            "test",
            &path,
            heading_params(&root.id, 1, "H1", "h1", 0, 10, 500),
        );

        let h2 = Node::heading(
            "test",
            &path,
            heading_params(&h1.id, 2, "H2", "h2", 10, 50, 200),
        );

        h1.children.push(h2);
        root.children.push(h1);

        assert_eq!(root.node_count(), 3);
    }

    #[test]
    fn test_is_leaf() {
        let path = PathBuf::from("doc.md");
        let mut root = Node::document("test", &path, "Doc".to_string(), 1000);

        let h1 = Node::heading(
            "test",
            &path,
            heading_params(&root.id, 1, "H1", "h1", 0, 10, 500),
        );

        assert!(h1.is_leaf());
        root.children.push(h1);
        assert!(!root.is_leaf());
    }
}
