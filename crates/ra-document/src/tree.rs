//! Chunk tree structure and operations.
//!
//! This module provides the `ChunkTree` type which represents a parsed markdown document
//! as a hierarchical tree of nodes. The tree supports traversal, node lookup, and
//! iteration over chunks (nodes with non-empty body text).

use std::path::{Path, PathBuf};

#[cfg(test)]
use crate::node::HeadingParams;
use crate::node::{Node, NodeKind};

/// A hierarchical tree of nodes representing a parsed document.
///
/// The tree has a single root (the document node) with heading nodes as descendants.
/// Nodes are arranged by heading depth, where each heading becomes a child of the
/// nearest preceding heading with strictly lower depth.
#[derive(Debug, Clone)]
pub struct ChunkTree {
    /// The root document node.
    root: Node,
    /// The original document content (for body extraction).
    content: String,
    /// Byte offset where the first heading line starts, if any.
    /// Used to compute the preamble even when all headings are filtered out.
    first_heading_start: Option<usize>,
}

impl ChunkTree {
    /// Creates a new chunk tree with the given root node and content.
    pub fn new(root: Node, content: String) -> Self {
        Self {
            root,
            content,
            first_heading_start: None,
        }
    }

    /// Creates a new chunk tree with a known first heading position.
    pub fn with_first_heading(root: Node, content: String, first_heading_start: usize) -> Self {
        Self {
            root,
            content,
            first_heading_start: Some(first_heading_start),
        }
    }

    /// Returns a reference to the root document node.
    pub fn root(&self) -> &Node {
        &self.root
    }

    /// Returns a mutable reference to the root document node.
    pub fn root_mut(&mut self) -> &mut Node {
        &mut self.root
    }

    /// Returns the original document content.
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Returns an iterator over all nodes in pre-order (depth-first) traversal.
    pub fn iter_preorder(&self) -> impl Iterator<Item = &Node> {
        self.root.iter_preorder()
    }

    /// Returns an iterator over all chunks (nodes with non-empty body text).
    ///
    /// A chunk is any node whose body (the span text excluding child spans)
    /// contains non-whitespace characters.
    pub fn iter_chunks(&self) -> impl Iterator<Item = &Node> {
        self.root.iter_preorder().filter(|node| self.has_body(node))
    }

    /// Looks up a node by its ID.
    ///
    /// Returns `None` if no node with the given ID exists in the tree.
    pub fn get_node(&self, id: &str) -> Option<&Node> {
        self.root.iter_preorder().find(|node| node.id == id)
    }

    /// Returns the total number of nodes in the tree.
    pub fn node_count(&self) -> usize {
        self.root.node_count()
    }

    /// Returns the number of chunks (nodes with non-empty body text).
    pub fn chunk_count(&self) -> usize {
        self.iter_chunks().count()
    }

    /// Extracts the body text for a node.
    ///
    /// The body is the text within the node's span that is not covered by any
    /// child node's span. For the document node, this is the preamble (content
    /// before the first heading). For heading nodes, this is content after the
    /// heading line minus child spans.
    pub fn body(&self, node: &Node) -> &str {
        self.body_impl(node, &self.content)
    }

    /// Checks if a node has non-empty body text (contains non-whitespace).
    pub fn has_body(&self, node: &Node) -> bool {
        !self.body(node).trim().is_empty()
    }

