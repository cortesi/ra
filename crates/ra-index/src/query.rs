//! Query builder for constructing search queries.
//!
//! Turns simple agent input into Tantivy queries with field boosting.
//! The text analysis pipeline (configured in the analyzer module) handles
//! stemming and tokenization; this module handles query structure.
//!
//! # Query Syntax
//!
//! - Bare terms are combined with AND logic
//! - Quoted phrases match exact sequences (after tokenization)
//! - Each term/phrase is searched across multiple fields with different boosts
//!
//! # Example
//!
//! ```ignore
//! // QueryBuilder is used internally by Searcher
//! let mut searcher = Searcher::open(path, "english", &trees, 1.5)?;
//! let results = searcher.search("rust async handling", 10)?;
//! ```

use tantivy::{
    Term,
    query::{BooleanQuery, BoostQuery, Occur, PhraseQuery, Query, TermQuery},
    schema::{Field, IndexRecordOption},
    tokenizer::{TextAnalyzer, TokenStream},
};

use crate::{
    IndexError,
    analyzer::build_analyzer_from_name,
    schema::{IndexSchema, boost},
};

/// A parsed token from the query string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryToken {
    /// A single word term.
    Term(String),
    /// A quoted phrase (sequence of words).
    Phrase(Vec<String>),
}

/// Parses a query string into tokens.
///
/// Detects quoted phrases vs bare terms:
/// - `"exact phrase"` becomes a Phrase token
/// - `word` becomes a Term token
/// - Multiple words without quotes become multiple Term tokens
///
/// Quoted phrases are split by the provided tokenizer to get individual terms.
pub fn parse_query(input: &str) -> Vec<QueryToken> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    let mut current_term = String::new();

    while let Some(c) = chars.next() {
        match c {
            '"' => {
                // Flush any pending term
                if !current_term.is_empty() {
                    tokens.push(QueryToken::Term(current_term.trim().to_string()));
                    current_term.clear();
                }

                // Collect phrase until closing quote
                let mut phrase = String::new();
                for c in chars.by_ref() {
                    if c == '"' {
                        break;
                    }
                    phrase.push(c);
                }

                // Split phrase into words (simple whitespace split for now,
                // tokenization happens later)
                let words: Vec<String> = phrase.split_whitespace().map(|s| s.to_string()).collect();
                if !words.is_empty() {
                    tokens.push(QueryToken::Phrase(words));
                }
            }
            c if c.is_whitespace() => {
                if !current_term.is_empty() {
                    tokens.push(QueryToken::Term(current_term.clone()));
                    current_term.clear();
                }
            }
            c => {
                current_term.push(c);
            }
        }
    }

    // Flush final term
    if !current_term.is_empty() {
        tokens.push(QueryToken::Term(current_term));
    }

    tokens
}

/// Tokenizes text using the configured analyzer.
///
/// Returns the stemmed/normalized tokens that will be used for matching.
fn tokenize(analyzer: &mut TextAnalyzer, text: &str) -> Vec<String> {
    let mut stream = analyzer.token_stream(text);
    let mut tokens = Vec::new();
    while let Some(token) = stream.next() {
        tokens.push(token.text.clone());
    }
    tokens
}

/// Builds Tantivy queries from parsed query tokens.
pub struct QueryBuilder {
    /// Index schema with field handles.
    schema: IndexSchema,
    /// Text analyzer for tokenizing query input.
    analyzer: TextAnalyzer,
}

impl QueryBuilder {
    /// Creates a new query builder with the given schema and stemmer language.
    pub(crate) fn with_language(schema: IndexSchema, language: &str) -> Result<Self, IndexError> {
        let analyzer = build_analyzer_from_name(language)?;
        Ok(Self { schema, analyzer })
    }

    /// Builds a query from an input string.
    ///
    /// The input is parsed for quoted phrases and bare terms, then combined
    /// with AND logic. Each term/phrase searches across multiple fields with
    /// configured boost weights.
    pub fn build(&mut self, input: &str) -> Option<Box<dyn Query>> {
        let tokens = parse_query(input);
        if tokens.is_empty() {
            return None;
        }

        let clauses: Vec<(Occur, Box<dyn Query>)> = tokens
            .into_iter()
            .filter_map(|token| self.build_token_query(token))
            .map(|q| (Occur::Must, q))
            .collect();

        if clauses.is_empty() {
            return None;
        }

        Some(Box::new(BooleanQuery::new(clauses)))
    }

