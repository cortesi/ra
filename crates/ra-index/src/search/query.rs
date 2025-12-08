//! Query construction and fuzzy matching helpers for Searcher.

use std::{
    collections::{HashMap, HashSet},
    str,
};

use levenshtein_automata::{Distance, SINK_STATE};
use tantivy::{
    Searcher as TvSearcher, Term,
    query::{BooleanQuery, Occur, Query, TermQuery},
    schema::{Field, IndexRecordOption},
};
use tantivy_fst::Automaton;

use super::Searcher;
use crate::{
    IndexError,
    query::{QueryError, parse},
};

/// Wrapper that implements `tantivy_fst::Automaton` for `levenshtein_automata::DFA`.
pub(super) struct LevenshteinDfa(pub(super) levenshtein_automata::DFA);

impl Automaton for LevenshteinDfa {
    type State = u32;

    fn start(&self) -> Self::State {
        self.0.initial_state()
    }

    fn is_match(&self, state: &Self::State) -> bool {
        matches!(self.0.distance(*state), Distance::Exact(_))
    }

    fn can_match(&self, state: &Self::State) -> bool {
        *state != SINK_STATE
    }

    fn accept(&self, state: &Self::State, byte: u8) -> Self::State {
        self.0.transition(*state, byte)
    }
}

impl Searcher {
    /// Parses and compiles a query string into a Tantivy query.
    pub(crate) fn build_query(
        &mut self,
        query_str: &str,
    ) -> Result<Option<Box<dyn Query>>, IndexError> {
        let expr = parse(query_str).map_err(|e| {
            let query_err: QueryError = e;
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

    /// Finds actual terms in the index that match the query terms (including fuzzy matches).
    ///
    /// For each query term, uses a Levenshtein automaton to search the term dictionary
    /// and collect all indexed terms that match within the configured fuzzy distance.
    pub(crate) fn find_matched_terms(
        &self,
        searcher: &TvSearcher,
        query_terms: &[String],
        fields: &[Field],
    ) -> HashSet<String> {
        let mut matched_terms = HashSet::new();

        if self.fuzzy_distance == 0 {
            matched_terms.extend(query_terms.iter().cloned());
            return matched_terms;
        }

        for segment_reader in searcher.segment_readers() {
            for field in fields {
                if let Ok(inverted_index) = segment_reader.inverted_index(*field) {
                    let term_dict = inverted_index.terms();

                    for query_term in query_terms {
                        let dfa = LevenshteinDfa(self.lev_builder.build_dfa(query_term));

                        let mut stream = term_dict.search(dfa).into_stream().unwrap();

                        while stream.advance() {
                            if let Ok(term_str) = str::from_utf8(stream.key()) {
                                matched_terms.insert(term_str.to_string());
                            }
                        }
                    }
                }
            }
        }

        if matched_terms.is_empty() {
            matched_terms.extend(query_terms.iter().cloned());
        }

        matched_terms
    }

    /// Finds term mappings from query terms to indexed terms (with fuzzy matching).
    ///
    /// Returns a map where keys are query terms and values are the indexed terms
    /// they matched (including fuzzy matches).
    pub(crate) fn find_term_mappings(
        &self,
        searcher: &TvSearcher,
        query_terms: &[String],
    ) -> HashMap<String, Vec<String>> {
        let mut mappings: HashMap<String, Vec<String>> = HashMap::new();

        if self.fuzzy_distance == 0 {
            for term in query_terms {
                mappings.insert(term.clone(), vec![term.clone()]);
            }
            return mappings;
        }

        for segment_reader in searcher.segment_readers() {
            if let Ok(inverted_index) = segment_reader.inverted_index(self.schema.body) {
                let term_dict = inverted_index.terms();

                for query_term in query_terms {
                    let dfa = LevenshteinDfa(self.lev_builder.build_dfa(query_term));
                    let mut stream = term_dict.search(dfa).into_stream().unwrap();

                    let entry = mappings.entry(query_term.clone()).or_default();
                    while stream.advance() {
                        if let Ok(term_str) = str::from_utf8(stream.key())
                            && !entry.contains(&term_str.to_string())
                        {
                            entry.push(term_str.to_string());
                        }
                    }
                }
            }
        }

        for term in query_terms {
            mappings
                .entry(term.clone())
                .or_insert_with(|| vec![term.clone()]);
        }

        mappings
    }
}
