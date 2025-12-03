//! Text analysis pipeline for the ra search index.
//!
//! Implements a four-stage text analysis pipeline:
//! 1. `SimpleTokenizer` - splits on whitespace and punctuation
//! 2. `LowerCaser` - converts tokens to lowercase
//! 3. `RemoveLongFilter` - removes tokens longer than 40 bytes
//! 4. `Stemmer` - applies language-specific stemming
//!
//! The stemmer language is configurable via the `stemmer` setting in `.ra.toml`.

use tantivy::tokenizer::{
    Language, LowerCaser, RemoveLongFilter, SimpleTokenizer, Stemmer, TextAnalyzer,
};

use crate::IndexError;

/// Name of the custom tokenizer registered with Tantivy.
pub const RA_TOKENIZER: &str = "ra_text";

/// Maximum token length in bytes before filtering.
const MAX_TOKEN_LENGTH: usize = 40;

/// Parses a stemmer language string into a Tantivy `Language`.
///
/// Supports lowercase language names matching Tantivy's `Language` enum.
/// Returns an error if the language is not recognized.
pub fn parse_language(name: &str) -> Result<Language, IndexError> {
    match name.to_lowercase().as_str() {
        "arabic" => Ok(Language::Arabic),
        "danish" => Ok(Language::Danish),
        "dutch" => Ok(Language::Dutch),
        "english" => Ok(Language::English),
        "finnish" => Ok(Language::Finnish),
        "french" => Ok(Language::French),
        "german" => Ok(Language::German),
        "greek" => Ok(Language::Greek),
        "hungarian" => Ok(Language::Hungarian),
        "italian" => Ok(Language::Italian),
        "norwegian" => Ok(Language::Norwegian),
        "portuguese" => Ok(Language::Portuguese),
        "romanian" => Ok(Language::Romanian),
        "russian" => Ok(Language::Russian),
        "spanish" => Ok(Language::Spanish),
        "swedish" => Ok(Language::Swedish),
        "tamil" => Ok(Language::Tamil),
        "turkish" => Ok(Language::Turkish),
        other => Err(IndexError::InvalidLanguage(other.to_string())),
    }
}

/// Builds the ra text analyzer with the specified stemmer language.
///
/// The pipeline is:
/// 1. `SimpleTokenizer` - splits text on whitespace and punctuation
/// 2. `LowerCaser` - normalizes tokens to lowercase
/// 3. `RemoveLongFilter` - removes tokens > 40 bytes
/// 4. `Stemmer` - applies language-specific stemming
pub fn build_analyzer(language: Language) -> TextAnalyzer {
    TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(LowerCaser)
        .filter(RemoveLongFilter::limit(MAX_TOKEN_LENGTH))
        .filter(Stemmer::new(language))
        .build()
}

/// Builds the ra text analyzer from a language name string.
///
/// Convenience function combining [`parse_language`] and [`build_analyzer`].
pub fn build_analyzer_from_name(language_name: &str) -> Result<TextAnalyzer, IndexError> {
    let language = parse_language(language_name)?;
    Ok(build_analyzer(language))
}

#[cfg(test)]
mod test {
    use std::iter;

    use tantivy::tokenizer::TokenStream;

    use super::*;

    #[test]
    fn parse_all_languages() {
        let languages = [
            ("arabic", Language::Arabic),
            ("danish", Language::Danish),
            ("dutch", Language::Dutch),
            ("english", Language::English),
            ("finnish", Language::Finnish),
            ("french", Language::French),
            ("german", Language::German),
            ("greek", Language::Greek),
            ("hungarian", Language::Hungarian),
            ("italian", Language::Italian),
            ("norwegian", Language::Norwegian),
            ("portuguese", Language::Portuguese),
            ("romanian", Language::Romanian),
            ("russian", Language::Russian),
            ("spanish", Language::Spanish),
            ("swedish", Language::Swedish),
            ("tamil", Language::Tamil),
            ("turkish", Language::Turkish),
        ];

        for (name, expected) in languages {
            assert_eq!(
                parse_language(name).unwrap(),
                expected,
                "failed to parse {name}"
            );
        }
    }

    #[test]
    fn parse_case_insensitive() {
        assert_eq!(parse_language("English").unwrap(), Language::English);
        assert_eq!(parse_language("FRENCH").unwrap(), Language::French);
        assert_eq!(parse_language("GeRmAn").unwrap(), Language::German);
    }

    #[test]
    fn parse_invalid_language() {
        let err = parse_language("klingon").unwrap_err();
        assert!(err.to_string().contains("klingon"));
    }

    #[test]
    fn analyzer_lowercases() {
        let mut analyzer = build_analyzer(Language::English);
        let mut stream = analyzer.token_stream("HELLO World");

        let token = stream.next().unwrap();
        assert_eq!(token.text, "hello");

        let token = stream.next().unwrap();
        assert_eq!(token.text, "world");

        assert!(stream.next().is_none());
    }

    #[test]
    fn analyzer_stems_english() {
        let mut analyzer = build_analyzer(Language::English);
        let mut stream = analyzer.token_stream("handling running");

        let token = stream.next().unwrap();
        assert_eq!(token.text, "handl");

        let token = stream.next().unwrap();
        assert_eq!(token.text, "run");

        assert!(stream.next().is_none());
    }

    #[test]
    fn analyzer_removes_long_tokens() {
        let mut analyzer = build_analyzer(Language::English);
        let long_token = "a".repeat(50);
        let text = format!("short {long_token} word");
        let mut stream = analyzer.token_stream(&text);

        let token = stream.next().unwrap();
        assert_eq!(token.text, "short");

        let token = stream.next().unwrap();
        assert_eq!(token.text, "word");

        assert!(stream.next().is_none());
    }

    #[test]
    fn analyzer_splits_punctuation() {
        let mut analyzer = build_analyzer(Language::English);
        let mut stream = analyzer.token_stream("hello, world! foo-bar");

        let tokens: Vec<_> = iter::from_fn(|| stream.next().map(|t| t.text.clone())).collect();
        assert_eq!(tokens, vec!["hello", "world", "foo", "bar"]);
    }

    #[test]
    fn build_from_name() {
        let mut analyzer = build_analyzer_from_name("english").unwrap();
        let mut stream = analyzer.token_stream("testing");
        let token = stream.next().unwrap();
        assert_eq!(token.text, "test");
    }

    #[test]
    fn build_from_invalid_name() {
        let result = build_analyzer_from_name("invalid");
        assert!(result.is_err());
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("expected error"),
        };
        assert!(err.to_string().contains("invalid"));
    }
}