    /// Builds a query for a single token (term or phrase).
    fn build_token_query(&mut self, token: QueryToken) -> Option<Box<dyn Query>> {
        match token {
            QueryToken::Term(text) => self.build_term_query(&text),
            QueryToken::Phrase(words) => self.build_phrase_query(&words),
        }
    }

    /// Builds a boosted multi-field query for a single term.
    ///
    /// The term is tokenized (stemmed) and searched across all text fields
    /// with their respective boost weights.
    fn build_term_query(&mut self, text: &str) -> Option<Box<dyn Query>> {
        let terms = tokenize(&mut self.analyzer, text);
        if terms.is_empty() {
            return None;
        }

        // If tokenization produced multiple tokens, treat them as a phrase
        if terms.len() > 1 {
            return self.build_phrase_query_from_tokens(&terms);
        }

        let term_text = &terms[0];
        self.build_multi_field_term_query(term_text)
    }

    /// Builds a boosted multi-field query for a phrase.
    ///
    /// Each word in the phrase is tokenized (stemmed) and combined into
    /// phrase queries across all text fields.
    fn build_phrase_query(&mut self, words: &[String]) -> Option<Box<dyn Query>> {
        // Tokenize each word and flatten into a single token list
        let tokens: Vec<String> = words
            .iter()
            .flat_map(|word| tokenize(&mut self.analyzer, word))
            .collect();

        self.build_phrase_query_from_tokens(&tokens)
    }

    /// Builds a phrase query from pre-tokenized terms.
    fn build_phrase_query_from_tokens(&self, tokens: &[String]) -> Option<Box<dyn Query>> {
        if tokens.is_empty() {
            return None;
        }

        // If only one token, use a term query instead
        if tokens.len() == 1 {
            return self.build_multi_field_term_query(&tokens[0]);
        }

        // Build phrase queries for each searchable field
        let fields_with_boosts: [(Field, f32); 4] = [
            (self.schema.title, boost::TITLE),
            (self.schema.tags, boost::TAGS),
            (self.schema.path, boost::PATH),
            (self.schema.body, boost::BODY),
        ];

        let clauses: Vec<(Occur, Box<dyn Query>)> = fields_with_boosts
            .into_iter()
            .map(|(field, boost_value)| {
                let terms: Vec<Term> = tokens
                    .iter()
                    .map(|t| Term::from_field_text(field, t))
                    .collect();
                let phrase_query = PhraseQuery::new(terms);
                let boosted: Box<dyn Query> =
                    Box::new(BoostQuery::new(Box::new(phrase_query), boost_value));
                (Occur::Should, boosted)
            })
            .collect();

        Some(Box::new(BooleanQuery::new(clauses)))
    }

