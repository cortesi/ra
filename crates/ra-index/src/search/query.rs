//! Query construction helpers for Searcher.

use tantivy::Term;
use tantivy::query::{BooleanQuery, Occur, Query, TermQuery};
use tantivy::schema::IndexRecordOption;

use super::Searcher;
use crate::IndexError;
use crate::query::{QueryError, parse};

impl Searcher {
    /// Parses and compiles a query string into a Tantivy query.
    pub(crate) fn build_query(
        &mut self,
        query_str: &str,
    ) -> Result<Option<Box<dyn Query>>, IndexError> {
        let expr = parse(query_str).map_err(|e| {
            let query_err: QueryError = e.into();
            IndexError::Query(query_err.with_query(query_str))
        })?;

        match expr {
            Some(e) => {
                let result = self.query_compiler.compile(&e).map_err(|e| {
                    let query_err: QueryError = e.into();
                    IndexError::Query(query_err.with_query(query_str))
                })?;
                Ok(result)
            }
            None => Ok(None),
        }
    }

    /// Builds a tree filter query for the given tree names.
    pub(crate) fn build_tree_filter(&self, trees: &[String]) -> Option<Box<dyn Query>> {
        if trees.is_empty() {
            return None;
        }

        if trees.len() == 1 {
            let term = Term::from_field_text(self.schema.tree, &trees[0]);
            return Some(Box::new(TermQuery::new(term, IndexRecordOption::Basic)));
        }

        let clauses: Vec<(Occur, Box<dyn Query>)> = trees
            .iter()
            .map(|tree_name| {
                let term = Term::from_field_text(self.schema.tree, tree_name);
                let query: Box<dyn Query> =
                    Box::new(TermQuery::new(term, IndexRecordOption::Basic));
                (Occur::Should, query)
            })
            .collect();

        Some(Box::new(BooleanQuery::new(clauses)))
    }

    /// Wraps a content query with a tree filter.
    pub(crate) fn apply_tree_filter(
        &self,
        content_query: Box<dyn Query>,
        trees: &[String],
    ) -> Box<dyn Query> {
        match self.build_tree_filter(trees) {
            Some(tree_filter) => {
                let clauses = vec![(Occur::Must, content_query), (Occur::Must, tree_filter)];
                Box::new(BooleanQuery::new(clauses))
            }
            None => content_query,
        }
    }

    /// Tokenizes a query string to extract individual search terms.
    ///
    /// Filters out query syntax elements (OR, AND, NOT, field prefixes) before
    /// tokenizing to avoid treating keywords as search terms.
    pub(crate) fn tokenize_query(&mut self, query_str: &str) -> Vec<String> {
        let filtered: String = query_str
            .split_whitespace()
            .filter(|word| {
                let upper = word.to_uppercase();
                upper != "OR" && upper != "AND" && upper != "NOT" && !word.contains(':')
            })
            .collect::<Vec<_>>()
            .join(" ");

        let mut stream = self.analyzer.token_stream(&filtered);
        let mut tokens = Vec::new();
        while let Some(token) = stream.next() {
            tokens.push(token.text.clone());
        }
        tokens
    }
}
