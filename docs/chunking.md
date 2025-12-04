# Chunking Specification

This document defines the canonical behaviour for document chunking in ra. It
refines the high-level description in `docs/search.md` and is the source of
truth for how markdown and text files are transformed into chunks suitable for
indexing and search.


## Goals

- Represent the full heading hierarchy of each document.
- Produce leaf chunks that are small enough for high recall but large enough to
  be meaningful in isolation.
- Keep chunk identifiers stable across edits that do not materially change
  structure.
- Support efficient, hierarchy-aware merging at search time without storing
  duplicated text.


## Terminology

- **Document**: A single source file in a tree.
- **Tree**: A named collection of documents sharing a common root directory.
  Tree identifiers are assigned by the index configuration.
- **Heading**: A markdown heading `#`–`######` (h1–h6).
- **Node**: A structural element in the chunk tree (document node or heading
  node).
- **Leaf**: A node with no children.
- **Chunk**: A node emitted to the index. A node becomes a chunk if it has
  non-empty body text (more than just whitespace).
- **Span**: A half-open byte range `[byte_start, byte_end)` into the original
  document content, representing the full extent of a node.


## Parameters

The following parameters control merging behaviour at search time:

| Parameter | Default | Description |
|-----------|---------|-------------|
| `aggregation_threshold` | 0.5 | Fraction of a parent's children that must match before sibling aggregation is triggered. |
| `min_aggregation_matches` | 2 | Minimum number of matching siblings required for aggregation, regardless of threshold. |
| `score_cap_multiplier` | 2.0 | When aggregating, cap the parent's score at this multiple of the highest child score. |


## Chunk Tree Construction

This section describes how markdown documents with headings are turned into a
hierarchical chunk tree.

1. **Parse headings**
   - Use the markdown parser to identify all headings (h1–h6) in order.
   - Record for each heading:
     - `level`: heading level (h1–h6),
     - `text`: normalized heading text (including inline code spans),
     - `heading_start` / `heading_end`: byte offsets covering the heading line
       itself.
   - Everything before the first heading is the **preamble**.

2. **Create nodes**
   - Create a **document node** (the root):
     - `depth = 0`
     - `title = document title` (from frontmatter, first h1, or filename)
     - `slug = None`
     - `span = [0, len(content))`
   - For each heading, create a heading node with:
     - `depth = heading_level` (h1 → 1, …, h6 → 6),
     - `title = heading text`,
     - `slug = computed_slug` (see Slug Generation below),
     - a span assigned in the next step.

3. **Assign heading spans**
   - For each heading node, its span runs from the byte after the heading line
     ends to the byte before the next heading of **equal or lower** depth (or
     end of document).
   - Heading lines themselves are deliberately excluded from spans; their
     content is captured in the node's `title` field.
   - Headings whose computed span would be empty (no content between this
     heading and the next) are discarded.

4. **Establish hierarchy**
   - Nodes form a tree:
     - The document node is the root.
     - Heading nodes are attached to the nearest preceding heading of strictly
       lower depth; if none exists, they are attached to the document node.
   - Children of a node are ordered by document order.


## Body Text

A node's **body** is derived from its span at read time—it is not stored
separately.

1. **Body derivation**
   - A node's body is the text within its span that is **not** covered by any
     child node's span.
   - For heading nodes: the body is content after the heading line, excluding
     child spans.
   - For the document node: the body is the preamble (content before the first
     heading), including any frontmatter. If the document has no headings, the
     body is the entire file.
   - Each byte of document text belongs to at most one node's body.

2. **Empty body**
   - A body is considered empty if it contains only whitespace.
   - Nodes with empty bodies are still part of the tree structure but are not
     emitted as chunks to the index.

3. **Leaves**
   - A **leaf** is any node with no children.
   - Leaves always have non-empty bodies (the content under that heading or the
     entire document for plain text files).

4. **Chunk sizing**
   - Chunking never drops or alters nodes based on size: every node with a
     non-empty body becomes a chunk, regardless of how small or large it is.
   - Large nodes (e.g. long code blocks or extensive text without headings) are
     **not** split. It is the user's responsibility to structure documents with
     sufficient granularity to avoid oversized chunks.

5. **Plain text documents**
   - Files without headings (e.g. `.txt`) produce a single document node whose
     body is the entire file content.
   - The chunk uses the filename (without extension) as its title.


## Metadata and Identifiers

Every node in the chunk tree has the following structural metadata:

