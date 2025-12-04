//! Query lexer (tokenizer).
//!
//! Converts a query string into a stream of tokens for the parser.

use std::{iter::Peekable, str::Chars};

/// A token in the query language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    /// A bare word (search term).
    Term(String),

    /// A quoted phrase (the quotes are stripped, content preserved).
    Phrase(String),

    /// The OR keyword.
    Or,

    /// Negation prefix (-).
    Not,

    /// Left parenthesis.
    LParen,

    /// Right parenthesis.
    RParen,

    /// Field prefix (e.g., "title:" produces FieldPrefix("title")).
    FieldPrefix(String),
}

/// Lexer error with position information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexError {
    /// Error message.
    pub message: String,
    /// Byte position in input where error occurred.
    pub position: usize,
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "at position {}: {}", self.position, self.message)
    }
}

impl std::error::Error for LexError {}

/// Tokenizes a query string.
pub struct Lexer<'a> {
    input: &'a str,
    chars: Peekable<Chars<'a>>,
    position: usize,
}

impl<'a> Lexer<'a> {
    /// Creates a new lexer for the given input.
    pub fn new(input: &'a str) -> Self {
        Self {
            input,
            chars: input.chars().peekable(),
            position: 0,
        }
    }

    /// Tokenizes the entire input, returning all tokens or an error.
    pub fn tokenize(mut self) -> Result<Vec<Token>, LexError> {
        let mut tokens = Vec::new();

        while let Some(token) = self.next_token()? {
            tokens.push(token);
        }

        Ok(tokens)
    }

    /// Returns the next token, or None if at end of input.
    fn next_token(&mut self) -> Result<Option<Token>, LexError> {
        self.skip_whitespace();

        let Some(&ch) = self.chars.peek() else {
            return Ok(None);
        };

        match ch {
            '"' => self.read_phrase(),
            '(' => {
                self.advance();
                Ok(Some(Token::LParen))
            }
            ')' => {
                self.advance();
                Ok(Some(Token::RParen))
            }
            '-' => {
                self.advance();
                Ok(Some(Token::Not))
            }
            _ => self.read_term_or_keyword(),
        }
    }

    /// Reads a quoted phrase.
    fn read_phrase(&mut self) -> Result<Option<Token>, LexError> {
        let start_pos = self.position;
        self.advance(); // consume opening quote

        let mut content = String::new();

        loop {
            match self.chars.peek() {
                Some(&'"') => {
                    self.advance(); // consume closing quote
                    return Ok(Some(Token::Phrase(content)));
                }
                Some(&ch) => {
                    content.push(ch);
                    self.advance();
                }
                None => {
                    // Unclosed quote - treat rest as phrase content
                    return Err(LexError {
                        message: "unclosed quote".into(),
                        position: start_pos,
                    });
                }
            }
        }
    }

    /// Reads a term, keyword (OR), or field prefix.
    fn read_term_or_keyword(&mut self) -> Result<Option<Token>, LexError> {
        let mut word = String::new();

        while let Some(&ch) = self.chars.peek() {
            if ch.is_whitespace() || ch == '(' || ch == ')' || ch == '"' {
                break;
            }

            // Check for field prefix (word ending in colon)
            if ch == ':' {
                self.advance(); // consume the colon
                if word.is_empty() {
                    // Bare colon, treat as part of next term
                    continue;
                }
                return Ok(Some(Token::FieldPrefix(word)));
            }

            word.push(ch);
            self.advance();
        }

        if word.is_empty() {
            return Ok(None);
        }

        // Check for OR keyword (case-insensitive)
        if word.eq_ignore_ascii_case("OR") {
            return Ok(Some(Token::Or));
        }

        Ok(Some(Token::Term(word)))
    }

