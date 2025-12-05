//! Fuzzy term lookup helpers and Levenshtein automaton wrapper.

use std::{
    collections::{HashMap, HashSet},
    str,
};

use levenshtein_automata::{Distance, SINK_STATE};
use tantivy::{Searcher as TvSearcher, schema::Field};
use tantivy_fst::Automaton;

use super::Searcher;

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
