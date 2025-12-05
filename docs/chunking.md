# Chunking

This document specifies how ra transforms documents into searchable chunks and how search
results are processed through elbow cutoff and hierarchical aggregation.


## Overview

ra builds a hierarchical chunk tree from each markdown document. The document itself is the
root, and each heading creates a nested node. This structure lets search return results at
the appropriate granularity—from a single subsection to an entire document.


## Terminology

| Term | Definition |
|------|------------|
| Document | A single source file in a tree |
| Tree | A named collection of documents sharing a root directory |
| Heading | A markdown heading `#`–`######` (h1–h6) |
| Node | A structural element in the chunk tree (document or heading) |
| Leaf | A node with no children |
| Chunk | An indexed node; all nodes become chunks |
| Span | A byte range `[start, end)` into the original document content |


## Chunk Tree Construction

### 1. Parse Headings

Identify all headings (h1–h6) in document order. Record each heading's:

- `level`: h1–h6
- `text`: normalized heading text (including inline code spans)
- `heading_start` / `heading_end`: byte offsets of the heading line itself

Everything before the first heading is the **preamble**.

### 2. Create Nodes

**Document node** (the root):

- `depth = 0`
- `title`: First available from: YAML frontmatter `title`, first h1 heading, or filename
  without extension
- `slug = None`
- `span = [0, len(content))`

**Heading nodes** (one per heading):

- `depth = heading_level` (h1 → 1, h2 → 2, ..., h6 → 6)
- `title = heading text`
- `slug`: computed from heading text (see [slugs.md](slugs.md))
- Span assigned in the next step

### 3. Assign Heading Spans

For each heading node:

- Span starts at the first byte after the heading line's terminating newline
- Span ends at the byte before the next heading of **equal or lower** depth, or end of file

Heading lines themselves are excluded from spans—their content is captured in `title`.

Headings with empty spans (no content before the next heading) are discarded.

### 4. Establish Hierarchy

Nodes form a tree:

- The document node is the root
- Each heading attaches to the nearest preceding heading of strictly lower depth
- If no lower-depth heading exists, the heading attaches to the document node
- Children are ordered by document order


## Body Text

A node's **body** is derived from its span at read time.

### Derivation

- Body = text within the span **not** covered by any child span
- For heading nodes: content after the heading line, excluding child spans
- For document nodes: the preamble (content before the first heading), including frontmatter
- If the document has no headings, body = entire file

Each byte belongs to at most one node's body.

### Empty Bodies

A body is empty if it contains only whitespace. All nodes are indexed regardless of body
content—this ensures titles remain searchable even when a section contains only subsections.

### Size Handling

ra does not split large sections. Every node becomes exactly one chunk regardless of size.
Document structure should provide sufficient granularity. Use `ra status` to see warnings
about oversized chunks.


## Plain Text and Edge Cases

- **Plain text files** (`.txt`): Produce a single document chunk; body = entire file; title =
  filename without extension
- **Empty files**: Produce a document node with empty body; title/path remain searchable


## Chunk Metadata

Every indexed chunk includes:

| Field | Description |
|-------|-------------|
| `id` | `{tree}:{path}` (document) or `{tree}:{path}#{slug}` (heading) |
| `doc_id` | Document identifier, same for all chunks in a file |
| `parent_id` | Parent node's `id`; `None` for document nodes |
| `depth` | 0 for document, 1 for h1, ..., 6 for h6 |
| `position` | Document order index (pre-order traversal, starting at 0) |
| `title` | Heading text or document title |
| `slug` | Fragment identifier for headings; `None` for documents |
| `byte_start` / `byte_end` | Span in source file |
| `sibling_count` | Number of siblings under the same parent (for aggregation) |


## Breadcrumbs

Each chunk stores a breadcrumb showing its position in the hierarchy:

```
> Document Title › Parent Section › Chunk Title
```

Rules:

- Always begins with document title
- Intermediate ancestors in order from shallowest to deepest
- Node's own title appears last (omitted for document chunks)
- If first heading equals document title, omit it to avoid duplication

Breadcrumbs are stored for display but not indexed.


## Search Result Processing

After retrieving candidates from the index, ra applies two post-processing phases.

### Phase 2: Elbow Cutoff

BM25 scores follow a long-tail distribution. Rather than using arbitrary thresholds, ra
detects the natural "elbow" where relevant results end.

**Algorithm:**

1. If fewer than 2 candidates, return all
2. Compute score ratio between each adjacent pair: `ratio[i] = score[i+1] / score[i]`
3. Find first index where `ratio < cutoff_ratio`
4. Return candidates up to and including that index

**Edge cases:**

- All ratios ≥ `cutoff_ratio`: return up to `max_results`
- First ratio < threshold: return only the first result
- Zero or negative scores: immediate cutoff

**Example with `cutoff_ratio = 0.5`:**

```
Scores:  [8.0, 7.5, 7.0, 3.2, 3.0, 2.8, 0.9]
Ratios:  [0.94, 0.93, 0.46, ...]
                      ↑
                      First ratio < 0.5

Result: first 3 candidates (8.0, 7.5, 7.0)
```

### Phase 3: Hierarchical Aggregation

When multiple siblings match a query, ra merges them into their parent. This provides
unified context instead of fragmented results.

**Algorithm:**

```
for current_depth from max_depth down to 1:
    for each parent with matching children at current_depth:
        match_fraction = matching_children / total_children

        if match_fraction >= aggregation_threshold:
            replace children with aggregated parent result

filter out any result whose ancestor also appears in results
sort by score descending
```

**Key behaviors:**

- Works bottom-up, deepest matches first
- When siblings aggregate into a parent, that parent may itself aggregate with its siblings
- After aggregation, descendants of included results are filtered out
- Parent's score = maximum of constituent scores

**Example with `aggregation_threshold = 0.5`:**

```
Document
├── Section A (matches)
│   ├── Subsection A1 (matches)  }
│   └── Subsection A2 (matches)  } → 2/2 = 100%, aggregate to Section A
└── Section B
    └── Subsection B1 (matches)    (1/1 = 100%, keep as-is)
```

### Aggregated Result Structure

Aggregated results include:

- `id`: Parent chunk's identifier
- `score`: Maximum score among constituents
- `constituents`: List of original matches (for UI expansion)


## Parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| `candidate_limit` | 100 | Maximum candidates from index before cutoff |
| `cutoff_ratio` | 0.5 | Score ratio threshold for elbow detection |
| `max_results` | 20 | Fallback limit when no elbow detected |
| `aggregation_threshold` | 0.5 | Sibling fraction required to trigger aggregation |


## Complete Search Flow

```
1. Query Tantivy index (limit = candidate_limit)
   → Candidates ranked by BM25

2. Elbow cutoff (cutoff_ratio)
   → Relevant matches only

3. Hierarchical aggregation (aggregation_threshold)
   → Individual chunks + aggregated parents

4. Filter descendants of included ancestors

5. Sort by score descending
   → Final results
```