    /// Builds a multi-field term query with boosts.
    ///
    /// Searches the term across title, tags, path, path_components, and body fields,
    /// each with their configured boost weight.
    fn build_multi_field_term_query(&self, term_text: &str) -> Option<Box<dyn Query>> {
        let fields_with_boosts: [(Field, f32); 5] = [
            (self.schema.title, boost::TITLE),
            (self.schema.tags, boost::TAGS),
            (self.schema.path, boost::PATH),
            (self.schema.path_components, boost::PATH_COMPONENTS),
            (self.schema.body, boost::BODY),
        ];

        let clauses: Vec<(Occur, Box<dyn Query>)> = fields_with_boosts
            .into_iter()
            .map(|(field, boost_value)| {
                let term = Term::from_field_text(field, term_text);
                let term_query = TermQuery::new(term, IndexRecordOption::WithFreqs);
                let boosted: Box<dyn Query> =
                    Box::new(BoostQuery::new(Box::new(term_query), boost_value));
                (Occur::Should, boosted)
            })
            .collect();

        Some(Box::new(BooleanQuery::new(clauses)))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn parse_single_term() {
        let tokens = parse_query("hello");
        assert_eq!(tokens, vec![QueryToken::Term("hello".to_string())]);
    }

    #[test]
    fn parse_multiple_terms() {
        let tokens = parse_query("hello world");
        assert_eq!(
            tokens,
            vec![
                QueryToken::Term("hello".to_string()),
                QueryToken::Term("world".to_string()),
            ]
        );
    }

    #[test]
    fn parse_quoted_phrase() {
        let tokens = parse_query("\"hello world\"");
        assert_eq!(
            tokens,
            vec![QueryToken::Phrase(vec![
                "hello".to_string(),
                "world".to_string()
            ])]
        );
    }

    #[test]
    fn parse_mixed_terms_and_phrases() {
        let tokens = parse_query("foo \"hello world\" bar");
        assert_eq!(
            tokens,
            vec![
                QueryToken::Term("foo".to_string()),
                QueryToken::Phrase(vec!["hello".to_string(), "world".to_string()]),
                QueryToken::Term("bar".to_string()),
            ]
        );
    }

    #[test]
    fn parse_empty_string() {
        let tokens = parse_query("");
        assert!(tokens.is_empty());
    }

    #[test]
    fn parse_whitespace_only() {
        let tokens = parse_query("   ");
        assert!(tokens.is_empty());
    }

    #[test]
    fn parse_empty_quotes() {
        let tokens = parse_query("\"\"");
        assert!(tokens.is_empty());
    }

    #[test]
    fn parse_unclosed_quote() {
        // Unclosed quote should capture everything after as a phrase
        let tokens = parse_query("\"hello world");
        assert_eq!(
            tokens,
            vec![QueryToken::Phrase(vec![
                "hello".to_string(),
                "world".to_string()
            ])]
        );
    }

    #[test]
    fn parse_multiple_phrases() {
        let tokens = parse_query("\"one two\" \"three four\"");
        assert_eq!(
            tokens,
            vec![
                QueryToken::Phrase(vec!["one".to_string(), "two".to_string()]),
                QueryToken::Phrase(vec!["three".to_string(), "four".to_string()]),
            ]
        );
    }

    #[test]
    fn parse_extra_whitespace() {
        let tokens = parse_query("  hello   world  ");
        assert_eq!(
            tokens,
            vec![
                QueryToken::Term("hello".to_string()),
                QueryToken::Term("world".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_applies_stemming() {
        let mut analyzer = build_analyzer_from_name("english").unwrap();
        let tokens = tokenize(&mut analyzer, "handling");
        assert_eq!(tokens, vec!["handl"]);
    }

    #[test]
    fn tokenize_lowercases() {
        let mut analyzer = build_analyzer_from_name("english").unwrap();
        let tokens = tokenize(&mut analyzer, "HELLO");
        assert_eq!(tokens, vec!["hello"]);
    }

    #[test]
    fn tokenize_splits_punctuation() {
        let mut analyzer = build_analyzer_from_name("english").unwrap();
        let tokens = tokenize(&mut analyzer, "foo-bar");
        assert_eq!(tokens, vec!["foo", "bar"]);
    }

    #[test]
    fn query_builder_empty_input() {
        let schema = IndexSchema::new();
        let mut builder = QueryBuilder::with_language(schema, "english").unwrap();
        assert!(builder.build("").is_none());
        assert!(builder.build("   ").is_none());
    }

    #[test]
    fn query_builder_single_term() {
        let schema = IndexSchema::new();
        let mut builder = QueryBuilder::with_language(schema, "english").unwrap();
        let query = builder.build("rust");
        assert!(query.is_some());
    }

    #[test]
    fn query_builder_multiple_terms() {
        let schema = IndexSchema::new();
        let mut builder = QueryBuilder::with_language(schema, "english").unwrap();
        let query = builder.build("rust async");
        assert!(query.is_some());
    }

    #[test]
    fn query_builder_phrase() {
        let schema = IndexSchema::new();
        let mut builder = QueryBuilder::with_language(schema, "english").unwrap();
        let query = builder.build("\"error handling\"");
        assert!(query.is_some());
    }

    #[test]
    fn query_builder_mixed() {
        let schema = IndexSchema::new();
        let mut builder = QueryBuilder::with_language(schema, "english").unwrap();
        let query = builder.build("rust \"error handling\" async");
        assert!(query.is_some());
    }

    #[test]
    fn query_builder_with_language() {
        let schema = IndexSchema::new();
        let builder = QueryBuilder::with_language(schema, "french");
        assert!(builder.is_ok());
    }

    #[test]
    fn query_builder_invalid_language() {
        let schema = IndexSchema::new();
        let result = QueryBuilder::with_language(schema, "invalid");
        assert!(result.is_err());
    }
}
