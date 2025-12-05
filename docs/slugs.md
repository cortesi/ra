# Slug Generation

Slugs are URL-compatible identifiers derived from heading text. They provide stable,
human-readable fragment identifiers for chunk IDs.


## Chunk Identifiers

| Node Type | ID Format | Example |
|-----------|-----------|---------|
| Document | `{tree}:{path}` | `docs:guides/auth.md` |
| Heading | `{tree}:{path}#{slug}` | `docs:guides/auth.md#oauth-setup` |

Document nodes have no slug. Heading nodes always have a slug derived from their text.


## Algorithm

The slug generation algorithm follows GitHub's conventions for heading anchors:

1. **Lowercase**: Convert heading text to lowercase
2. **Filter**: Keep only alphanumerics, hyphens, spaces, and underscores
3. **Spaces to hyphens**: Convert spaces to hyphens
4. **Collapse hyphens**: Replace consecutive hyphens with a single hyphen
5. **Trim**: Remove leading and trailing hyphens
6. **Empty fallback**: If result is empty, use `"heading"`
7. **Deduplicate**: If slug exists in document, append `-N` (starting at 1)


## Examples

| Heading Text | Slug |
|--------------|------|
| `Getting Started` | `getting-started` |
| `OAuth 2.0 Setup` | `oauth-20-setup` |
| `Error Handling` | `error-handling` |
| `FAQ` | `faq` |
| `C++ Integration` | `c-integration` |
| `日本語` | `heading` |
| `???` | `heading` |


## Deduplication

When multiple headings produce the same slug, suffixes are appended:

```markdown
# Introduction        → introduction
# Setup               → setup
# Introduction        → introduction-1
# Introduction        → introduction-2
```

The first occurrence keeps the base slug. Subsequent duplicates get `-1`, `-2`, etc.


## Compatibility

Slugs are compatible with:

- GitHub markdown heading anchors
- Standard URL fragment identifiers
- HTML `id` attributes

Chunk IDs can be used directly in URLs and cross-references.
