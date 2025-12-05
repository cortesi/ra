//! Query compiler.
//!
//! Compiles a query AST into Tantivy queries.

use std::{error::Error, fmt};

use ra_query::QueryExpr;
use tantivy::{
    Term,
    query::{
        AllQuery, BooleanQuery, BoostQuery, FuzzyTermQuery, Occur, PhraseQuery, Query, TermQuery,
    },
    schema::{Field, IndexRecordOption},
    tokenizer::{TextAnalyzer, TokenStream},
};

use crate::{
    IndexError,
    analyzer::build_analyzer_from_name,
    schema::{IndexSchema, boost},
};

/// Error during query compilation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    /// Error message.
    pub message: String,
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Error for CompileError {}

/// Compiles query AST nodes into Tantivy queries.
pub struct QueryCompiler {
    /// Index schema for field references.
    schema: IndexSchema,
    /// Text analyzer for tokenizing query terms.
    analyzer: TextAnalyzer,
    /// Levenshtein distance for fuzzy matching (0 = disabled).
    fuzzy_distance: u8,
}

impl QueryCompiler {
    /// Creates a new query compiler.
    pub fn new(
        schema: IndexSchema,
        language: &str,
        fuzzy_distance: u8,
    ) -> Result<Self, IndexError> {
        let analyzer = build_analyzer_from_name(language)?;
        Ok(Self {
            schema,
            analyzer,
            fuzzy_distance,
        })
    }

    /// Compiles a query expression into a Tantivy query.
    ///
    /// Returns `None` for empty queries, `Some(query)` for valid queries,
    /// or an error for invalid constructs (e.g., NOT-only queries).
    pub fn compile(&mut self, expr: &QueryExpr) -> Result<Option<Box<dyn Query>>, CompileError> {
        match expr {
            QueryExpr::Term(text) => Ok(self.compile_term(text)),
            QueryExpr::Phrase(words) => Ok(self.compile_phrase(words)),
            QueryExpr::Not(inner) => self.compile_not(inner),
            QueryExpr::And(exprs) => self.compile_and(exprs),
            QueryExpr::Or(exprs) => self.compile_or(exprs),
            QueryExpr::Field { name, expr } => self.compile_field(name, expr),
            QueryExpr::Boost { expr, factor } => self.compile_boost(expr, *factor),
        }
    }

    /// Compiles a boosted expression.
    ///
    /// Wraps the inner query with a `BoostQuery` that multiplies the score.
    fn compile_boost(
        &mut self,
        expr: &QueryExpr,
        factor: f32,
    ) -> Result<Option<Box<dyn Query>>, CompileError> {
        match self.compile(expr)? {
            Some(inner) => Ok(Some(Box::new(BoostQuery::new(inner, factor)))),
            None => Ok(None),
        }
    }

    /// Compiles a term into a multi-field query with boosts.
    fn compile_term(&mut self, text: &str) -> Option<Box<dyn Query>> {
        let tokens = self.tokenize(text);
        if tokens.is_empty() {
            return None;
        }

        // If tokenization produced multiple tokens, treat as phrase
        if tokens.len() > 1 {
            return self.compile_phrase_from_tokens(&tokens);
        }

        self.build_multi_field_term_query(&tokens[0])
    }

    /// Compiles a phrase into multi-field phrase queries with boosts.
    fn compile_phrase(&mut self, words: &[String]) -> Option<Box<dyn Query>> {
        let tokens: Vec<String> = words.iter().flat_map(|w| self.tokenize(w)).collect();
        self.compile_phrase_from_tokens(&tokens)
    }

