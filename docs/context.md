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


## Context Rules

Rules customize context search behavior based on file patterns. When `ra context` analyzes a
file, matching rules can:

- **Inject terms** into the search query
- **Limit search** to specific trees
- **Auto-include** specific files in results

### Rule Format

Rules use TOML's array-of-tables syntax under `[[context.rules]]`:

```toml
[[context.rules]]
match = "*.rs"
trees = ["docs"]
terms = ["rust"]
include = ["docs:api/overview.md"]
```

### Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `match` | `String` or `[String]` | Yes | Glob pattern(s) matched against file paths |
| `trees` | `[String]` | No | Limit search to these trees (default: all trees) |
| `terms` | `[String]` | No | Additional search terms to inject into the query |
| `include` | `[String]` | No | Files to always include in results |

### Match Patterns

The `match` field accepts either a single glob or an array of globs:

```toml
# Single pattern
[[context.rules]]
match = "*.rs"
terms = ["rust"]

# Multiple patterns
[[context.rules]]
match = ["*.tsx", "*.jsx"]
terms = ["react", "components"]
```

Patterns are matched against the file path relative to the config file location, using standard
glob syntax (`*`, `**`, `?`, `[...]`).

### Tree-Prefixed Include Paths

The `include` field uses tree-prefixed paths in the format `tree:path`:

```toml
[[context.rules]]
match = "src/api/**"
include = ["docs:api/overview.md", "docs:api/authentication.md"]
```

This explicitly names which tree contains each file, avoiding ambiguity when multiple trees
might contain files with similar paths.

### Rule Merging

When multiple rules match a file, their effects are merged:

- **terms**: All terms from matching rules are concatenated (deduplicated)
- **trees**: Intersection of all specified trees (if any rule specifies trees, only the
  intersection is searched; if no rules specify trees, all trees are searched)
- **include**: All include paths from matching rules are concatenated (deduplicated)

Example with two matching rules:

```toml
[[context.rules]]
match = "*.rs"
trees = ["docs", "examples"]
terms = ["rust"]

[[context.rules]]
match = "src/api/**"
trees = ["docs"]
terms = ["http", "handlers"]
include = ["docs:api/overview.md"]
```

For `src/api/handler.rs`:
- **trees**: `["docs"]` (intersection of `["docs", "examples"]` and `["docs"]`)
- **terms**: `["rust", "http", "handlers"]`
- **include**: `["docs:api/overview.md"]`

### CLI Interaction

The `--tree` flag interacts with rule-based tree filtering:

- If neither CLI nor rules specify trees: search all trees
- If only CLI specifies trees: use CLI trees
- If only rules specify trees: use rule trees
- If both specify trees: use intersection of CLI and rule trees

### Explain Output

Use `--explain` to see which rules matched and their effects:

```bash
$ ra context --explain src/api/handler.rs

File: src/api/handler.rs

Matched rules:
  - *.rs
    terms: ["rust"]
    trees: ["docs", "examples"]
  - src/api/**
    terms: ["http", "handlers"]
    trees: ["docs"]
    include: ["docs:api/overview.md"]

Merged effects:
  terms: ["rust", "http", "handlers"]
  trees: ["docs"]
  include: ["docs:api/overview.md"]

Ranked terms:
  ...
```

### Example Configuration

```toml
[context]
limit = 10
min_word_length = 4

[[context.rules]]
match = "*.rs"
trees = ["docs"]
terms = ["rust"]

[[context.rules]]
match = "*.py"
trees = ["docs"]
terms = ["python"]

[[context.rules]]
match = "src/api/**"
terms = ["http", "routing", "handlers"]
include = ["docs:api/overview.md"]

[[context.rules]]
match = ["*.tsx", "*.jsx"]
terms = ["react", "typescript", "components"]
include = ["docs:frontend/components.md"]

[[context.rules]]
match = "tests/**"
trees = ["docs", "examples"]
terms = ["testing"]
```
