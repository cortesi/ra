# Chunking Specification

This document defines how documents are chunked for indexing in ra, and how
search results are processed through elbow cutoff and hierarchical aggregation.


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
- **Heading**: A markdown heading `#`–`######` (h1–h6).
- **Node**: A structural element in the chunk tree (document node or heading
  node).
- **Leaf**: A node with no children.
- **Chunk**: A node emitted to the index. All nodes are indexed (including those
  with empty bodies) to ensure titles remain searchable.
- **Span**: A half-open byte range `[byte_start, byte_end)` into the original
  document content, representing the full extent of a node.


## Parameters

The following parameters control search and result processing:

| Parameter | Default | Description |
|-----------|---------|-------------|
| `candidate_limit` | 100 | Maximum candidates to retrieve from the search index before cutoff and aggregation. |
| `cutoff_ratio` | 0.5 | Score ratio threshold for elbow detection; cut results when next score is less than this fraction of current. |
| `max_results` | 20 | Maximum results to return if no elbow is detected. |
| `aggregation_threshold` | 0.5 | Fraction of a parent's children that must match to trigger aggregation to the parent. |


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
     - `title = document title`, determined by the first available source:
       1. YAML frontmatter `title` field (if present)
       2. First h1 heading in the document (if present)
       3. Filename without extension
     - `slug = None`
     - `span = [0, len(content))`
   - For each heading, create a heading node with:
     - `depth = heading_level` (h1 → 1, …, h6 → 6),
     - `title = heading text`,
     - `slug = computed_slug` (see Slug Generation below),
     - a span assigned in the next step.

3. **Assign heading spans**
   - For each heading node, its span starts at the first byte after the heading
     line's terminating newline (i.e., the first character of the following
     line) and ends at the byte before the next heading of **equal or lower**
     depth (or end of document).
   - Heading lines themselves are deliberately excluded from spans; their
     content is captured in the node's `title` field.
   - Headings whose computed span would be empty (no content between this
     heading and the next) are discarded. This includes consecutive headings
     with no intervening content.

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

2. **Empty body handling**
   - A body is considered empty if it contains only whitespace.
   - All nodes are indexed regardless of whether they have body text. This
     ensures document and section titles remain searchable even when they
     contain only sub-sections with no direct content.

3. **Leaves**
   - A **leaf** is any node with no children.
   - Leaves always have non-empty bodies (the content under that heading or the
     entire document for plain text files).

4. **Chunk sizing**
   - Chunking never drops or alters nodes based on size: every node becomes a
     chunk, regardless of how small or large it is.
   - Large nodes (e.g. long code blocks or extensive text without headings) are
     **not** split. It is the user's responsibility to structure documents with
     sufficient granularity to avoid oversized chunks.

5. **Plain text documents**
   - Files without headings (e.g. `.txt`) produce a single document node whose
     body is the entire file content.
   - The chunk uses the filename (without extension) as its title.

6. **Empty documents**
   - Files that are empty or contain only whitespace are ignored and produce no
     chunks.


## Metadata and Identifiers

Every node in the chunk tree has the following structural metadata:

- `id`: Globally unique chunk identifier, constructed from tree, path, and
  optional slug.
  - Document node: `{tree}:{path}` (slug is `None`)
  - Heading nodes: `{tree}:{path}#{slug}`
  - Examples: `docs:guides/auth.md`, `docs:guides/auth.md#oauth-setup`
- `doc_id`: Document identifier, the same for all nodes in a file.
  - `doc_id = "{tree}:{path}"`
- `parent_id`: The parent node's fully qualified `id` (`None` for the document
  node). Since IDs are globally unique, `parent_id` unambiguously identifies
  the parent across all documents and trees.
- `depth`: Hierarchy depth.
  - `0` for document node,
  - `1` for h1, `2` for h2, … `6` for h6.
- `position`: Document order index (starting at 0), assigned in a pre-order
  traversal of the tree.
- `title`: Heading text or document title.
- `slug`: Optional; used in fragment identifiers for heading nodes.
- `byte_start` / `byte_end`: The node's span in bytes into the original
  document. Body text is derived by reading this span and excluding child spans.
- `sibling_count`: Number of siblings (including this node) under the same
  parent. This count is used as the denominator for aggregation threshold
  calculation. For document nodes (depth 0), `sibling_count = 1`.

### Slug Generation

Slugs provide stable, human-readable fragment identifiers for heading nodes.
The document node has no slug (`None`). Heading nodes always have a slug
derived from the heading text using a GitHub-compatible algorithm.

See [slugs.md](slugs.md) for the complete slug generation algorithm.

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

**Rules**:

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

- Every node becomes one indexed chunk with:
  - `id`, `doc_id`, `title`, `body`, `breadcrumb`, `depth`, `position`,
    `byte_start`, `byte_end`, `sibling_count`, and path/tree metadata.
- Leaves are always chunks with body text; non-leaf parents are also chunks
  (with potentially empty bodies to ensure their titles are searchable).


