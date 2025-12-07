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


## Aggregated Result Score Computation

When sibling chunks are aggregated into a parent node, we need to compute a score for the
aggregated result. Currently we use `max(constituent_scores)`, but this may not be optimal.

**Current approach: `max(constituent_scores)`**

- Simple and predictable
- Parent scores at least as high as its best child
- Doesn't reward breadth of matches

**Alternative approaches to consider:**

1. **Sum of constituent scores** - Rewards breadth; a parent with many matching children scores
   higher than one with a single strong match. Risk: inflates scores for large sections.

2. **Max + bonus per additional match** - Compromise: `max + 0.1 * (count - 1) * max`. Rewards
   breadth without runaway inflation.

3. **Weighted sum with diminishing returns** - `sum(scores) / sqrt(count)`. Balances breadth
   and depth.

4. **Include parent's direct score** - If the parent node itself matched the query (not just
   its children), should that score factor in? Currently it does via `max()`, but explicit
   combination might be better.

**Considerations:**

- Score magnitude affects elbow cutoff detection
- Aggregated results compete with non-aggregated results in final ranking
- Users expect parent sections to rank appropriately vs. leaf matches
