//! Fuzzy matching helpers for Searcher.

use levenshtein_automata::{Distance, SINK_STATE};
use tantivy_fst::Automaton;

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

// NOTE: Searcher methods live in `search/mod.rs`. This module only supplies the
// Levenshtein DFA adapter used for fuzzy term lookup.