    /// Compiles pre-tokenized terms into phrase queries.
    fn compile_phrase_from_tokens(&self, tokens: &[String]) -> Option<Box<dyn Query>> {
        if tokens.is_empty() {
            return None;
        }

        if tokens.len() == 1 {
            return self.build_multi_field_term_query(&tokens[0]);
        }

        // Build phrase queries for each searchable field (excluding path_components)
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

    /// Compiles a NOT expression.
    ///
    /// NOT alone is invalid (nothing to exclude from). This is handled at the
    /// And level where we separate positive and negative clauses.
    fn compile_not(&mut self, inner: &QueryExpr) -> Result<Option<Box<dyn Query>>, CompileError> {
        // A standalone NOT requires an AllQuery to exclude from
        let inner_query = self.compile(inner)?;
        match inner_query {
            Some(q) => {
                // Wrap in a boolean with AllQuery MUST and inner MUST_NOT
                let clauses = vec![
                    (Occur::Must, Box::new(AllQuery) as Box<dyn Query>),
                    (Occur::MustNot, q),
                ];
                Ok(Some(Box::new(BooleanQuery::new(clauses))))
            }
            None => Ok(None),
        }
    }

    /// Compiles an AND expression.
    ///
    /// Separates positive and negative clauses. Negative clauses (NOT) become
    /// MUST_NOT in the boolean query. If all clauses are negative, we use
    /// AllQuery as the base to exclude from.
    fn compile_and(&mut self, exprs: &[QueryExpr]) -> Result<Option<Box<dyn Query>>, CompileError> {
        if exprs.is_empty() {
            return Ok(None);
        }

        let mut positive_clauses: Vec<Box<dyn Query>> = Vec::new();
        let mut negative_clauses: Vec<Box<dyn Query>> = Vec::new();

        for expr in exprs {
            match expr {
                QueryExpr::Not(inner) => {
                    if let Some(q) = self.compile(inner)? {
                        negative_clauses.push(q);
                    }
                }
                other => {
                    if let Some(q) = self.compile(other)? {
                        positive_clauses.push(q);
                    }
                }
            }
        }

        if positive_clauses.is_empty() && negative_clauses.is_empty() {
            return Ok(None);
        }

        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();

        // Add positive clauses as MUST
        for q in positive_clauses {
            clauses.push((Occur::Must, q));
        }

        // If we have negative clauses but no positive ones, use AllQuery as base
        if clauses.is_empty() && !negative_clauses.is_empty() {
            clauses.push((Occur::Must, Box::new(AllQuery)));
        }

        // Add negative clauses as MUST_NOT
        for q in negative_clauses {
            clauses.push((Occur::MustNot, q));
        }

        Ok(Some(Box::new(BooleanQuery::new(clauses))))
    }

    /// Compiles an OR expression.
    fn compile_or(&mut self, exprs: &[QueryExpr]) -> Result<Option<Box<dyn Query>>, CompileError> {
        if exprs.is_empty() {
            return Ok(None);
        }

        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();

        for expr in exprs {
            if let Some(q) = self.compile(expr)? {
                clauses.push((Occur::Should, q));
            }
        }

        if clauses.is_empty() {
            return Ok(None);
        }

        // For OR to work correctly, at least one SHOULD must match
        // Tantivy handles this by default when there are only SHOULD clauses
        Ok(Some(Box::new(BooleanQuery::new(clauses))))
    }

    /// Compiles a field-specific query.
    fn compile_field(
        &mut self,
        name: &str,
        expr: &QueryExpr,
    ) -> Result<Option<Box<dyn Query>>, CompileError> {
        match name {
            "title" => self.compile_single_field_query(self.schema.title, boost::TITLE, expr),
            "tags" => self.compile_single_field_query(self.schema.tags, boost::TAGS, expr),
            "body" => self.compile_single_field_query(self.schema.body, boost::BODY, expr),
            "path" => self.compile_single_field_query(self.schema.path, boost::PATH, expr),
            "tree" => self.compile_tree_query(expr),
            _ => Err(CompileError {
                message: format!("unknown field: {}", name),
            }),
        }
    }

    /// Compiles a query for a single text field.
    fn compile_single_field_query(
        &mut self,
        field: Field,
        boost_value: f32,
        expr: &QueryExpr,
    ) -> Result<Option<Box<dyn Query>>, CompileError> {
        match expr {
            QueryExpr::Term(text) => {
                let tokens = self.tokenize(text);
                if tokens.is_empty() {
                    return Ok(None);
                }
                if tokens.len() > 1 {
                    return self.compile_single_field_phrase(field, boost_value, &tokens);
                }
                Ok(self.build_single_field_term_query(field, boost_value, &tokens[0]))
            }
            QueryExpr::Phrase(words) => {
                let tokens: Vec<String> = words.iter().flat_map(|w| self.tokenize(w)).collect();
                self.compile_single_field_phrase(field, boost_value, &tokens)
            }
            QueryExpr::Or(exprs) => {
                let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
                for e in exprs {
                    if let Some(q) = self.compile_single_field_query(field, boost_value, e)? {
                        clauses.push((Occur::Should, q));
                    }
                }
                if clauses.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(Box::new(BooleanQuery::new(clauses))))
                }
            }
            QueryExpr::And(exprs) => {
                let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
                for e in exprs {
                    if let Some(q) = self.compile_single_field_query(field, boost_value, e)? {
                        clauses.push((Occur::Must, q));
                    }
                }
                if clauses.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(Box::new(BooleanQuery::new(clauses))))
                }
            }
            QueryExpr::Not(inner) => {
                if let Some(q) = self.compile_single_field_query(field, boost_value, inner)? {
                    let clauses = vec![
                        (Occur::Must, Box::new(AllQuery) as Box<dyn Query>),
                        (Occur::MustNot, q),
                    ];
                    Ok(Some(Box::new(BooleanQuery::new(clauses))))
                } else {
                    Ok(None)
                }
            }
            QueryExpr::Field { .. } => Err(CompileError {
                message: "nested field queries not supported".into(),
            }),
            QueryExpr::Boost {
                expr: inner,
                factor,
            } => {
                // Apply both the field boost and the explicit boost
                if let Some(q) = self.compile_single_field_query(field, boost_value, inner)? {
                    Ok(Some(Box::new(BoostQuery::new(q, *factor))))
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Compiles a phrase query for a single field.
    fn compile_single_field_phrase(
        &self,
        field: Field,
        boost_value: f32,
        tokens: &[String],
    ) -> Result<Option<Box<dyn Query>>, CompileError> {
        if tokens.is_empty() {
            return Ok(None);
        }
        if tokens.len() == 1 {
            return Ok(self.build_single_field_term_query(field, boost_value, &tokens[0]));
        }

        let terms: Vec<Term> = tokens
            .iter()
            .map(|t| Term::from_field_text(field, t))
            .collect();
        let phrase_query = PhraseQuery::new(terms);
        let boosted: Box<dyn Query> =
            Box::new(BoostQuery::new(Box::new(phrase_query), boost_value));
        Ok(Some(boosted))
    }

    /// Compiles a tree filter query.
    ///
    /// The tree field is STRING (not tokenized), so we use exact matching.
    fn compile_tree_query(
        &mut self,
        expr: &QueryExpr,
    ) -> Result<Option<Box<dyn Query>>, CompileError> {
        match expr {
            QueryExpr::Term(text) => {
                // Tree field uses raw tokenizer, so no stemming/lowercasing
                let term = Term::from_field_text(self.schema.tree, text);
                let query: Box<dyn Query> =
                    Box::new(TermQuery::new(term, IndexRecordOption::Basic));
                Ok(Some(query))
            }
            QueryExpr::Or(exprs) => {
                let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
                for e in exprs {
                    if let Some(q) = self.compile_tree_query(e)? {
                        clauses.push((Occur::Should, q));
                    }
                }
                if clauses.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(Box::new(BooleanQuery::new(clauses))))
                }
            }
            _ => Err(CompileError {
                message: "tree: only supports terms or OR of terms".into(),
            }),
        }
    }

    /// Builds a multi-field term query with boosts and optional fuzzy matching.
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
                let query: Box<dyn Query> = if self.fuzzy_distance > 0 {
                    Box::new(FuzzyTermQuery::new(term, self.fuzzy_distance, true))
                } else {
                    Box::new(TermQuery::new(term, IndexRecordOption::WithFreqs))
                };
                let boosted: Box<dyn Query> = Box::new(BoostQuery::new(query, boost_value));
                (Occur::Should, boosted)
            })
            .collect();

        Some(Box::new(BooleanQuery::new(clauses)))
    }

    /// Builds a single-field term query with boost and optional fuzzy matching.
    fn build_single_field_term_query(
        &self,
        field: Field,
        boost_value: f32,
        term_text: &str,
    ) -> Option<Box<dyn Query>> {
        let term = Term::from_field_text(field, term_text);
        let query: Box<dyn Query> = if self.fuzzy_distance > 0 {
            Box::new(FuzzyTermQuery::new(term, self.fuzzy_distance, true))
        } else {
            Box::new(TermQuery::new(term, IndexRecordOption::WithFreqs))
        };
        let boosted: Box<dyn Query> = Box::new(BoostQuery::new(query, boost_value));
        Some(boosted)
    }

    /// Tokenizes text using the configured analyzer.
    fn tokenize(&mut self, text: &str) -> Vec<String> {
        let mut stream = self.analyzer.token_stream(text);
        let mut tokens = Vec::new();
        while let Some(token) = stream.next() {
            tokens.push(token.text.clone());
        }
        tokens
    }
}