    /// Internal body extraction implementation.
    fn body_impl<'a>(&self, node: &Node, content: &'a str) -> &'a str {
        if node.children.is_empty() {
            // Leaf node: body is the entire span, unless this is the document node
            // and we know where the first heading starts (even if filtered out)
            if node.kind == NodeKind::Document
                && let Some(first_heading) = self.first_heading_start
            {
                // Document's preamble ends at the first heading line
                return &content[node.byte_start..first_heading];
            }
            &content[node.byte_start..node.byte_end]
        } else {
            // Non-leaf: body is span content before first child's heading line
            // Use heading_line_start to exclude the child's heading line from parent body
            let first_child_heading_start = node.children[0].heading_line_start;
            &content[node.byte_start..first_child_heading_start]
        }
    }

    /// Assigns position values to all nodes via pre-order traversal.
    ///
    /// This should be called after the tree structure is fully built.
    pub fn assign_positions(&mut self) {
        for (position, node) in self.root.iter_preorder_mut().enumerate() {
            node.position = position;
        }
    }

    /// Computes and assigns sibling_count for all nodes.
    ///
    /// This should be called after the tree structure is fully built.
    pub fn assign_sibling_counts(&mut self) {
        assign_sibling_counts_recursive(&mut self.root);
    }

    /// Extracts all chunks from the tree with their metadata.
    ///
    /// This produces `TreeChunk` structs ready for indexing, including
    /// body text extraction and breadcrumb generation.
    pub fn extract_chunks(&self, doc_title: &str) -> Vec<TreeChunk> {
        self.iter_chunks()
            .map(|node| {
                let breadcrumb = self.build_breadcrumb(node, doc_title);
                TreeChunk {
                    id: node.id.clone(),
                    doc_id: node.doc_id.clone(),
                    parent_id: node.parent_id.clone(),
                    title: node.title.clone(),
                    body: self.body(node).to_string(),
                    breadcrumb,
                    depth: node.depth,
                    position: node.position,
                    byte_start: node.byte_start,
                    byte_end: node.byte_end,
                    sibling_count: node.sibling_count,
                }
            })
            .collect()
    }

    /// Builds a breadcrumb string for a node.
    ///
    /// Format: `> Doc Title › Parent › Current`
    ///
    /// The document title always comes first. If the first h1 heading matches
    /// the document title, it's omitted to avoid duplication. The node's own
    /// title appears last (omitted for document-level chunks).
    fn build_breadcrumb(&self, node: &Node, doc_title: &str) -> String {
        let mut parts = vec![doc_title.to_string()];

        // Collect ancestor titles
        let ancestors = self.collect_ancestors(node);
        for ancestor in ancestors {
            // Skip if this is the first h1 and it matches the doc title
            if ancestor.depth == 1 && ancestor.title == doc_title {
                continue;
            }
            parts.push(ancestor.title.clone());
        }

        // Add the node's own title for non-document nodes
        if node.kind != NodeKind::Document {
            // Skip if this node's title matches doc title and it's h1
            if !(node.depth == 1 && node.title == doc_title) {
                parts.push(node.title.clone());
            }
        }

        format!("> {}", parts.join(" › "))
    }

    /// Collects ancestor nodes from root to parent (not including self).
    fn collect_ancestors(&self, node: &Node) -> Vec<&Node> {
        let mut ancestors = Vec::new();
        let mut current_id = node.parent_id.as_deref();

        while let Some(parent_id) = current_id {
            if let Some(parent) = self.get_node(parent_id) {
                // Don't include the document node in ancestors for breadcrumb
                if parent.kind != NodeKind::Document {
                    ancestors.push(parent);
                }
                current_id = parent.parent_id.as_deref();
            } else {
                break;
            }
        }

        ancestors.reverse();
        ancestors
    }
}

/// Recursively assigns sibling counts to all nodes in the tree.
fn assign_sibling_counts_recursive(node: &mut Node) {
    let child_count = node.children.len();
    for child in &mut node.children {
        child.sibling_count = child_count;
        assign_sibling_counts_recursive(child);
    }
}

/// A chunk extracted from the tree, ready for indexing.
///
/// This struct contains all the metadata needed for indexing and search-time
/// aggregation, including the extracted body text.
#[derive(Debug, Clone)]
pub struct TreeChunk {
    /// Globally unique chunk identifier.
    pub id: String,
    /// Document identifier (same for all chunks in a file).
    pub doc_id: String,
    /// Parent node's ID, or None for the document node.
    pub parent_id: Option<String>,
    /// The chunk title (document title or heading text).
    pub title: String,
    /// The chunk body text (content within span, excluding child spans).
    pub body: String,
    /// Breadcrumb showing hierarchy path.
    pub breadcrumb: String,
    /// Hierarchy depth (0 for document, 1-6 for headings).
    pub depth: u8,
    /// Document order index (0-based, pre-order traversal).
    pub position: usize,
    /// Byte offset where this chunk's span starts.
    pub byte_start: usize,
    /// Byte offset where this chunk's span ends (exclusive).
    pub byte_end: usize,
    /// Number of siblings including this chunk.
    pub sibling_count: usize,
}

/// Builder for constructing chunk trees from parsed heading data.
pub struct ChunkTreeBuilder {
    /// The tree name (collection identifier).
    tree_name: String,
    /// The document path within the tree.
    path: PathBuf,
}

impl ChunkTreeBuilder {
    /// Creates a new tree builder for the given tree name and document path.
    pub fn new(tree_name: impl Into<String>, path: impl AsRef<Path>) -> Self {
        Self {
            tree_name: tree_name.into(),
            path: path.as_ref().to_path_buf(),
        }
    }

    /// Returns the tree name.
    pub fn tree_name(&self) -> &str {
        &self.tree_name
    }

    /// Returns the document path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
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

