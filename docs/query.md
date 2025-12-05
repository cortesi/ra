# Query Syntax

ra provides a powerful query language for searching your knowledge base.


## Quick Reference

| Syntax | Meaning | Example |
|--------|---------|---------|
| `term` | Must contain term | `rust` |
| `term1 term2` | Must contain both (AND) | `rust async` |
| `"phrase"` | Exact phrase | `"error handling"` |
| `-term` | Must NOT contain | `-deprecated` |
| `a OR b` | Either term | `rust OR golang` |
| `(...)` | Grouping | `(rust OR go) async` |
| `field:term` | Search specific field | `title:guide` |
| `term^N` | Boost importance | `rust^2.5` |


## Command-Line Arguments

When using `ra search`, multiple arguments are joined with OR:

```bash
ra search foo bar
# Equivalent to: (foo) OR (bar)

ra search "error handling" "exception handling"
# Equivalent to: ("error handling") OR ("exception handling")
```

Each argument is wrapped in parentheses before joining:

```bash
ra search "rust async" "golang goroutine"
# Equivalent to: (rust AND async) OR (golang AND goroutine)
```

To require ALL terms, put them in a single quoted argument:

```bash
ra search "rust async"        # rust AND async (single argument)
ra search rust async          # rust OR async (two arguments)
```


## Basic Queries

### Single Terms

```
rust
```

Finds all chunks containing "rust" (case-insensitive, with stemming).

### Multiple Terms (AND)

Space-separated terms within a single query string require all terms to match:

```
rust async
```

Finds chunks containing both "rust" AND "async".

### Phrases

Quote words for exact phrase matching:

```
"error handling"
```

Finds chunks containing "error handling" as an adjacent phrase.


## Boolean Operators

### OR

Use `OR` (case-insensitive) to match either term:

```
rust OR golang
```

Multiple ORs chain together:

```
rust OR golang OR python
```

### Negation

Prefix with `-` to exclude:

```
rust -deprecated
```

Finds chunks containing "rust" but NOT "deprecated".

Negation applies to the immediately following term, phrase, or group:

```
-"legacy code"      # exclude phrase
-(old deprecated)   # exclude chunks matching both
```

### Grouping

Parentheses control precedence:

```
(rust OR golang) async
```

Finds chunks containing ("rust" OR "golang") AND "async".

Without parentheses, `rust OR golang async` parses as `rust OR (golang AND async)` because
AND has higher precedence than OR.


## Operator Precedence

From highest to lowest:

1. **Grouping**: `(...)`
2. **Field prefix**: `field:`
3. **Negation**: `-`
4. **AND** (implicit, between adjacent terms)
5. **OR** (explicit keyword)

| Query | Parsed As |
|-------|-----------|
| `a b OR c` | `(a AND b) OR c` |
| `a OR b c` | `a OR (b AND c)` |
| `-a b` | `(-a) AND b` |


## Field Queries

Search within specific fields using the `field:` prefix.

### Available Fields

| Field | Description | Boost |
|-------|-------------|-------|
| `title` | Chunk or document title | 3.0× |
| `tags` | Frontmatter tags | 2.5× |
| `path` | File path within tree | 2.0× |
| `body` | Chunk content | 1.0× |
| `tree` | Tree name (exact match) | — |

### Syntax

```
title:guide                    # term in title
title:"getting started"        # phrase in title
title:(rust OR golang)         # OR in title
```

Combine with other terms:

```
title:guide rust               # "guide" in title AND "rust" anywhere
title:api tags:reference       # "api" in title AND "reference" in tags
```

### Tree Filtering

```
tree:docs authentication       # search only "docs" tree
tree:notes                     # all results from "notes" tree
```

### Negating Fields

```
-title:deprecated              # title must NOT contain "deprecated"
-tags:draft                    # exclude drafts
```


## Boosting

Boost terms to increase their importance in ranking:

```
rust^2.5                       # "rust" is 2.5× more important
"error handling"^3.0           # boost phrase
(async await)^2.0              # boost group
title:guide^2.5                # boost field query
```

Use cases:

```
kubernetes^2.0 docker          # prefer kubernetes matches
rust^2.0 OR golang^1.5 OR python^1.0   # weight alternatives
```


## Examples

### Common Patterns

Find documentation about a topic:

```
"error handling" rust
```

Find guides:

```
title:guide OR title:tutorial
```

Exclude deprecated content:

```
authentication -deprecated -legacy
```

Search specific tree:

```
tree:docs api endpoints
```

Complex research:

```
title:(guide OR tutorial) (rust OR golang) -deprecated
```


## Shell Escaping

When using ra from the command line, shell metacharacters need escaping.

### Quotes

```bash
ra search '"error handling"'
ra search "\"error handling\""
```

### Parentheses

```bash
ra search '(rust OR golang) async'
ra search "(rust OR golang) async"
```

### Safe Characters

These typically don't need escaping:

- `OR` (no shell meaning)
- `-term` (not at line start)
- `field:term` (colons are safe)
- `^2.5` (usually safe)


## Debugging Queries

Use `--explain` to see how ra parses your query:

```bash
$ ra search --explain 'title:guide (rust OR golang)'

Query AST:
  And
  ├─ Field(title)
  │  └─ Term("guide")
  └─ Or
     ├─ Term("rust")
     └─ Term("golang")
```


## Text Processing

Queries undergo the same analysis as indexed content:

1. **Tokenization**: Split on whitespace and punctuation
2. **Lowercasing**: Case-insensitive matching
3. **Stemming**: "handling" matches "handled", "handles"

This means:

- `Rust` matches `rust`, `RUST`
- `handling` matches documents containing `handled`
- `error-handling` tokenizes as `error` and `handling`


## Tips

1. **Start broad, then narrow**: Begin with key terms, add constraints if needed.
2. **Use phrases for precision**: `"error handling"` is more precise than `error handling`.
3. **Leverage field searches**: `title:guide` finds guides faster than hoping "guide" appears
   in the body.
4. **Exclude noise**: `-deprecated -draft -wip` removes common false positives.
5. **Try synonyms with OR**: `(error OR exception) handling` catches both conventions.
6. **Verify with --explain**: Check that complex queries parse as intended.
