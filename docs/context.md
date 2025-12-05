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
   analysis (directory names, filename) and content parsers (e.g., markdown
   headings vs body text).

2. **Term Ranking**: Weight terms by source (path > headers > body), filter
   stopwords (English + Rust keywords), and score using TF-IDF with IDF values
   from the Tantivy index. Terms not in the index are filtered out.

3. **Query Construction**: Select the top N terms by score, boost each by its
   TF-IDF score, and build an OR query: `term1^5.2 OR term2^3.1 OR ...`

4. **Search Execution**: Execute the generated query via the standard ra search
   pipeline.


## Term Extraction

The source file being analyzed does not need to be in the index. Context search
operates on any file you can read, extracting terms from its content and using
the index only to compute IDF values for ranking.

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

Path components are split on `_`, `-`, `.` delimiters. Terms are filtered by
minimum length (default 3 characters) and against stopwords.

### Parsers

Parsers extract terms from source files and assign weights based on structural
context. The weight flows through to query construction, where higher-weighted
terms contribute more to search ranking.

#### Parser Interface

Each parser implements a common interface:

```rust
trait ContentParser {
    /// Returns true if this parser handles the given file.
    fn can_parse(&self, path: &Path) -> bool;

    /// Extract weighted terms from file content.
    fn parse(&self, path: &Path, content: &str) -> Vec<WeightedTerm>;
}

struct WeightedTerm {
    term: String,
    weight: f32,
    source: String,  // Human-readable label (e.g., "md:h1", "body")
    frequency: u32,
}
```

#### Markdown Parser

Uses the `ra-document` parser to extract structural elements:

| Element | Weight | Source Label |
|---------|--------|--------------|
| h1 headers | 3.0 | `md:h1` |
| h2-h3 headers | 2.0 | `md:h2-h3` |
| h4-h6 headers | 1.5 | `md:h4-h6` |
| Body text | 1.0 | `body` |

YAML frontmatter is skipped during parsing.

#### Text Parser (Fallback)

For unsupported file types:
1. Split on whitespace and punctuation
2. Filter tokens by length (minimum 3 characters)
3. Apply stopword filtering
4. All terms receive body weight (1.0)

#### Parser Selection

Parsers are selected by file extension:

| Extensions | Parser |
|------------|--------|
| `.md`, `.markdown` | Markdown |
| (other) | Text (fallback) |


## Term Ranking

After extraction, terms are ranked to identify the most salient concepts.

### TF-IDF Scoring

Each term receives a score combining:

- **Term Frequency (TF)**: How often the term appears in the source file
- **Source Weight**: Weight based on where the term was found (heading > body)
- **Inverse Document Frequency (IDF)**: From the Tantivy index - terms rare
  across the knowledge base score higher

```
score(term) = frequency × source_weight × IDF
```

The IDF is computed from the index, so domain-specific terms that are rare in
your knowledge base (like character names) naturally score higher than common
words.

### Index Filtering

Terms that don't appear in the index receive no IDF and are filtered out during
ranking. This ensures the generated query only contains terms that can actually
match documents.

### Stopword Filtering

Two categories of stopwords are filtered:

**English stopwords**: Standard set from the `stop-words` crate including
articles, prepositions, conjunctions, common verbs.

**Rust stopwords**: Keywords (`fn`, `let`, `impl`, `struct`, `trait`, `async`,
etc.), reserved keywords, primitive types (`i32`, `bool`, `str`, etc.), and
common standard library types (`Option`, `Result`, `Vec`, `String`, `Clone`,
`Debug`, etc.).

Stopwords are applied during term extraction, before TF-IDF scoring.


## Query Construction

### Term Selection

Select the top N terms by score. The default N is configurable:
- CLI flag: `--terms N`
- Default: 15

### Per-Term Boosting

Each term in the generated query is boosted by its TF-IDF score. Terms from
high-signal sources (headers, filenames) with high IDF (rare in the corpus)
receive higher boosts.

The query is built programmatically using Tantivy's `BoostQuery`:

```rust
let mut exprs = Vec::new();
for term in top_terms {
    let term_expr = QueryExpr::Term(term.term.clone());
    let boosted = QueryExpr::boost(term_expr, term.score);
    exprs.push(boosted);
}
let query = QueryExpr::or(exprs);
```

### Query Output

The generated query uses boost notation:

```
kubernetes^12.5 OR orchestration^8.3 OR container^5.1 OR deployment^4.2
```

This syntax is also available in `ra search` for manual queries.


## CLI Interface

### Basic Usage

```bash
# Find context for a single file
ra context chapter1.md

# Find context for multiple files
ra context chapter1.md chapter2.md

# Limit number of results
ra context -n 20 chapter1.md

# Limit to specific trees
ra context -t docs chapter1.md
```

### Explain Mode

Show the generated query without executing the search:

```bash
$ ra context --explain chapter1.md

File: chapter1.md

Ranked terms:
┌───────────────┬──────────┬────────┬──────┬───────┬────────┐
│ Term          │ Source   │ Weight │ Freq │ IDF   │ Score  │
├───────────────┼──────────┼────────┼──────┼───────┼────────┤
│ ashford       │ body     │ 1.0    │ 7    │ 4.23  │ 29.61  │
│ thornwood     │ body     │ 1.0    │ 3    │ 5.12  │ 15.36  │
│ rebellion     │ md:h2-h3 │ 2.0    │ 2    │ 3.45  │ 13.80  │
└───────────────┴──────────┴────────┴──────┴───────┴────────┘

Generated query:
  ashford^29.61 OR thornwood^15.36 OR rebellion^13.80 OR ...
```

### Output Modes

Same as `ra search`:
- Default: full content with highlighting
- `--list`: titles only
- `--json`: structured JSON output

### Flags

| Flag | Description |
|------|-------------|
| `--explain` | Show extracted terms and query without searching |
| `-n, --limit N` | Maximum results to return (default: 10) |
| `--terms N` | Maximum terms in generated query (default: 15) |
| `-t, --tree NAME` | Limit results to specific tree(s) |
| `--list` | Output titles only |
| `--json` | Output in JSON format |


## Implementation Notes

### Crate Structure

The implementation lives in `ra-context`:

```
ra-context/
├── src/
│   ├── lib.rs           # Public API, path term extraction
│   ├── analyze.rs       # Main analysis coordinator
│   ├── parser/
│   │   ├── mod.rs       # Parser trait and utilities
│   │   ├── markdown.rs  # Markdown parser
│   │   └── text.rs      # Plain text parser (fallback)
│   ├── rank.rs          # TF-IDF ranking
│   ├── query.rs         # Query construction
│   ├── stopwords.rs     # Stopword lists
│   └── term.rs          # WeightedTerm type
```

### Crate Dependencies

```
ra-context
├── ra-query      # For QueryExpr construction
├── ra-document   # For markdown parsing (headings, frontmatter)
└── stop-words    # For English stopwords
```

The main entry point accepts an `IdfProvider` trait for index access:

```rust
pub trait IdfProvider {
    fn idf(&self, term: &str) -> Option<f32>;
}

pub fn analyze_context<I: IdfProvider>(
    path: &Path,
    content: &str,
    idf_provider: &I,
    config: &AnalysisConfig,
) -> ContextAnalysis;
```

### Tree Filtering

When `--tree` is specified, IDF lookups are filtered to only consider documents
in the specified trees. This ensures the generated query is tuned to the
relevant subset of the knowledge base.
