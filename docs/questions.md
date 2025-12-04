# Open Questions

## Tree-Filtered BM25 Scoring

When filtering search results to specific trees via `SearchParams::trees`, BM25 scoring
uses corpus-wide statistics (document frequency, average document length) computed across
**all** indexed documents, not just those in the filtered trees.

This means a term's IDF reflects its rarity across the entire index. If "async" appears
in 100 documents total but only 5 are in the filtered tree, the IDF is computed using
n=100, not n=5.

**Why this is usually fine:**

- Relative ranking within a single query's results remains meaningful
- Users compare results within a query, not across different queries
- The existing `tree:` query syntax has identical behavior

**When this might matter:**

- Score-based thresholds (e.g., "show results with score > X")
- Comparing scores across queries with different tree filters
- Trees with very different term distributions

**Alternatives if needed:**

1. **Separate indices per tree** - correct stats but complex cross-tree search
2. **Per-tree statistics** - store `(term, tree) -> doc_count`, custom scorer
3. **Post-hoc re-ranking** - retrieve candidates, recompute with filtered stats