    /// Skips whitespace characters.
    fn skip_whitespace(&mut self) {
        while let Some(&ch) = self.chars.peek() {
            if ch.is_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
    }

    /// Advances to the next character.
    fn advance(&mut self) {
        if let Some(ch) = self.chars.next() {
            self.position += ch.len_utf8();
        }
    }
}

/// Convenience function to tokenize a query string.
pub fn tokenize(input: &str) -> Result<Vec<Token>, LexError> {
    Lexer::new(input).tokenize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input() {
        assert_eq!(tokenize("").unwrap(), vec![]);
    }

    #[test]
    fn whitespace_only() {
        assert_eq!(tokenize("   ").unwrap(), vec![]);
    }

    #[test]
    fn single_term() {
        assert_eq!(tokenize("rust").unwrap(), vec![Token::Term("rust".into())]);
    }

    #[test]
    fn multiple_terms() {
        assert_eq!(
            tokenize("rust async").unwrap(),
            vec![Token::Term("rust".into()), Token::Term("async".into())]
        );
    }

    #[test]
    fn quoted_phrase() {
        assert_eq!(
            tokenize("\"hello world\"").unwrap(),
            vec![Token::Phrase("hello world".into())]
        );
    }

    #[test]
    fn unclosed_quote_error() {
        let err = tokenize("\"hello world").unwrap_err();
        assert_eq!(err.position, 0);
        assert!(err.message.contains("unclosed"));
    }

    #[test]
    fn or_keyword() {
        assert_eq!(
            tokenize("rust OR golang").unwrap(),
            vec![
                Token::Term("rust".into()),
                Token::Or,
                Token::Term("golang".into())
            ]
        );
    }

    #[test]
    fn or_case_insensitive() {
        assert_eq!(
            tokenize("rust or golang").unwrap(),
            vec![
                Token::Term("rust".into()),
                Token::Or,
                Token::Term("golang".into())
            ]
        );
        assert_eq!(
            tokenize("rust Or golang").unwrap(),
            vec![
                Token::Term("rust".into()),
                Token::Or,
                Token::Term("golang".into())
            ]
        );
    }

    #[test]
    fn negation() {
        assert_eq!(
            tokenize("-deprecated").unwrap(),
            vec![Token::Not, Token::Term("deprecated".into())]
        );
    }

    #[test]
    fn negation_with_terms() {
        assert_eq!(
            tokenize("rust -deprecated").unwrap(),
            vec![
                Token::Term("rust".into()),
                Token::Not,
                Token::Term("deprecated".into())
            ]
        );
    }

    #[test]
    fn parentheses() {
        assert_eq!(
            tokenize("(rust async)").unwrap(),
            vec![
                Token::LParen,
                Token::Term("rust".into()),
                Token::Term("async".into()),
                Token::RParen
            ]
        );
    }

    #[test]
    fn field_prefix() {
        assert_eq!(
            tokenize("title:guide").unwrap(),
            vec![
                Token::FieldPrefix("title".into()),
                Token::Term("guide".into())
            ]
        );
    }

    #[test]
    fn field_prefix_with_other_terms() {
        assert_eq!(
            tokenize("title:guide rust").unwrap(),
            vec![
                Token::FieldPrefix("title".into()),
                Token::Term("guide".into()),
                Token::Term("rust".into())
            ]
        );
    }

    #[test]
    fn complex_query() {
        assert_eq!(
            tokenize("title:guide (rust OR golang) -deprecated").unwrap(),
            vec![
                Token::FieldPrefix("title".into()),
                Token::Term("guide".into()),
                Token::LParen,
                Token::Term("rust".into()),
                Token::Or,
                Token::Term("golang".into()),
                Token::RParen,
                Token::Not,
                Token::Term("deprecated".into())
            ]
        );
    }

    #[test]
    fn phrase_in_complex_query() {
        assert_eq!(
            tokenize("\"error handling\" OR logging").unwrap(),
            vec![
                Token::Phrase("error handling".into()),
                Token::Or,
                Token::Term("logging".into())
            ]
        );
    }

    #[test]
    fn extra_whitespace() {
        assert_eq!(
            tokenize("  rust   async  ").unwrap(),
            vec![Token::Term("rust".into()), Token::Term("async".into())]
        );
    }

    #[test]
    fn field_with_phrase() {
        assert_eq!(
            tokenize("title:\"getting started\"").unwrap(),
            vec![
                Token::FieldPrefix("title".into()),
                Token::Phrase("getting started".into())
            ]
        );
    }
}
