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
  large amounts of duplicated text.


## Terminology

- **Document**: A single source file in a tree.
- **Heading**: A markdown heading `#`–`######` (h1–h6).
- **Node**: A structural element in the chunk tree (document node or heading
  node).
- **Leaf**: A node with no children that carries `body` text.
- **Chunk**: A node with a non-empty `body` that is emitted to the index.
- **Span**: A half-open byte range `[byte_start, byte_end)` into the original
  document content.


## Parameters

The following parameters control chunking and merging behaviour:

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
     - `start` / `end`: byte offsets covering the heading line itself.
   - Everything before the first heading is the **preamble**.

2. **Create nodes**
   - Create a **document node** (the root):
     - `depth = 0`
     - `title = document title` (from frontmatter, first h1, or filename)
     - `slug = ""`
     - `span = [0, len(content))`
   - For each heading, create a heading node with:
     - `depth = heading_level` (h1 → 1, …, h6 → 6),
     - `title = heading text`,
     - a span assigned in the next step.

3. **Assign heading spans**
   - For each heading node, its span runs from the byte after the heading line
     ends to the byte before the next heading of **equal or lower** depth (or
     end of document).
   - Headings whose computed span would be empty (no content between this
     heading and the next) are discarded.

4. **Establish hierarchy**
   - Nodes form a tree:
     - The document node is the root.
     - Heading nodes are attached to the nearest preceding heading of strictly
       lower depth; if none exists, they are attached to the document node.
   - Children of a node are ordered by document order.


## Node Bodies

Once the tree is constructed, we assign `body` text to nodes.

1. **Body definition**
   - A node's body is the parts of its span **not** covered by any child
     node's span.
   - For heading nodes: the body starts immediately after the heading line and
     excludes any child spans.
   - For the document node: the body is the preamble (content before the first
     heading), including any frontmatter. If the document has no headings, the
     body is the entire file.
   - Body text is never duplicated: each byte of document text belongs to at
     most one node body.
   - Heading lines themselves are exposed via `title`, not included in `body`.

2. **Leaves**
   - A **leaf** is any node with no children.
   - Leaves always have a body (the content under that heading or the entire
     document for plain text files).

3. **Chunk Sizing**
   - Chunking never drops or alters nodes based on size: every node with a
     non-empty `body` becomes a chunk, regardless of how small or large it is.
   - Large nodes (e.g. long code blocks or extensive text without headings) are
     **not** split. It is the user's responsibility to structure documents with
     sufficient granularity to avoid oversized chunks.

4. **Plain text documents**
   - Files without headings (e.g. `.txt`) produce a single document node whose
     body is the entire file content.
   - The chunk:
     - Uses the filename (without extension) as its title,
     - Has no fragment in its ID: `id = "{tree}:{path}"`.


## Metadata and Identifiers

Every node in the chunk tree has the following structural metadata:

- `id`: Unique chunk identifier.
  - Document node (when it has body): `{tree}:{path}` (no fragment)
  - Heading nodes: `{tree}:{path}#{slug}`
- `doc_id`: Document identifier (same for all nodes in a file).
  - `doc_id = "{tree}:{path}"`
- `parent_id`: Parent node ID (`None` for the document node).
- `depth`: Hierarchy depth.
  - `0` for document node,
  - `1` for h1, `2` for h2, … `6` for h6.
- `position`: Document order index (starting at 0), assigned in a pre-order
  traversal of the tree.
- `title`: Heading text or document title.
- `slug`: GitHub-compatible slug used in IDs:
  - Lowercase,
  - Keep alphanumeric characters and underscores,
  - Convert spaces and hyphens to single hyphens,
  - Remove other punctuation and non-ASCII characters,
  - Collapse consecutive hyphens and trim leading/trailing hyphens,
  - Fall back to `"heading"` if empty,
  - Deduplicate repeated slugs within the document by appending `-N` (N starts
    at 1).
- `byte_start` / `byte_end`: Body span in bytes into the original document.
- `body` (optional): Text content of this node (empty for non-leaf parents
  without direct content).

Parent nodes can be reconstructed in full by:

- Concatenating their own `body` (if any) with the bodies of descendant nodes
  in document order, or
- Reading the original file from the first child's heading start to the last
  descendant's byte_end.


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

- Every node with a non-empty `body` becomes one indexed chunk with:
  - `id`, `doc_id`, `title`, `body`, `breadcrumb`, `depth`, `position`,
    `byte_start`, `byte_end`, and path/tree metadata.
- Leaves are always chunks; non-leaf parents are also chunks if they have
  direct body text (e.g. a document with preamble content, or an h1 followed
  by introductory paragraphs before an h2).

### Hierarchical Merging (Search-Time)

When multiple chunks from the same document match a query, the merge layer
promotes results to a common ancestor using these rules (applied bottom-up
after initial scoring):

1. **Sibling aggregation**: If the fraction of matching siblings under a parent
   exceeds `aggregation_threshold` **and** the number of matches is at least
   `min_aggregation_matches`, replace them with a single result for the parent
   node.

2. **Parent match priority**: If a parent node matches the query in its own
   right (via title or body), return the parent rather than individual
   children.

3. **Score combination**: When aggregating children into a parent, the
   parent's score is the sum of child scores, capped at `score_cap_multiplier`
   times the highest individual child score.

4. **Depth limit**: Aggregation stops at depth 1 (h1 sections). Document-level
   results (depth 0) are only returned when:
   - The document node itself matches (has matching preamble content or title),
     or
   - All top-level children (depth 1) would be aggregated.

5. **Standalone leaves**: If a leaf matches but its parent does not (and
   sibling aggregation threshold is not met), the leaf is returned as a
   standalone result regardless of size.

This design keeps content non-duplicated across chunks while preserving the
full hierarchy needed for effective search and flexible result merging.
