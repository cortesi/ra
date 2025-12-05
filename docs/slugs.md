# Slug Generation

Slugs are URL-compatible identifiers derived from heading text. They provide
stable, human-readable fragment identifiers for linking to specific sections
within documents.


## Chunk Identifiers

Chunk IDs combine tree name, file path, and optional slug:

| Node Type | ID Format | Example |
|-----------|-----------|---------|
| Document | `{tree}:{path}` | `docs:guides/auth.md` |
| Heading | `{tree}:{path}#{slug}` | `docs:guides/auth.md#oauth-setup` |

Document nodes have no slug. Heading nodes always have a slug derived from the
heading text.


## Algorithm

The slug generation algorithm follows GitHub's conventions for heading anchors:

1. **Lowercase**: Convert the heading text to lowercase.

2. **Filter characters**: Keep only:
   - Alphanumeric characters (a-z, 0-9)
   - Hyphens (`-`)
   - Spaces (converted to hyphens in step 3)
   - Underscores (`_`)

3. **Replace spaces**: Convert spaces to hyphens.

4. **Collapse hyphens**: Replace consecutive hyphens with a single hyphen.

5. **Trim hyphens**: Remove leading and trailing hyphens.

6. **Empty fallback**: If the result is empty (e.g., heading was all punctuation
   or non-ASCII), use `"heading"` as the slug.

7. **Deduplicate**: If this slug already exists in the document, append `-N`
   where N starts at 1 and increments for each duplicate.


## Examples

| Heading Text | Slug |
|--------------|------|
| `Getting Started` | `getting-started` |
| `OAuth 2.0 Setup` | `oauth-20-setup` |
| `Error Handling` | `error-handling` |
| `FAQ` | `faq` |
| `C++ Integration` | `c-integration` |
| `日本語` | `heading` (non-ASCII filtered) |
| `???` | `heading` (all punctuation filtered) |


## Deduplication

When multiple headings produce the same slug, suffixes are appended:

```markdown
# Introduction        → introduction
# Setup               → setup
# Introduction        → introduction-1
# Introduction        → introduction-2
```

Suffixes start at 1 and increment for each subsequent duplicate. The first
occurrence keeps the base slug without a suffix.


## Implementation

The `Slugifier` struct in `ra-document` tracks used slugs within a document:

```rust
let mut slugifier = Slugifier::new();

slugifier.slugify("Getting Started")  // → "getting-started"
slugifier.slugify("FAQ")              // → "faq"
slugifier.slugify("Getting Started")  // → "getting-started-1"
```

Each document uses its own `Slugifier` instance, so slug deduplication is
scoped to individual documents.


## Compatibility

The algorithm produces slugs compatible with:

- GitHub markdown heading anchors
- Standard URL fragment identifiers
- HTML `id` attributes

This ensures chunk IDs can be used directly in URLs and cross-references.
