# Context Search

The `ra context` command analyzes source files and automatically generates search
queries to find relevant background material from across the knowledge base.

## Use Case

When working on a file, you often need context from related documentation. For
example, when editing a novel chapter that mentions "Lord Ashford", "the
rebellion", and "Thornwood Castle", you want to automatically surface:

- `characters/lord-ashford.md`
- `history/the-rebellion.md`
- `locations/thornwood-castle.md`

Rather than manually searching for each concept, `ra context chapter1.md`
extracts salient terms and constructs a query that retrieves all relevant
background material.


## Algorithm Overview

1. **Term Extraction**: Extract candidate terms from the source file using path
   analysis (directory names, filename), language-specific parsers (e.g.,
   markdown headers), and a naive text tokenizer as fallback.

2. **Term Ranking**: Weight terms by source (path > headers > body), filter
   stopwords (English + programming), and score using TF-IDF with IDF values
   from the Tantivy index.

3. **Phrase Detection**: Extract candidate bigrams/trigrams from top-ranked
   terms, validate each against the index to check if it exists as a phrase,
   and promote validated phrases over individual terms.

4. **Query Construction**: Select the top N terms/phrases by score and build an
   OR query: `term1 OR term2 OR "phrase" OR ...`

5. **Search Execution**: Execute the generated query via the standard ra search
   pipeline.


## Term Extraction

The source file being analyzed does not need to be in the index. Context search
operates on any file you can read, extracting terms from its content and using
the index only to compute IDF values and validate phrases.

Terms are extracted from multiple sources with different weights reflecting
their likely importance.

### Source Weights

| Source | Weight | Rationale |
|--------|--------|-----------|
| Path: filename (sans extension) | 4.0 | Intentional human naming |
| Path: directory names | 3.0 | Organizational structure reflects topics |
| Markdown: h1 headers | 3.0 | Primary topic markers |
| Markdown: h2-h3 headers | 2.0 | Secondary topic markers |
| Markdown: h4-h6 headers | 1.5 | Minor topic markers |
| Body text | 1.0 | General content |

### Path Analysis

File paths encode intentional human decisions about organization:

```
src/auth/oauth_handler.rs
     ↓
["auth", "oauth", "handler"]  (weights: 3.0, 3.0, 4.0)
```

Path components are split on `_`, `-`, `.` delimiters. Common directory names
(`src`, `lib`, `test`, `docs`) and file extensions are filtered.

### Parsers

Parsers are responsible for extracting terms from source files and assigning
weights based on the structural context of each term. The weight assigned by
the parser flows through to query construction, where higher-weighted terms
contribute more to search ranking.

#### Parser Interface

Each parser implements a common interface:

```rust
trait ContextParser {
    /// Returns true if this parser handles the given file.
    fn can_parse(&self, path: &Path) -> bool;

    /// Extract weighted terms from file content.
    fn parse(&self, path: &Path, content: &str) -> Vec<WeightedTerm>;
}

struct WeightedTerm {
    term: String,
    weight: f32,
    source: TermSource,  // For --explain output
}
```

#### Naive Text Parser (Fallback)

For unsupported file types:
1. Split on whitespace and punctuation
2. Filter tokens by length (minimum 3 characters)
3. Apply stopword filtering
4. All terms receive body weight (1.0)

