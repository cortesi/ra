//! IDF helpers, tree filtering, and convenience lookups.

use std::collections::HashSet;

use ra_context::IdfProvider;
use tantivy::{
    Term,
    collector::{Count, TopDocs},
    query::{AllQuery, Query, TermQuery},
    schema::IndexRecordOption,
};

use super::{SearchCandidate, Searcher};
use crate::IndexError;

impl Searcher {
    /// Returns the number of documents in the index.
    pub fn num_docs(&self) -> Result<u64, IndexError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| IndexError::Write(e.to_string()))?;
        Ok(reader.searcher().num_docs())
    }

    /// Computes the IDF (Inverse Document Frequency) for a term.
    pub fn term_idf(&self, term: &str) -> Result<Option<f32>, IndexError> {
        self.term_idf_in_trees(term, &[])
    }

    /// Computes IDF for a term, optionally filtered to specific trees.
    pub fn term_idf_in_trees(
        &self,
        term: &str,
        trees: &[String],
    ) -> Result<Option<f32>, IndexError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| IndexError::Write(e.to_string()))?;
        let searcher = reader.searcher();

        let mut analyzer = self.analyzer.clone();
        let mut stream = analyzer.token_stream(term);
        let stemmed_term = if let Some(token) = stream.next() {
            token.text.clone()
        } else {
            term.to_string()
        };

        let term_query: Box<dyn Query> = Box::new(TermQuery::new(
            Term::from_field_text(self.schema.body, &stemmed_term),
            IndexRecordOption::Basic,
        ));

        let query = self.apply_tree_filter(term_query, trees);

        let doc_freq = searcher
            .search(&query, &Count)
            .map_err(|e| IndexError::Write(e.to_string()))?;

        if doc_freq == 0 {
            return Ok(None);
        }

        let total_docs = if trees.is_empty() {
            searcher.num_docs() as f32
        } else {
            let tree_filter = self.build_tree_filter(trees);
            match tree_filter {
                Some(filter) => searcher
                    .search(&filter, &Count)
                    .map_err(|e| IndexError::Write(e.to_string()))?
                    as f32,
                None => searcher.num_docs() as f32,
            }
        };

        let idf = ((total_docs + 1.0) / (doc_freq as f32 + 1.0)).ln() + 1.0;

        Ok(Some(idf))
    }

    /// Retrieves a chunk by its exact ID.
    pub fn get_by_id(&self, id: &str) -> Result<Option<SearchCandidate>, IndexError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let searcher = reader.searcher();

        let term = Term::from_field_text(self.schema.id, id);
        let query = TermQuery::new(term, IndexRecordOption::Basic);

        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(1))
            .map_err(|e| IndexError::Write(e.to_string()))?;

        if let Some((score, doc_address)) = top_docs.first() {
            let doc: tantivy::TantivyDocument = searcher
                .doc(*doc_address)
                .map_err(|e| IndexError::Write(e.to_string()))?;

            let empty_terms = HashSet::new();
            let result = self.doc_to_result(&doc, *score, &None, &empty_terms);
            Ok(Some(result))
        } else {
            Ok(None)
        }
    }

    /// Lists all chunks in the index, ordered by ID.
    pub fn list_all(&self) -> Result<Vec<SearchCandidate>, IndexError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let searcher = reader.searcher();

        let all_docs = searcher
            .search(&AllQuery, &TopDocs::with_limit(100_000))
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let mut results: Vec<SearchCandidate> = all_docs
            .into_iter()
            .filter_map(|(score, doc_address)| {
                let doc: tantivy::TantivyDocument = searcher.doc(doc_address).ok()?;
                let empty_terms = HashSet::new();
                Some(self.doc_to_result(&doc, score, &None, &empty_terms))
            })
            .collect();

        results.sort_by(|a, b| a.id.cmp(&b.id));

        Ok(results)
    }

    /// Retrieves all chunks from a document by path.
    pub fn get_by_path(&self, tree: &str, path: &str) -> Result<Vec<SearchCandidate>, IndexError> {
        let reader = self
            .index
            .reader()
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let searcher = reader.searcher();

        let id_prefix = format!("{tree}:{path}");

        let all_docs = searcher
            .search(&AllQuery, &TopDocs::with_limit(10000))
            .map_err(|e| IndexError::Write(e.to_string()))?;

        let mut results: Vec<SearchCandidate> = all_docs
            .into_iter()
            .filter_map(|(score, doc_address)| {
                let doc: tantivy::TantivyDocument = searcher.doc(doc_address).ok()?;
                let empty_terms = HashSet::new();
                let result = self.doc_to_result(&doc, score, &None, &empty_terms);
                if result.id == id_prefix || result.id.starts_with(&format!("{id_prefix}#")) {
                    Some(result)
                } else {
                    None
                }
            })
            .collect();

        results.sort_by(|a, b| a.id.cmp(&b.id));

        Ok(results)
    }
}

impl IdfProvider for Searcher {
    fn idf(&self, term: &str) -> Option<f32> {
        self.term_idf(term).ok().flatten()
    }
}

/// A wrapper around `Searcher` that filters IDF lookups to specific trees.
pub struct TreeFilteredSearcher<'a> {
    /// Underlying searcher providing IDF values.
    searcher: &'a Searcher,
    /// Trees to limit IDF calculations to.
    trees: Vec<String>,
}

impl<'a> TreeFilteredSearcher<'a> {
    /// Creates a new tree-filtered searcher.
    pub fn new(searcher: &'a Searcher, trees: Vec<String>) -> Self {
        Self { searcher, trees }
    }
}

impl IdfProvider for TreeFilteredSearcher<'_> {
    fn idf(&self, term: &str) -> Option<f32> {
        self.searcher
            .term_idf_in_trees(term, &self.trees)
            .ok()
            .flatten()
    }
}