## Search and Result Processing

Search operates in three phases: candidate retrieval, elbow cutoff, and
hierarchical aggregation.

### Motivation

The chunk tree stores content exactly once, at the most granular level. However,
search results need to be returned at the appropriate level of granularity for
the query. When a user searches for "authentication", they may want:

- A specific paragraph mentioning it (a leaf match),
- An entire section about authentication (aggregated siblings),
- A document titled "Authentication Guide" (a parent match).

Rather than duplicating content at every hierarchy level—which would bloat the
index and distort BM25's IDF calculations—we store chunks once and aggregate
results at search time.

### Phase 1: Candidate Retrieval

Retrieve up to `candidate_limit` results from the search index (Tantivy),
ranked by BM25 score. This limit should be generous (default 100) to ensure
the subsequent phases have enough candidates to work with.

### Phase 2: Elbow Cutoff

BM25 scores are relative within a query and follow a long-tail distribution.
Rather than using arbitrary score thresholds, we detect the natural "elbow"
where relevant results end and noise begins.

**Algorithm**:

1. If fewer than 2 candidates, return all.
2. Compute the score ratio between each adjacent pair:
   `ratio[i] = score[i+1] / score[i]`
3. Find the first index where `ratio[i] < cutoff_ratio`.
4. Return candidates up to and including that index.

**Edge cases**:

- All ratios >= `cutoff_ratio`: no elbow detected; return up to `max_results`
  candidates.
- First ratio < `cutoff_ratio`: return only the first result (single strong
  match).
- Zero or negative scores: trigger immediate cutoff.
- Ratio exactly at threshold: does **not** trigger cutoff (must be strictly
  below).

**Example**:

```
Scores:  [8.0, 7.5, 7.0, 3.2, 3.0, 2.8, 0.9]
Ratios:  [0.94, 0.93, 0.46, 0.94, 0.93, 0.32]
                      ^^^^
                      first ratio < 0.5, cut here

Result: first 3 candidates (scores 8.0, 7.5, 7.0)
```

### Phase 3: Hierarchical Aggregation

After cutoff, aggregate sibling matches under their nearest common parent when
appropriate. The algorithm works bottom-up, processing the deepest matches
first and progressively aggregating toward shallower ancestors.

**Algorithm**:

```
function aggregate(matches):
    # Process depths from deepest to shallowest (stop at depth 1)
    for current_depth from max_depth down to 1:
        # Select only matches at this depth
        at_depth = [m for m in matches if m.depth == current_depth]

        # Group matches at this depth by their parent
        groups = group_by(at_depth, m => m.parent_id)

        for (parent_id, siblings) in groups:
            # All siblings share the same sibling_count
            match_fraction = len(siblings) / siblings[0].sibling_count

            if match_fraction >= aggregation_threshold:
                # Look up parent to get its parent_id for further aggregation
                parent = get_node(parent_id)
                # Aggregate: represent siblings as parent
                replace siblings with AggregatedResult {
                    id: parent_id,
                    score: max(s.score for s in siblings),
                    depth: parent.depth,
                    parent_id: parent.parent_id,
                    sibling_count: parent.sibling_count,
                    constituents: siblings,
                }
            # else: keep as individual results

    # Filter descendants whose ancestors appear in results
    # If a parent appears, its children shouldn't appear separately
    remove results where any ancestor is also in results

    return matches sorted by score descending
```

**Key points**:

- Each iteration processes only matches at the current depth, leaving shallower
  matches untouched until their depth is reached.
- When siblings aggregate into a parent, the parent (now at a shallower depth)
  may itself be aggregated with its own siblings in a subsequent iteration.
- Matches at different depths under the same parent are handled naturally: the
  deeper ones aggregate first, then the results join shallower matches.
- After aggregation completes, any result whose ancestor also appears in results
  is filtered out (to avoid showing both a document and its subsection).

**Score calculation**: When aggregating siblings into a parent, the parent's
score is the **maximum** of the sibling scores. If the parent also matched
directly (i.e., it has its own indexed body text that matched the query), take
the maximum of the parent's direct score and the aggregated children's scores.

**Depth limit**: Aggregation can cascade all the way to the document level
(depth 0) if enough siblings match at each level.

**Output**: A flat list of results, each either:

- An original leaf/chunk match, or
- An aggregated parent with a `constituents` list (for UI expansion).

Results are sorted by score descending.

### Complete Search Flow

```
1. tantivy_search(query, limit=candidate_limit)
   → ranked candidates with BM25 scores

2. elbow_cutoff(candidates, cutoff_ratio, max_results)
   → relevant matches only

3. aggregate(matches, aggregation_threshold)
   → final results (individual chunks + aggregated parents)

4. filter descendants of results that appear as ancestors

5. sort by score descending
   → return to caller
```

This design keeps content non-duplicated in the index while providing
appropriately granular results through post-search aggregation.