**Important**: The naive parser must use the same tokenization as the index
analyzer (SimpleTokenizer → LowerCaser → Stemmer) to ensure extracted terms
match indexed content. See [Text Analysis](search.md#text-analysis) for the
analyzer pipeline.

#### Markdown Parser

Uses the existing `ra-document` parser to extract structural elements:

| Element | Weight | Rationale |
|---------|--------|-----------|
| h1 headers | 3.0 | Primary topic markers |
| h2-h3 headers | 2.0 | Secondary topics |
| h4-h6 headers | 1.5 | Minor topics |
| Body text | 1.0 | General content |

Note: Bold/italic emphasis extraction is not currently supported by
`ra-document` and may be added in a future enhancement.

#### Rust Parser (Future)

Planned extraction with weights:

| Element | Weight | Rationale |
|---------|--------|-----------|
| Crate imports (`use foo::*`) | 2.5 | External dependencies indicate concepts |
| Struct/enum names | 2.0 | Core domain types |
| Trait names | 2.0 | Core abstractions |
| Function names (public) | 1.5 | Public API surface |
| Doc comments | 1.5 | Intentional documentation |
| Type parameters | 1.2 | Generic concepts |
| Function names (private) | 1.0 | Implementation detail |
| Variable names | 0.5 | Local context, low signal |

#### Parser Selection

Parsers are selected by file extension:

| Extensions | Parser |
|------------|--------|
| `.md`, `.markdown` | Markdown |
| `.rs` | Rust (future) |
| `.py` | Python (future) |
| `.js`, `.ts`, `.jsx`, `.tsx` | JavaScript (future) |
| (other) | Naive text |


## Term Ranking

After extraction, terms are ranked to identify the most salient concepts.

### TF-IDF Scoring

Each term receives a score combining:

- **Term Frequency (TF)**: How often the term appears in the source file,
  weighted by source type
- **Inverse Document Frequency (IDF)**: From the Tantivy index - terms rare
  across the knowledge base score higher

```
score(term) = TF(term) × source_weight × IDF(term)
```

The IDF is computed from the index, so domain-specific terms that are rare in
your knowledge base (like character names) naturally score higher than common
words.

### IDF Computation

Tantivy provides document frequency statistics via the `Searcher` API:

```rust
// Get document frequency for a term
let term = Term::from_field_text(body_field, "ashford");
let doc_freq = searcher.doc_freq(&term);
let total_docs = searcher.num_docs();

// Compute IDF (standard formula with smoothing)
let idf = ((total_docs as f32 + 1.0) / (doc_freq as f32 + 1.0)).ln() + 1.0;
```

Terms not found in the index receive a high IDF (treated as maximally rare),
which is appropriate since domain-specific terms not yet documented are likely
important concepts worth surfacing.

### Stopword Filtering

Two categories of stopwords are filtered:

**English stopwords**: Standard set including articles, prepositions,
conjunctions, common verbs (the, a, an, is, are, was, were, have, has, do, does,
will, would, could, should, etc.)

**Programming stopwords**: Common technical terms that rarely indicate specific
concepts (function, class, method, return, error, data, value, type, variable,
parameter, argument, etc.)

Stopwords are applied before TF-IDF scoring to avoid wasting ranking capacity on
uninformative terms.


## Phrase Detection

Multi-word concepts like "Lord Ashford" or "binding ritual" should be searched
as phrases rather than individual terms.

### Candidate Extraction

From the top-ranked individual terms, extract candidate phrases:
1. Find adjacent terms in the original text
2. Generate bigrams and trigrams from these adjacencies
3. Filter candidates by combined score threshold

### Index Validation

For each candidate phrase, query the Tantivy index:
```
Does "Lord Ashford" exist as a phrase in any indexed document?
```

If the phrase returns results, it's a meaningful concept in the knowledge base
and should be searched as `"Lord Ashford"` rather than `Lord OR Ashford`.

### Phrase Promotion

Validated phrases replace their constituent terms in the final query, avoiding
redundancy:

```
Before: Lord OR Ashford OR rebellion OR Thornwood OR Castle
After:  "Lord Ashford" OR rebellion OR "Thornwood Castle"
```


## Query Construction

### Term Selection

Select the top N terms/phrases by score. The default N is configurable:
- CLI flag: `--terms N`
- Config: `context.max_terms`
- Default: 15

### Per-Term Boosting

Each term in the generated query carries its computed weight as a boost factor.
Terms from high-signal sources (headers, struct names) boost search results more
than terms from body text.

Tantivy supports per-term boosting via `BoostQuery`, which multiplies the score
contribution of a wrapped query by a boost factor. The context query is
constructed programmatically using this mechanism:

```rust
// Simplified example
let mut clauses = Vec::new();
for weighted_term in selected_terms {
    let term_query = build_term_query(&weighted_term.term);
    let boosted = BoostQuery::new(term_query, weighted_term.weight);
    clauses.push((Occur::Should, boosted));
}
let query = BooleanQuery::new(clauses);
```

### Query Syntax Extension

To support `--explain` output showing the weighted query in human-readable form,
the query syntax is extended with boost notation:

```
term^2.5              # term with boost 2.5
"phrase"^3.0          # phrase with boost 3.0
```

The `--explain` output displays the generated query:

```
"Lord Ashford"^4.2 OR "Thornwood Castle"^3.9 OR rebellion^3.1 OR binding^2.5
```

This syntax extension is also available in `ra search` for manual queries,
though it's primarily intended for context query inspection.

### OR Query Structure

The final query joins all selected terms with OR (disjunction):

```
term1^w1 OR term2^w2 OR "phrase"^w3 OR ...
```

The OR structure ensures broad coverage - any document mentioning any of these
concepts is a candidate. The per-term boosts then influence final ranking so
that matches on high-signal terms score higher.


## CLI Interface

### Basic Usage

```bash
# Find context for a single file
ra context chapter1.md

# Find context for multiple files
ra context chapter1.md chapter2.md

# Limit number of results
ra context -n 20 chapter1.md
```

### Explain Mode

Show the generated query without executing the search:

```bash
$ ra context --explain chapter1.md

Extracted terms (by score):
  4.23  "Lord Ashford"     (phrase, from body)
  3.87  "Thornwood Castle" (phrase, from body)
  3.12  rebellion          (from body, freq: 7)
  2.89  chapter            (from path)
  2.45  binding            (from body, freq: 3)
  ...

Generated query:
  "Lord Ashford" OR "Thornwood Castle" OR rebellion OR binding OR ritual
```

### Output Modes

Same as `ra search`:
- Default: full content with highlighting
- `--list`: titles and snippets only
- `--json`: structured JSON output

### Flags

| Flag | Description |
|------|-------------|
| `--explain` | Show extracted terms and query without searching |
| `-n, --limit N` | Maximum results to return (default: 10) |
| `--terms N` | Maximum terms in generated query (default: 15) |
| `-t, --tree NAME` | Limit results to specific tree(s) |
| `--list` | Output titles and snippets only |
| `--json` | Output in JSON format |


## Configuration

Settings in `.ra.toml` under `[context]`. This is a new configuration section
that will be added to `ra-config`:

```toml
[context]
# Maximum terms in generated query
max_terms = 15

# Minimum term score to include (0.0 to disable)
min_score = 0.0

# Additional stopwords beyond defaults
stopwords = ["myproject", "internal"]

# Source weight overrides
[context.weights]
path_filename = 4.0
path_directory = 3.0
markdown_h1 = 3.0
markdown_h2 = 2.0
body = 1.0
```

The `ContextConfig` struct will be added to `ra-config` alongside existing
configuration types, with sensible defaults that can be overridden per-project.


## Implementation Notes

### Crate Structure

The implementation lives in `ra-context`, expanding the existing crate:

```
ra-context/
├── src/
│   ├── lib.rs           # Public API
│   ├── extract.rs       # Term extraction coordinator
│   ├── parser/
│   │   ├── mod.rs       # Parser trait and registry
│   │   ├── text.rs      # Naive text parser (fallback)
│   │   └── markdown.rs  # Markdown parser
│   ├── rank.rs          # TF-IDF ranking
│   ├── phrase.rs        # Phrase detection
│   ├── query.rs         # Query construction
│   └── stopwords.rs     # Stopword lists
```

### Crate Dependencies

The `ra-context` crate requires access to the Tantivy index for IDF computation
and phrase validation. This creates a dependency on `ra-index`:

```
ra-context
├── ra-index        # For Searcher access (IDF, phrase validation)
├── ra-document     # For markdown parsing
└── ra-config       # For configuration
```

The main entry point accepts a `Searcher` reference:

```rust
pub fn analyze_context(
    path: &Path,
    content: &str,
    searcher: &Searcher,
    config: &ContextConfig,
) -> ContextAnalysis { ... }
```

### Migration from Current Implementation

The current `ra-context` implementation has a simpler `ContextSignals` struct
that extracts path and pattern terms without weights. This will be replaced:

**Current** (to be removed):
```rust
pub struct ContextSignals {
    pub path_terms: Vec<String>,
    pub pattern_terms: Vec<String>,
    pub content_sample: Option<String>,
}
```

**New**:
```rust
pub struct ContextAnalysis {
    /// Ranked terms with weights and source information.
    pub terms: Vec<WeightedTerm>,
    /// The generated query expression.
    pub query: QueryExpr,
    /// Human-readable query string for --explain output.
    pub query_string: String,
}

pub struct WeightedTerm {
    pub term: String,
    pub weight: f32,
    pub source: TermSource,
    pub frequency: u32,
}
```

The existing `search_context` function in `ra-index` will be updated to use the
new `ContextAnalysis` output.

### Query System Changes

The `ra-index` query system requires extension to support per-term boosting:

1. **AST Extension**: Add `Boosted(Box<QueryExpr>, f32)` variant to `QueryExpr`

2. **Parser Extension**: Support `term^2.5` syntax in the query parser

3. **Compiler Extension**: Handle `Boosted` by wrapping the inner query with
   `BoostQuery`

These changes enable both:
- Programmatic query construction with boosts (for context queries)
- Human-readable query display (for `--explain`)
- Manual boosted queries in `ra search` (bonus)

### Index Integration

The ranker needs access to the Tantivy index for:
1. IDF computation (term document frequencies)
2. Phrase validation (phrase query existence check)

This requires passing an index reader or searcher to the context analyzer.

### Performance Considerations

- **IDF lookup**: Batch term lookups to minimize index access
- **Phrase validation**: Limit candidate phrases to avoid excessive queries
- **Caching**: Consider caching IDF values for repeated context queries

### Future Enhancements

- Language-specific parsers for Rust, Python, JavaScript
- Configurable source weights per file type
- Entity recognition for proper nouns
- Learning from user feedback (which results were useful)