#[cfg(test)]
mod tests {
    use ra_query::parse;

    use super::*;

    fn compile_query(input: &str) -> Option<Box<dyn Query>> {
        let schema = IndexSchema::new();
        let mut compiler = QueryCompiler::new(schema, "english", 0).unwrap();
        let expr = parse(input).unwrap()?;
        compiler.compile(&expr).unwrap()
    }

    fn compile_query_fuzzy(input: &str, fuzzy: u8) -> Option<Box<dyn Query>> {
        let schema = IndexSchema::new();
        let mut compiler = QueryCompiler::new(schema, "english", fuzzy).unwrap();
        let expr = parse(input).unwrap()?;
        compiler.compile(&expr).unwrap()
    }

    #[test]
    fn empty_query() {
        assert!(compile_query("").is_none());
        assert!(compile_query("   ").is_none());
    }

    #[test]
    fn single_term() {
        let q = compile_query("rust");
        assert!(q.is_some());
    }

    #[test]
    fn multiple_terms_and() {
        let q = compile_query("rust async");
        assert!(q.is_some());
    }

    #[test]
    fn phrase_query() {
        let q = compile_query("\"error handling\"");
        assert!(q.is_some());
    }

    #[test]
    fn negation_with_term() {
        let q = compile_query("rust -deprecated");
        assert!(q.is_some());
    }

