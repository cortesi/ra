# Configuration

ra uses TOML files named `.ra.toml` for configuration. This guide covers setup and tuning.


## File Locations

ra discovers configuration by walking up from the current working directory:

1. Collect all `.ra.toml` files from CWD to filesystem root
2. Append `~/.ra.toml` if present (global config, lowest precedence)
3. Merge configurations; nearer files take precedence

Set `root = true` in a `.ra.toml` to stop discovery from climbing further.

The search index is stored in `.ra/index/` next to the highest-precedence `.ra.toml`. If only
the global config exists, the index is `~/.ra/index/`.

Use `ra status` to see which configs were found and which defines the index location.


## Merge Rules

- **Scalar settings**: Nearer files override more distant files
- **Trees**: Merged by name; nearer definition completely replaces more distant definition
- **Context patterns**: Merged by glob key; nearer definition wins


## Minimal Example

```toml
[tree.docs]
path = "./docs"

[settings]
default_limit = 10
```

Run `ra update` after creating your first config.


## Trees

Trees name the document collections ra indexes.

```toml
[tree.guides]
path = "./docs"
include = ["**/*.md", "**/*.txt"]  # default if omitted
exclude = ["**/drafts/**"]          # optional
```

| Key | Required | Description |
|-----|----------|-------------|
| `path` | Yes | Root directory; relative to config file |
| `include` | No | Glob patterns to index (default: `**/*.md`, `**/*.txt`) |
| `exclude` | No | Glob patterns to skip |

Trees defined in `~/.ra.toml` are global. Trees defined elsewhere are local and receive a
relevance boost in search results.


## Settings

### General (`[settings]`)

| Key | Default | Description |
|-----|---------|-------------|
| `default_limit` | 5 | Results per query when no limit specified |
| `local_boost` | 1.5 | Relevance multiplier for local trees vs global |
| `chunk_at_headings` | true | Preserve markdown heading hierarchy |
| `max_chunk_size` | 50000 | Warning threshold for oversized chunks |

### Search (`[search]`)

| Key | Default | Description |
|-----|---------|-------------|
| `stemmer` | `"english"` | Language for stemming (see [search.md](search.md)) |
| `fuzzy_distance` | 1 | Levenshtein edit distance; 0 disables fuzzy matching |

### Context (`[context]`)

Settings for `ra context` and context analysis.

| Key | Default | Description |
|-----|---------|-------------|
| `limit` | 10 | Max chunks returned by `ra context` |
| `min_term_frequency` | 2 | Skip terms appearing fewer times |
| `min_word_length` | 4 | Skip shorter tokens |
| `max_word_length` | 30 | Skip longer tokens |
| `sample_size` | 50000 | Bytes to read from large files |


## Context Patterns

Map file globs to hint terms:

```toml
[context.patterns]
"*.rs" = ["rust"]
"src/api/**" = ["http", "api"]
```

Patterns appear in `ra inspect ctx` and are available to custom tooling. They are not yet
incorporated into the generated query from `ra context`.


## Global vs Project Configs

- Put broadly useful trees and defaults in `~/.ra.toml`
- Put project-specific trees and overrides in `.ra.toml` inside the project
- Use `root = true` to isolate a project from parent directories and global config


## Common Workflows

### Start a New Project

```bash
cd /path/to/project
ra init
# Edit .ra.toml to add your trees
ra update
```

### Add a Document Tree

Add to `.ra.toml`:

```toml
[tree.notes]
path = "./notes"
```

Then run `ra update`.

### Tune Search Behavior

Adjust settings in `.ra.toml`:

```toml
[settings]
default_limit = 10
local_boost = 2.0

[search]
fuzzy_distance = 2
```

Changes trigger automatic reindexing on next use.

### Tune Context Analysis

```toml
[context]
limit = 15
min_term_frequency = 3
```


## Troubleshooting

**Wrong configs loading?**

Run `ra status` to see which files are active.

**Paths resolving incorrectly?**

Paths are resolved relative to the config file that declared the tree, not the working
directory.

**Search results seem off?**

Check `stemmer` and `fuzzy_distance`, then rebuild with `ra update`.

**Warnings about large chunks?**

Add more headings to your documents to provide structure, or adjust `max_chunk_size` if large
chunks are intentional.
