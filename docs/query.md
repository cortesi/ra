# Query Syntax

ra provides a powerful query language for searching your knowledge base. This
document covers everything from basic searches to advanced query construction.


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

When using `ra search` from the command line, multiple arguments are joined
with OR:

```bash
ra search foo bar
# Equivalent to: (foo) OR (bar)

ra search "error handling" "exception handling"
# Equivalent to: ("error handling") OR ("exception handling")
```

This makes it easy to search for multiple topics at once. Each argument is
wrapped in parentheses before joining, so complex expressions work correctly:

```bash
ra search "rust async" "golang goroutine"
# Equivalent to: (rust async) OR (golang goroutine)
# Which means: (rust AND async) OR (golang AND goroutine)
```

To search for terms that must ALL appear, put them in a single quoted argument:

```bash
ra search "rust async"        # rust AND async (single argument)
ra search rust async          # rust OR async (two arguments)
```


## Basic Queries

### Single Terms

The simplest query is a single word:

```
rust
```

This finds all chunks containing "rust" (case-insensitive, with stemming).

### Multiple Terms (AND)

Within a single query string, space-separated terms are combined with AND. All
must match:

```
rust async
```

Finds chunks containing both "rust" AND "async".

### Phrases

Quote multiple words for exact phrase matching:

```
"error handling"
```

Finds chunks containing the exact phrase "error handling" (words must appear
adjacent and in order).


## Boolean Operators

### OR

Use `OR` (case-insensitive) to match either term:

```
rust OR golang
```

Finds chunks containing "rust" OR "golang" (or both).

Multiple ORs chain together:

```
rust OR golang OR python
```

### Negation

Prefix a term with `-` to exclude it:

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

Use parentheses to control precedence:

```
(rust OR golang) async
```

Finds chunks containing ("rust" OR "golang") AND "async".

Without parentheses, `rust OR golang async` would parse as
`rust OR (golang AND async)` because AND has higher precedence than OR.

Complex groupings are supported:

```
(rust async) OR (golang goroutine)
```


## Operator Precedence

From highest to lowest:

1. **Grouping**: `(...)`
2. **Field prefix**: `field:`
3. **Negation**: `-`
4. **AND** (implicit, between adjacent terms)
5. **OR** (explicit keyword)

Examples showing how precedence affects parsing:

| Query | Parsed As |
|-------|-----------|
| `a b OR c` | `(a AND b) OR c` |
| `a OR b c` | `a OR (b AND c)` |
| `-a b` | `(-a) AND b` |
| `a OR b OR c` | `a OR b OR c` (flat) |


## Field-Specific Queries

Search within specific fields using the `field:` prefix.

### Available Fields

| Field | Description | Boost |
|-------|-------------|-------|
| `title` | Chunk or document title | 3.0x |
| `tags` | Frontmatter tags | 2.5x |
| `path` | File path within tree | 2.0x |
| `body` | Chunk content | 1.0x |
| `tree` | Tree name (exact match) | n/a |

### Field Query Syntax

```
title:guide                    # term in title
title:"getting started"        # phrase in title
title:(rust OR golang)         # OR in title
```

Field queries can be combined with other terms:

```
title:guide rust               # "guide" in title AND "rust" anywhere
title:api tags:reference       # "api" in title AND "reference" in tags
```

### Tree Filtering

The `tree` field filters results to a specific knowledge tree:

```
tree:docs authentication       # search only in "docs" tree
tree:notes                     # all results from "notes" tree
```

### Negating Fields

Negate field queries to exclude matches:

```
-title:deprecated              # title must NOT contain "deprecated"
-tags:draft                    # exclude drafts
```


## Boosting

Boost terms to increase their importance in ranking. Higher-boosted terms
contribute more to the relevance score.

### Syntax

Append `^N` where N is a decimal number:

```
rust^2.5                       # "rust" is 2.5x more important
"error handling"^3.0           # boost phrase
(async await)^2.0              # boost group
title:guide^2.5                # boost field query
```

### Use Cases

Prioritize certain terms:

```
kubernetes^2.0 docker          # prefer kubernetes matches
```

Emphasize title matches:

```
title:authentication^3.0 authentication
```

Weight alternatives:

```
rust^2.0 OR golang^1.5 OR python^1.0
```


## Examples

### Common Patterns

Find documentation about a topic:

```
"error handling" rust
```

Find guides (title search):

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

Complex research query:

```
title:(guide OR tutorial) (rust OR golang) -deprecated
```

### Real-World Queries

Find async patterns in Rust:

```
rust async "error handling" -deprecated
```

API documentation:

```
tree:docs path:api title:(reference OR guide)
```

Search for alternatives:

```
(authentication OR auth) (jwt OR session OR oauth)
```

Boosted multi-topic search:

```
kubernetes^2.0 deployment^1.5 yaml
```


## Shell Escaping

When using ra from the command line, shell metacharacters need escaping.

### Quotes

```bash
# Protect inner quotes with outer quotes
ra search '"error handling"'
ra search "\"error handling\""
```

### Parentheses

```bash
# Quote the whole query
ra search '(rust OR golang) async'
ra search "(rust OR golang) async"
```

### Safe Characters

These do not need escaping in most shells:

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

This helps verify that operator precedence produces the intended structure.


## Error Messages

ra provides helpful error messages for invalid queries:

```
$ ra search 'title:'
Error: expected term, phrase, or group after 'title:'

$ ra search '(rust async'
Error: expected closing parenthesis

$ ra search '"unclosed phrase'
Error: unclosed quote

$ ra search 'OR rust'
Error: unexpected OR (needs expression before it)

$ ra search '^2.5 rust'
Error: unexpected boost (needs expression before it)
```


## Text Processing

Queries undergo the same text analysis as indexed content:

1. **Tokenization**: Split on whitespace and punctuation
2. **Lowercasing**: Case-insensitive matching
3. **Stemming**: "handling" matches "handled", "handles"

This means:

- `Rust` matches `rust`, `RUST`
- `handling` matches documents containing `handled`
- `error-handling` is tokenized as `error` and `handling`


## Tips for Effective Searching

1. **Start broad, then narrow**: Begin with key terms, add constraints if too
   many results.

2. **Use phrases for precision**: `"error handling"` is more precise than
   `error handling`.

3. **Leverage field searches**: `title:guide` finds guides faster than hoping
   "guide" appears in the body.

4. **Exclude noise**: `-deprecated -draft -wip` removes common false positives.

5. **Try synonyms with OR**: `(error OR exception) handling` catches both
   conventions.

6. **Check with --explain**: Verify complex queries parse as intended before
   relying on results.
