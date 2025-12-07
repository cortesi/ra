//! Index schema definition for the ra search index.
//!
//! Defines the Tantivy schema with all fields needed for chunk indexing:
//! - `id`: Unique chunk identifier (stored only)
//! - `doc_id`: Document identifier (stored)
//! - `parent_id`: Parent chunk identifier (stored, optional)
//! - `hierarchy`: Hierarchy path as multi-value text (text, stored)
//! - `tags`: Document tags (text, stored)
//! - `path`: File path within tree (text, stored)
//! - `tree`: Tree name (string, stored, fast)
//! - `body`: Chunk content (text, stored)
//! - `depth`: Heading level (u64, stored, fast) - 0 for document, 1-6 for h1-h6
//! - `position`: Document order index (u64, stored, indexed)
//! - `byte_start`: Content span start (u64, stored)
//! - `byte_end`: Content span end (u64, stored)
//! - `sibling_count`: Number of siblings (u64, stored)
//! - `mtime`: File modification time (date, indexed, fast)

use tantivy::schema::{
    DateOptions, FAST, Field, INDEXED, IndexRecordOption, STORED, STRING, Schema,
    TextFieldIndexing, TextOptions,
};

use crate::analyzer::RA_TOKENIZER;

/// Handles to all fields in the index schema.
#[derive(Debug, Clone)]
pub struct IndexSchema {
    /// The underlying Tantivy schema.
    schema: Schema,
    /// Unique chunk identifier: `{tree}:{path}#{slug}`.
    pub id: Field,
    /// Document identifier: `{tree}:{path}` (same for all chunks in a file).
    pub doc_id: Field,
    /// Parent chunk identifier, or empty for document nodes.
    pub parent_id: Field,
    /// Hierarchy path as multi-value text field.
    /// Each value is a title in the path from document root to this chunk.
    pub hierarchy: Field,
    /// Document tags from frontmatter.
    pub tags: Field,
    /// File path within the tree.
    pub path: Field,
    /// Tree name this chunk belongs to.
    pub tree: Field,
    /// Chunk body content.
    pub body: Field,
    /// Heading level: 0 for document node, 1-6 for h1-h6.
    pub depth: Field,
    /// Document order index (0-based pre-order traversal).
    pub position: Field,
    /// Byte offset where content span starts.
    pub byte_start: Field,
    /// Byte offset where content span ends.
    pub byte_end: Field,
    /// Number of siblings including this node.
    pub sibling_count: Field,
    /// File modification time.
    pub mtime: Field,
}

impl IndexSchema {
    /// Creates a new index schema with all fields configured.
    pub fn new() -> Self {
        let mut builder = Schema::builder();

        // ID field: stored only, not indexed (we use exact term queries for lookup)
        let id = builder.add_text_field("id", STRING | STORED);

        // Doc ID field: stored, for grouping chunks by document
        let doc_id = builder.add_text_field("doc_id", STRING | STORED);

        // Parent ID field: stored, for hierarchy traversal (empty string for root nodes)
        let parent_id = builder.add_text_field("parent_id", STORED);

        // Hierarchy field: multi-value text with positions, stored
        // Each value is a title in the path from document root to this chunk
        let hierarchy_options = TextOptions::default()
            .set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer(RA_TOKENIZER)
                    .set_index_option(IndexRecordOption::WithFreqsAndPositions),
            )
            .set_stored();
        let hierarchy = builder.add_text_field("hierarchy", hierarchy_options);

        // Tags field: text with positions, stored
        let tags_options = TextOptions::default()
            .set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer(RA_TOKENIZER)
                    .set_index_option(IndexRecordOption::WithFreqsAndPositions),
            )
            .set_stored();
        let tags = builder.add_text_field("tags", tags_options);

        // Path field: text with positions, stored
        let path_options = TextOptions::default()
            .set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer(RA_TOKENIZER)
                    .set_index_option(IndexRecordOption::WithFreqsAndPositions),
            )
            .set_stored();
        let path = builder.add_text_field("path", path_options);

        // Tree field: string (single token), stored, fast for filtering
        let tree = builder.add_text_field("tree", STRING | STORED | FAST);

        // Body field: text with positions, stored
        let body_options = TextOptions::default()
            .set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer(RA_TOKENIZER)
                    .set_index_option(IndexRecordOption::WithFreqsAndPositions),
            )
            .set_stored();
        let body = builder.add_text_field("body", body_options);

        // Depth field: u64, stored and fast for hierarchy boost computation
        let depth = builder.add_u64_field("depth", STORED | FAST);

        // Position field: u64, stored and indexed for ordering
        let position = builder.add_u64_field("position", STORED | INDEXED);

        // Byte span fields: u64, stored only (for source lookup)
        let byte_start = builder.add_u64_field("byte_start", STORED);
        let byte_end = builder.add_u64_field("byte_end", STORED);

        // Sibling count field: u64, stored for aggregation threshold calculation
        let sibling_count = builder.add_u64_field("sibling_count", STORED);

        // Mtime field: date, indexed, fast for filtering/sorting
        let mtime_options = DateOptions::default().set_indexed().set_fast();
        let mtime = builder.add_date_field("mtime", mtime_options);

        let schema = builder.build();

        Self {
            schema,
            id,
            doc_id,
            parent_id,
            hierarchy,
            tags,
            path,
            tree,
            body,
            depth,
            position,
            byte_start,
            byte_end,
            sibling_count,
            mtime,
        }
    }

    /// Returns a reference to the underlying Tantivy schema.
    pub fn schema(&self) -> &Schema {
        &self.schema
    }
}

