//! Index schema definition for the ra search index.
//!
//! Defines the Tantivy schema with all fields needed for chunk indexing:
//! - `id`: Unique chunk identifier (stored only)
//! - `title`: Chunk title (text, stored, boosted 3.0x)
//! - `tags`: Document tags (text, stored, boosted 2.5x)
//! - `path`: File path within tree (text, stored, boosted 2.0x)
//! - `path_components`: Path segments for partial matching (text, boosted 2.0x)
//! - `tree`: Tree name (string, stored, fast)
//! - `body`: Chunk content (text, stored)
//! - `mtime`: File modification time (date, indexed, fast)

use tantivy::schema::{
    DateOptions, FAST, Field, IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing,
    TextOptions,
};

use crate::analyzer::RA_TOKENIZER;

/// Field boost weights for search ranking.
pub mod boost {
    /// Title field boost (3.0x).
    pub const TITLE: f32 = 3.0;
    /// Tags field boost (2.5x).
    pub const TAGS: f32 = 2.5;
    /// Path field boost (2.0x).
    pub const PATH: f32 = 2.0;
    /// Path components field boost (2.0x).
    pub const PATH_COMPONENTS: f32 = 2.0;
    /// Body field boost (1.0x).
    pub const BODY: f32 = 1.0;
}

/// Handles to all fields in the index schema.
#[derive(Debug, Clone)]
pub struct IndexSchema {
    /// The underlying Tantivy schema.
    schema: Schema,
    /// Unique chunk identifier: `{tree}:{path}#{slug}`.
    pub id: Field,
    /// Chunk title.
    pub title: Field,
    /// Document tags from frontmatter.
    pub tags: Field,
    /// File path within the tree.
    pub path: Field,
    /// Path split into components for partial matching.
    pub path_components: Field,
    /// Tree name this chunk belongs to.
    pub tree: Field,
    /// Chunk body content.
    pub body: Field,
    /// File modification time.
    pub mtime: Field,
}

impl IndexSchema {
    /// Creates a new index schema with all fields configured.
    pub fn new() -> Self {
        let mut builder = Schema::builder();

        // ID field: stored only, not indexed (we use exact term queries for lookup)
        let id = builder.add_text_field("id", STRING | STORED);

        // Title field: text with positions, stored, boosted 3.0x
        let title_options = TextOptions::default()
            .set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer(RA_TOKENIZER)
                    .set_index_option(IndexRecordOption::WithFreqsAndPositions),
            )
            .set_stored();
        let title = builder.add_text_field("title", title_options);

        // Tags field: text with positions, stored, boosted 2.5x
        let tags_options = TextOptions::default()
            .set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer(RA_TOKENIZER)
                    .set_index_option(IndexRecordOption::WithFreqsAndPositions),
            )
            .set_stored();
        let tags = builder.add_text_field("tags", tags_options);

        // Path field: text with positions, stored, boosted 2.0x
        let path_options = TextOptions::default()
            .set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer(RA_TOKENIZER)
                    .set_index_option(IndexRecordOption::WithFreqsAndPositions),
            )
            .set_stored();
        let path = builder.add_text_field("path", path_options);

        // Path components field: text with positions, NOT stored (just for searching)
        let path_components_options = TextOptions::default().set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(RA_TOKENIZER)
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        );
        let path_components = builder.add_text_field("path_components", path_components_options);

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

        // Mtime field: date, indexed, fast for filtering/sorting
        let mtime_options = DateOptions::default().set_indexed().set_fast();
        let mtime = builder.add_date_field("mtime", mtime_options);

        let schema = builder.build();

        Self {
            schema,
            id,
            title,
            tags,
            path,
            path_components,
            tree,
            body,
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
        assert!(tantivy_schema.get_field("title").is_ok());
        assert!(tantivy_schema.get_field("tags").is_ok());
        assert!(tantivy_schema.get_field("path").is_ok());
        assert!(tantivy_schema.get_field("path_components").is_ok());
        assert!(tantivy_schema.get_field("tree").is_ok());
        assert!(tantivy_schema.get_field("body").is_ok());
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
            ("title", schema.title),
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
    fn path_components_not_stored() {
        let schema = IndexSchema::new();
        let entry = schema.schema().get_field_entry(schema.path_components);

        assert!(entry.is_indexed());
        assert!(!entry.is_stored());
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
}
