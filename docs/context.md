# Context Search

The `ra context` command analyzes source files and automatically generates search queries to
find relevant documentation.


## Use Case

When working on a file, you need context from related documentation. For example, editing a
novel chapter mentioning "Lord Ashford", "the rebellion", and "Thornwood Castle" should
surface:

- `characters/lord-ashford.md`
- `history/the-rebellion.md`
- `locations/thornwood-castle.md`

Rather than searching manually, `ra context chapter1.md` extracts salient terms and
constructs a query that retrieves all relevant material.


## How It Works

### 1. Term Extraction

ra extracts candidate terms from the source file using two signal types:

**Path analysis**: Directory names and filename provide high-signal terms.

```
src/auth/oauth_handler.rs
→ ["auth", "oauth", "handler"]  (weights: 3.0, 3.0, 4.0)
```

**Content analysis**: For markdown files, terms from headings receive higher weight than
body text. For other files, all tokens are treated equally.

### 2. Term Ranking

Terms are scored using TF-IDF:

```
score(term) = frequency × source_weight × IDF
```

- **Frequency**: How often the term appears in the source file
- **Source weight**: Where the term was found (filename > heading > body)
- **IDF**: From the search index—terms rare in the knowledge base score higher

Terms not in the index are filtered out. This ensures the query only contains terms that can
match documents.

### 3. Query Construction

The top N terms by score are combined into a boosted OR query:

```
kubernetes^12.5 OR orchestration^8.3 OR container^5.1 OR deployment^4.2
```

Each term's boost reflects its TF-IDF score.


## Source Weights

| Source | Weight | Rationale |
|--------|--------|-----------|
| Filename (sans extension) | 4.0 | Intentional human naming |
| Directory names | 3.0 | Organizational structure |
| Markdown h1 headers | 3.0 | Primary topic markers |
| Markdown h2-h3 headers | 2.0 | Secondary topics |
| Markdown h4-h6 headers | 1.5 | Minor topics |
| Body text | 1.0 | General content |


## Stopword Filtering

Two categories of terms are filtered:

**English stopwords**: Articles, prepositions, conjunctions, common verbs from the
`stop-words` crate.

**Rust stopwords**: Keywords (`fn`, `let`, `impl`, `struct`, `async`, etc.), primitive types
(`i32`, `bool`, `str`), and common standard library types (`Option`, `Result`, `Vec`).


## CLI Usage

### Basic

```bash
# Context for a single file
ra context chapter1.md

# Multiple files
ra context chapter1.md chapter2.md

# Limit results
ra context -n 20 chapter1.md

# Limit to specific trees
ra context -t docs chapter1.md
```

### Explain Mode

Show extracted terms and generated query without executing search:

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

### Flags

| Flag | Description |
|------|-------------|
| `--explain` | Show terms and query without searching |
| `-n, --limit N` | Maximum results (default: 10) |
| `--terms N` | Maximum terms in query (default: 15) |
| `-t, --tree NAME` | Limit to specific tree(s) |
| `--list` | Output titles only |
| `--json` | JSON output |
| `-v, --verbose` | Increase verbosity |


## Output Modes

- **Default**: Full content with highlighting
- **`--list`**: Titles and snippets only
- **`--json`**: Structured JSON output


## Tree-Aware IDF

When `--tree` is specified, IDF lookups consider only documents in the selected trees. This
tunes the query to the relevant subset of the knowledge base.


## Parsers

ra uses specialized parsers for different file types:

| Extensions | Parser |
|------------|--------|
| `.md`, `.markdown` | Markdown (heading-aware) |
| (other) | Plain text (uniform weighting) |

The markdown parser extracts structural elements and assigns weights by heading level. The
plain text parser tokenizes content uniformly.


## Configuration

Context analysis settings live in `.ra.toml`:

```toml
[context]
limit = 10              # Max results
min_term_frequency = 2  # Skip rare terms
min_word_length = 4     # Skip short tokens
max_word_length = 30    # Skip long tokens
sample_size = 50000     # Bytes read from large files
```

### Context Patterns

Patterns associate file globs with hint terms:

```toml
[context.patterns]
"*.rs" = ["rust"]
"src/api/**" = ["http", "api"]
```

These patterns are surfaced by `ra inspect ctx` but are not yet incorporated into generated
queries.