- `id`: Unique chunk identifier, constructed from tree, path, and optional slug.
  - Document node: `{tree}:{path}` (slug is `None`)
  - Heading nodes: `{tree}:{path}#{slug}`
- `doc_id`: Document identifier, the same for all nodes in a file.
  - `doc_id = "{tree}:{path}"`
- `parent_id`: Parent node's ID (`None` for the document node).
- `depth`: Hierarchy depth.
  - `0` for document node,
  - `1` for h1, `2` for h2, … `6` for h6.
- `position`: Document order index (starting at 0), assigned in a pre-order
  traversal of the tree.
- `title`: Heading text or document title.
- `slug`: Optional; used in fragment identifiers for heading nodes.
- `byte_start` / `byte_end`: The node's span in bytes into the original
  document. Body text is derived by reading this span and excluding child spans.

### Slug Generation

Slugs provide stable, human-readable fragment identifiers for heading nodes.
The document node has no slug (`None`).

For heading nodes, compute the slug from the heading text:

1. Convert to lowercase.
2. Keep alphanumeric characters and underscores.
3. Convert spaces and hyphens to single hyphens.
4. Remove other punctuation and non-ASCII characters.
5. Collapse consecutive hyphens and trim leading/trailing hyphens.
6. If the result is empty, use `"heading"` as a fallback.
7. Deduplicate repeated slugs within the document by appending `-N` (N starts
   at 1).

### Reconstructing Parent Content

Parent nodes can be reconstructed in full by reading the original file from
`byte_start` to `byte_end`. This includes all descendant content. For just the
parent's own body, exclude bytes covered by child spans.


## Breadcrumbs

Each chunk stores a breadcrumb string representing its position in the
hierarchy:

```text
> Document Title › Parent Section › Chunk Title
```

Rules:

- The breadcrumb always begins with the document title.
- Intermediate ancestors are included in order from shallowest to deepest.
- The node's own `title` appears last (omitted for document-level chunks).
- If the first heading in the document is identical to the document title,
  omit it from the breadcrumb to avoid duplication.

Breadcrumbs are stored as text and prepended to `body` when presented to
agents, but breadcrumbs themselves are not indexed or used for span
calculation.


## Emission to the Index

The chunk tree is a conceptual structure; the search index operates on
**chunks**:

- Every node with a non-empty body becomes one indexed chunk with:
  - `id`, `doc_id`, `title`, `body`, `breadcrumb`, `depth`, `position`,
    `byte_start`, `byte_end`, and path/tree metadata.
- Leaves are always chunks; non-leaf parents are also chunks if they have
  direct body text (e.g. a document with preamble content, or an h1 followed
  by introductory paragraphs before an h2).


## Hierarchical Merging (Search-Time)

### Motivation

The chunk tree stores content exactly once, at the most granular level
(leaves). However, search results need to be returned at the appropriate level
of granularity for the query. When a user searches for "authentication", they
may want:

- A specific paragraph mentioning it (a leaf match),
- An entire section about authentication (aggregated siblings),
- A document titled "Authentication Guide" (a parent match).

Rather than duplicating content at every hierarchy level—which would bloat the
index and create scoring ambiguity—we store leaves and reconstruct parent
context at search time through hierarchical merging.

### Merge Rules

When multiple chunks from the same document match a query, the merge layer
promotes results to a common ancestor. Rules are applied bottom-up after
initial scoring, in the following priority order:

1. **Parent match priority**: If a parent node matches the query in its own
   right (via title or body text), return the parent rather than individual
   children. The parent's score is the maximum of its own match score and the
   combined child score (see rule 3).

2. **Sibling aggregation**: If the fraction of matching siblings under a parent
   exceeds `aggregation_threshold` **and** the number of matches is at least
   `min_aggregation_matches`, replace them with a single result for the parent
   node.

3. **Score combination**: When aggregating children into a parent (via rule 1
   or 2), the combined score is the sum of child scores, capped at
   `score_cap_multiplier` times the highest individual child score.

4. **Depth limit**: Aggregation stops at depth 1 (h1 sections). Document-level
   results (depth 0) are only returned when:
   - The document node itself matches (has matching preamble content or title),
     or
   - All top-level children (depth 1) would be aggregated.

5. **Standalone leaves**: If a leaf matches but its parent does not (and
   sibling aggregation threshold is not met), the leaf is returned as a
   standalone result regardless of size.

This design keeps content non-duplicated across chunks while preserving the
full hierarchy needed for effective search and flexible result presentation.