    fn make_test_tree() -> ChunkTree {
        // Create a simple tree structure:
        // Doc (0-1000)
        //   ├── H1 (heading at 95, span 100-600)
        //   │   ├── H2a (heading at 145, span 150-300)
        //   │   └── H2b (heading at 295, span 300-600)
        //   └── H1b (heading at 595, span 600-1000)
        let path = PathBuf::from("test.md");

        let content = "preamble content here\n".to_string()
            + &" ".repeat(73) // Pad to 95 bytes (before h1 heading line)
            + "# H1\n" // 5 bytes (heading line at 95-100)
            + &" ".repeat(45) // Pad to 150 (before h2a heading line)
            + "## H2a\n" // 7 bytes (heading line at 145-152, but span starts at 150)
            + &" ".repeat(141) // Adjust padding to reach 300
            + "## H2b\n" // H2b heading at 295
            + &" ".repeat(287) // Pad to 600
            + "# H1b\n" // H1b heading at 595
            + &" ".repeat(394); // Pad to 1000 bytes

        let mut root = Node::document("test", &path, "Doc".to_string(), 1000);

        let mut h1 = Node::heading(
            "test",
            &path,
            heading_params(&root.id, 1, "H1", "h1", 95, 100, 600),
        );

        let h2a = Node::heading(
            "test",
            &path,
            heading_params(&h1.id, 2, "H2a", "h2a", 145, 150, 300),
        );

        let h2b = Node::heading(
            "test",
            &path,
            heading_params(&h1.id, 2, "H2b", "h2b", 295, 300, 600),
        );

        h1.children.push(h2a);
        h1.children.push(h2b);

        let h1b = Node::heading(
            "test",
            &path,
            heading_params(&root.id, 1, "H1b", "h1b", 595, 600, 1000),
        );

        root.children.push(h1);
        root.children.push(h1b);

        let mut tree = ChunkTree::new(root, content);
        tree.assign_positions();
        tree.assign_sibling_counts();
        tree
    }

    #[test]
    fn test_tree_creation() {
        let tree = make_test_tree();
        assert_eq!(tree.root().title, "Doc");
        assert_eq!(tree.root().depth, 0);
        assert_eq!(tree.node_count(), 5);
    }

    #[test]
    fn test_iter_preorder() {
        let tree = make_test_tree();
        let titles: Vec<&str> = tree.iter_preorder().map(|n| n.title.as_str()).collect();
        assert_eq!(titles, vec!["Doc", "H1", "H2a", "H2b", "H1b"]);
    }

    #[test]
    fn test_get_node() {
        let tree = make_test_tree();

        let doc = tree.get_node("test:test.md");
        assert!(doc.is_some());
        assert_eq!(doc.unwrap().title, "Doc");

        let h2a = tree.get_node("test:test.md#h2a");
        assert!(h2a.is_some());
        assert_eq!(h2a.unwrap().title, "H2a");

        let missing = tree.get_node("test:test.md#nonexistent");
        assert!(missing.is_none());
    }

