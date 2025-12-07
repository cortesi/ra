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

After retrieving candidates from the index, ra applies elbow cutoff to determine the
"relevant" set, then aggregates all relevant candidates hierarchically.

### Elbow Cutoff

BM25 scores follow a long-tail distribution. Rather than using arbitrary thresholds, ra
detects the natural "elbow" where relevance drops significantly. This happens **before**
aggregation, determining which candidates are worth aggregating.

**Algorithm:**

1. If fewer than 2 candidates, return all
2. Compute score ratio between each adjacent pair: `ratio[i] = score[i+1] / score[i]`
3. Find first index where `ratio < cutoff_ratio`
4. Return candidates up to and including that index

**Edge cases:**

- All ratios ≥ `cutoff_ratio`: return up to `max_candidates`
- First ratio < threshold: return only the first candidate
- Zero or negative scores: immediate cutoff

**Example with `cutoff_ratio = 0.5`:**

```
Scores:  [8.0, 7.5, 7.0, 3.2, 3.0, 2.8, 0.9]
Ratios:  [0.94, 0.93, 0.46, ...]
                      ↑
                      First ratio < 0.5

Result: first 3 candidates pass through to aggregation
```

### Adaptive Hierarchical Aggregation

After elbow cutoff, ra processes **all** relevant candidates in score order, aggregating
siblings when appropriate:

**Algorithm:**

```
results = []

for candidate in relevant_candidates (sorted by score descending):
    if candidate.id already in results:
        skip  # Added via earlier cascade

    if any ancestor of candidate is in results:
        skip  # Ancestor already covers this

    if candidate has descendants in results:
        remove descendants from results
        add candidate as aggregated result with descendants as constituents
        check for cascade (may aggregate with siblings)
        continue

    siblings_in_results = find siblings of candidate in results
    if (siblings.count + 1) / sibling_count >= threshold:
        remove siblings from results
        add parent as aggregated result
        check for cascade (parent may aggregate with its siblings)
    else:
        add candidate as single result
```

**Key behaviors:**

- **Processes all relevant candidates**: No early termination during aggregation
- **Ancestor subsumption**: When an ancestor arrives after its descendants, descendants are
  replaced by the ancestor
- **Cascade**: When siblings aggregate into a parent, that parent may itself trigger
  aggregation with its siblings
- **No duplicate documents**: A document never appears both as parent and child in results
- Parent's score = maximum of constituent scores

**Example with `aggregation_threshold = 0.5`:**

```
Relevant candidates (post-elbow):
1. Subsection A1 (score 10) → add as single
2. Subsection A2 (score 9)  → sibling A1 in results, 2/2 ≥ 0.5 → aggregate to Section A
3. Section A (score 5)      → already in results via cascade, skip
4. Subsection B1 (score 4)  → add as single (only 1 sibling, 1/1 but no siblings in results)

Result: [Section A (aggregated), Subsection B1 (single)]
```

### Aggregated Result Structure

Aggregated results include:

- `id`: Parent chunk's identifier
- `score`: Maximum score among constituents
- `constituents`: List of original matches (for UI expansion)


## Parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| `limit` | 10 | Target number of final results |
| `cutoff_ratio` | 0.3 | Score ratio threshold for elbow detection |
| `max_candidates` | 50 | Hard cap on results entering elbow cutoff |
| `aggregation_threshold` | 0.5 | Sibling fraction required to trigger aggregation |


## Complete Search Flow

```
1. Query Tantivy index (limit × 5 candidates)
   → Candidates ranked by BM25

2. Score normalization (multi-tree only)
   → Each tree's best result normalized to 1.0

3. Elbow cutoff on raw candidates
   → Detect relevance drop-off
   → Determines the "relevant" set

4. Adaptive aggregation (all relevant candidates)
   → Process all post-elbow candidates
   → Aggregate siblings when threshold met
   → Ancestors subsume descendants
   → Cascade upward when possible

5. Final limit truncation
   → Return up to `limit` results
```
