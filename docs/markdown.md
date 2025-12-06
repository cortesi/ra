# Writing Markdown for ra

This guide explains how to structure markdown documents for optimal indexing and search.

**Searchable fields (with boost):**

- title (10x) - heading text or document title
- path (8x) - file path within tree (tokenized on `/` and `.`)
- tags (5x) - frontmatter tags
- body (1x) - chunk content

**Not indexed:** code block language identifiers, HTML comments, link URLs (link text is indexed),
image alt text.

## Document Titles

ra determines a document's title using this priority:

1. **Frontmatter `title`** field (highest priority)
2. **First h1 heading** in the document
3. **Filename** without extension (fallback)

The title is heavily weighted in search (10x boost), so ensure it's descriptive.

```markdown
---
title: Error Handling in Rust
---
```

If no frontmatter title exists, the first `# Heading` becomes the title. If there are no h1
headings, the filename is used (e.g., `error-handling.md` becomes "error-handling").

## Frontmatter

Optional YAML metadata at the start of a file:

```markdown
---
title: My Document
tags: [rust, async, tutorial]
---
```

**Supported fields:**

- `title` - Document title (overrides h1)
- `tags` - List of tags for categorization (5x search boost)

Tags apply to all chunks in the document. Other frontmatter fields are ignored.

## Document Structure

ra chunks documents by heading hierarchy. Each heading creates a searchable chunk containing:

- The heading text (becomes the chunk's title, 10x boost)
- Content from the heading to the next heading of equal or higher level
- Child headings as nested chunks

```markdown
# Introduction           <-- chunk: depth 1

Intro content.

## Getting Started       <-- chunk: depth 2, child of Introduction

Setup steps.

## Configuration         <-- chunk: depth 2, child of Introduction

Config details.

# Advanced Topics        <-- chunk: depth 1

Advanced content.
```

**Preamble:** Content before the first heading becomes the document-level chunk (depth 0).

## Best Practices

**Use descriptive headings.** Headings are titles of searchable chunks. "Setup" is less useful than
"Installing the CLI Tool".

**Choose meaningful filenames.** The path is searchable with 8x boost. `rust-error-handling.md`
finds matches better than `doc1.md`.

**Add tags for cross-cutting concerns.** Tags let you categorize documents that share themes but
live in different locations.

**Write content after headings.** Consecutive headings with no content between them at the same
level cause earlier headings to be discarded (empty span).

```markdown
# Bad: Empty Section
# Another Section      <-- "Bad: Empty Section" is discarded

Content here.
```

```markdown
# Good: Has Content

Some introductory text.

# Another Section

More content.
```

**Structure depth logically.** h1 > h2 > h3 creates a navigable hierarchy. Skipping levels (h1 >
h3) works but may confuse readers.