    #[test]
    fn test_positions() {
        let tree = make_test_tree();
        let positions: Vec<usize> = tree.iter_preorder().map(|n| n.position).collect();
        assert_eq!(positions, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn test_sibling_counts() {
        let tree = make_test_tree();

        // Document has sibling_count = 1
        assert_eq!(tree.root().sibling_count, 1);

        // H1 and H1b are siblings (count = 2)
        let h1 = tree.get_node("test:test.md#h1").unwrap();
        assert_eq!(h1.sibling_count, 2);

        let h1b = tree.get_node("test:test.md#h1b").unwrap();
        assert_eq!(h1b.sibling_count, 2);

        // H2a and H2b are siblings (count = 2)
        let h2a = tree.get_node("test:test.md#h2a").unwrap();
        assert_eq!(h2a.sibling_count, 2);
    }

    #[test]
    fn test_body_extraction_leaf() {
        let path = PathBuf::from("test.md");
        let content = "Hello world content";
        let root = Node::document("test", &path, "Doc".to_string(), content.len());
        let tree = ChunkTree::new(root, content.to_string());

        assert_eq!(tree.body(tree.root()), "Hello world content");
    }

    #[test]
    fn test_body_extraction_with_children() {
        let path = PathBuf::from("test.md");
        // Content: "preamble\n# Child\nchild content"
        // Heading line starts at 9 (at "#"), span starts at 17 (after "# Child\n")
        let content = "preamble\n# Child\nchild content";
        let mut root = Node::document("test", &path, "Doc".to_string(), content.len());

        let child = Node::heading(
            "test",
            &path,
            heading_params(&root.id, 1, "Child", "child", 9, 17, content.len()),
        );
        root.children.push(child);

        let tree = ChunkTree::new(root, content.to_string());

        // Document body is preamble (content before first child's heading line)
        assert_eq!(tree.body(tree.root()), "preamble\n");

        // Child body is its full span (it's a leaf)
        let child_node = tree.get_node("test:test.md#child").unwrap();
        assert_eq!(tree.body(child_node), "child content");
    }

    #[test]
    fn test_has_body() {
        let path = PathBuf::from("test.md");
        let content = "   \n\t  ";
        let root = Node::document("test", &path, "Doc".to_string(), content.len());
        let tree = ChunkTree::new(root, content.to_string());

        assert!(!tree.has_body(tree.root()));

        let content2 = "Hello";
        let root2 = Node::document("test", &path, "Doc".to_string(), content2.len());
        let tree2 = ChunkTree::new(root2, content2.to_string());

        assert!(tree2.has_body(tree2.root()));
    }

    #[test]
    fn test_breadcrumb_simple() {
        let path = PathBuf::from("test.md");
        // Content structure: "preamble\n# Section\nheading content"
        let content = "preamble\n# Section\nheading content";
        let mut root = Node::document("test", &path, "My Doc".to_string(), content.len());

        let h1 = Node::heading(
            "test",
            &path,
            heading_params(&root.id, 1, "Section", "section", 9, 20, content.len()),
        );
        root.children.push(h1);

        let tree = ChunkTree::new(root, content.to_string());

        // Document breadcrumb
        assert_eq!(tree.build_breadcrumb(tree.root(), "My Doc"), "> My Doc");

        // Heading breadcrumb
        let h1_node = tree.get_node("test:test.md#section").unwrap();
        assert_eq!(
            tree.build_breadcrumb(h1_node, "My Doc"),
            "> My Doc › Section"
        );
    }

    #[test]
    fn test_breadcrumb_nested() {
        let path = PathBuf::from("test.md");
        let content = "x".repeat(300);
        let mut root = Node::document("test", &path, "Doc".to_string(), 300);

        let mut h1 = Node::heading(
            "test",
            &path,
            heading_params(&root.id, 1, "Parent", "parent", 0, 10, 200),
        );

        let h2 = Node::heading(
            "test",
            &path,
            heading_params(&h1.id, 2, "Child", "child", 40, 50, 200),
        );
        h1.children.push(h2);
        root.children.push(h1);

        let tree = ChunkTree::new(root, content);

        let h2_node = tree.get_node("test:test.md#child").unwrap();
        assert_eq!(
            tree.build_breadcrumb(h2_node, "Doc"),
            "> Doc › Parent › Child"
        );
    }

    #[test]
    fn test_breadcrumb_dedup_h1() {
        // When h1 title matches doc title, it should be omitted
        let path = PathBuf::from("test.md");
        let content = "x".repeat(200);
        let mut root = Node::document("test", &path, "My Title".to_string(), 200);

        let mut h1 = Node::heading(
            "test",
            &path,
            heading_params(&root.id, 1, "My Title", "my-title", 0, 10, 200), // Same as doc title
        );

        let h2 = Node::heading(
            "test",
            &path,
            heading_params(&h1.id, 2, "Details", "details", 40, 50, 200),
        );
        h1.children.push(h2);
        root.children.push(h1);

        let tree = ChunkTree::new(root, content);

        // H1 with same title as doc should just show doc title
        let h1_node = tree.get_node("test:test.md#my-title").unwrap();
        assert_eq!(tree.build_breadcrumb(h1_node, "My Title"), "> My Title");

        // H2 under duplicate H1 should skip the H1
        let h2_node = tree.get_node("test:test.md#details").unwrap();
        assert_eq!(
            tree.build_breadcrumb(h2_node, "My Title"),
            "> My Title › Details"
        );
    }

    #[test]
    fn test_extract_chunks() {
        let path = PathBuf::from("test.md");
        // Content: "preamble content\n# Section\nheading content here"
        // Heading line at 17, span starts at 27 (after "# Section\n")
        let content = "preamble content\n# Section\nheading content here";
        let mut root = Node::document("test", &path, "Doc".to_string(), content.len());

        let h1 = Node::heading(
            "test",
            &path,
            heading_params(&root.id, 1, "Section", "section", 17, 27, content.len()),
        );
        root.children.push(h1);

        let mut tree = ChunkTree::new(root, content.to_string());
        tree.assign_positions();
        tree.assign_sibling_counts();

        let chunks = tree.extract_chunks("Doc");
        assert_eq!(chunks.len(), 2);

        // Document chunk (preamble)
        assert_eq!(chunks[0].id, "test:test.md");
        assert_eq!(chunks[0].title, "Doc");
        assert_eq!(chunks[0].body, "preamble content\n");
        assert_eq!(chunks[0].depth, 0);
        assert_eq!(chunks[0].position, 0);

        // Heading chunk
        assert_eq!(chunks[1].id, "test:test.md#section");
        assert_eq!(chunks[1].title, "Section");
        assert_eq!(chunks[1].body, "heading content here");
        assert_eq!(chunks[1].depth, 1);
        assert_eq!(chunks[1].position, 1);
    }

    #[test]
    fn test_chunk_tree_builder() {
        let builder = ChunkTreeBuilder::new("docs", "guides/auth.md");
        assert_eq!(builder.tree_name(), "docs");
        assert_eq!(builder.path(), Path::new("guides/auth.md"));
    }
}