    #[test]
    fn negation_only() {
        // Negation alone uses AllQuery as base
        let q = compile_query("-deprecated");
        assert!(q.is_some());
    }

    #[test]
    fn or_query() {
        let q = compile_query("rust OR golang");
        assert!(q.is_some());
    }

    #[test]
    fn complex_query() {
        let q = compile_query("title:guide (rust OR golang) -deprecated");
        assert!(q.is_some());
    }

    #[test]
    fn field_title() {
        let q = compile_query("title:guide");
        assert!(q.is_some());
    }

    #[test]
    fn field_tags() {
        let q = compile_query("tags:tutorial");
        assert!(q.is_some());
    }

    #[test]
    fn field_body() {
        let q = compile_query("body:implementation");
        assert!(q.is_some());
    }

    #[test]
    fn field_path() {
        let q = compile_query("path:handlers");
        assert!(q.is_some());
    }

    #[test]
    fn field_tree() {
        let q = compile_query("tree:docs");
        assert!(q.is_some());
    }

    #[test]
    fn field_tree_or() {
        let q = compile_query("tree:(docs OR examples)");
        assert!(q.is_some());
    }

    #[test]
    fn unknown_field_error() {
        let schema = IndexSchema::new();
        let mut compiler = QueryCompiler::new(schema, "english", 0).unwrap();
        let expr = parse("unknown:value").unwrap().unwrap();
        let result = compiler.compile(&expr);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("unknown field"));
    }

    #[test]
    fn field_with_phrase() {
        let q = compile_query("title:\"getting started\"");
        assert!(q.is_some());
    }

    #[test]
    fn field_with_or() {
        let q = compile_query("title:(rust OR golang)");
        assert!(q.is_some());
    }

    #[test]
    fn fuzzy_matching() {
        let q = compile_query_fuzzy("rust", 1);
        assert!(q.is_some());
    }

    #[test]
    fn multiple_negations() {
        let q = compile_query("rust -deprecated -legacy");
        assert!(q.is_some());
    }

    #[test]
    fn negated_phrase() {
        let q = compile_query("-\"error handling\"");
        assert!(q.is_some());
    }

    #[test]
    fn grouped_or_with_and() {
        let q = compile_query("(rust async) OR (go goroutine)");
        assert!(q.is_some());
    }

    #[test]
    fn boosted_term() {
        let q = compile_query("rust^2.5");
        assert!(q.is_some());
    }

    #[test]
    fn boosted_phrase() {
        let q = compile_query("\"error handling\"^3.0");
        assert!(q.is_some());
    }

    #[test]
    fn boosted_group() {
        let q = compile_query("(rust async)^2.0");
        assert!(q.is_some());
    }

    #[test]
    fn boosted_or_terms() {
        let q = compile_query("rust^2.5 OR golang^1.5");
        assert!(q.is_some());
    }

    #[test]
    fn boosted_field() {
        let q = compile_query("title:guide^2.5");
        assert!(q.is_some());
    }

    #[test]
    fn boosted_in_complex_query() {
        let q = compile_query("title:guide^2.0 (rust^3.0 OR golang^2.5) -deprecated");
        assert!(q.is_some());
    }
}