impl Default for IndexSchema {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod test {
    use tantivy::schema::FieldType;

    use super::*;

    #[test]
    fn schema_has_all_fields() {
        let schema = IndexSchema::new();
        let tantivy_schema = schema.schema();

        // Verify all fields exist with expected names
        assert!(tantivy_schema.get_field("id").is_ok());
        assert!(tantivy_schema.get_field("doc_id").is_ok());
        assert!(tantivy_schema.get_field("parent_id").is_ok());
        assert!(tantivy_schema.get_field("hierarchy").is_ok());
        assert!(tantivy_schema.get_field("tags").is_ok());
        assert!(tantivy_schema.get_field("path").is_ok());
        assert!(tantivy_schema.get_field("tree").is_ok());
        assert!(tantivy_schema.get_field("body").is_ok());
        assert!(tantivy_schema.get_field("depth").is_ok());
        assert!(tantivy_schema.get_field("position").is_ok());
        assert!(tantivy_schema.get_field("byte_start").is_ok());
        assert!(tantivy_schema.get_field("byte_end").is_ok());
        assert!(tantivy_schema.get_field("sibling_count").is_ok());
        assert!(tantivy_schema.get_field("mtime").is_ok());
    }

    #[test]
    fn id_field_is_string_and_stored() {
        let schema = IndexSchema::new();
        let entry = schema.schema().get_field_entry(schema.id);

        assert!(entry.is_indexed());
        assert!(entry.is_stored());

        // STRING type means it's indexed as a single token
        if let FieldType::Str(opts) = entry.field_type() {
            let indexing = opts.get_indexing_options().unwrap();
            assert_eq!(indexing.tokenizer(), "raw");
        } else {
            panic!("id field should be text type");
        }
    }

    #[test]
    fn text_fields_are_tokenized_and_stored() {
        let schema = IndexSchema::new();

        for (name, field) in [
            ("hierarchy", schema.hierarchy),
            ("tags", schema.tags),
            ("path", schema.path),
            ("body", schema.body),
        ] {
            let entry = schema.schema().get_field_entry(field);
            assert!(entry.is_indexed(), "{name} should be indexed");
            assert!(entry.is_stored(), "{name} should be stored");

            if let FieldType::Str(opts) = entry.field_type() {
                let indexing = opts.get_indexing_options().unwrap();
                assert_eq!(
                    indexing.tokenizer(),
                    RA_TOKENIZER,
                    "{name} should use ra_text tokenizer"
                );
            } else {
                panic!("{name} field should be text type");
            }
        }
    }

    #[test]
    fn tree_field_is_string_stored_and_fast() {
        let schema = IndexSchema::new();
        let entry = schema.schema().get_field_entry(schema.tree);

        assert!(entry.is_indexed());
        assert!(entry.is_stored());
        assert!(entry.is_fast());

        if let FieldType::Str(opts) = entry.field_type() {
            let indexing = opts.get_indexing_options().unwrap();
            assert_eq!(indexing.tokenizer(), "raw");
        } else {
            panic!("tree field should be text type");
        }
    }

    #[test]
    fn mtime_field_is_indexed_and_fast() {
        let schema = IndexSchema::new();
        let entry = schema.schema().get_field_entry(schema.mtime);

        assert!(entry.is_indexed());
        assert!(entry.is_fast());

        assert!(
            matches!(entry.field_type(), FieldType::Date(_)),
            "mtime field should be date type"
        );
    }

    #[test]
    fn hierarchical_fields_have_correct_types() {
        let schema = IndexSchema::new();

        // doc_id: STRING, stored
        let entry = schema.schema().get_field_entry(schema.doc_id);
        assert!(entry.is_stored());
        assert!(entry.is_indexed());

        // parent_id: stored only (not indexed, just for lookup)
        let entry = schema.schema().get_field_entry(schema.parent_id);
        assert!(entry.is_stored());

        // position: u64, stored and indexed
        let entry = schema.schema().get_field_entry(schema.position);
        assert!(entry.is_stored());
        assert!(entry.is_indexed());
        assert!(matches!(entry.field_type(), FieldType::U64(_)));

        // byte_start, byte_end: u64, stored only
        let entry = schema.schema().get_field_entry(schema.byte_start);
        assert!(entry.is_stored());
        assert!(matches!(entry.field_type(), FieldType::U64(_)));

        let entry = schema.schema().get_field_entry(schema.byte_end);
        assert!(entry.is_stored());
        assert!(matches!(entry.field_type(), FieldType::U64(_)));

        // sibling_count: u64, stored only
        let entry = schema.schema().get_field_entry(schema.sibling_count);
        assert!(entry.is_stored());
        assert!(matches!(entry.field_type(), FieldType::U64(_)));

        // depth: u64, stored and fast
        let entry = schema.schema().get_field_entry(schema.depth);
        assert!(entry.is_stored());
        assert!(entry.is_fast());
        assert!(matches!(entry.field_type(), FieldType::U64(_)));
    }
}
