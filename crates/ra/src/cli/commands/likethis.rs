//! Implementation of `ra likethis`.

use std::{collections::HashSet, path::Path, process::ExitCode};

use ra_config::Config;
use ra_document::parse_file;
use ra_index::{MoreLikeThisExplanation, MoreLikeThisParams, SearchParams, SearchResult, Searcher};
use serde::Serialize;

use super::shared::SearchParamsOverrides;
use crate::cli::{
    args::LikeThisCommand,
    context::CommandContext,
    output::{output_aggregated_results, subheader},
};

/// Finds documents similar to a chunk ID or file path.
pub fn run(ctx: &mut CommandContext, cmd: &LikeThisCommand) -> ExitCode {
    let mlt_params = MoreLikeThisParams {
        min_doc_frequency: cmd.min_doc_freq,
        max_doc_frequency: cmd.max_doc_freq.unwrap_or(u64::MAX / 2),
        min_term_frequency: cmd.min_term_freq,
        max_query_terms: cmd.max_terms,
        min_word_length: cmd.min_word_len,
        max_word_length: cmd.max_word_len,
        boost_factor: cmd.boost,
        stop_words: Vec::new(),
    };

    let overrides = SearchParamsOverrides {
        limit: cmd.params.limit,
        aggregation_pool_size: cmd.params.aggregation_pool_size,
        cutoff_ratio: cmd.params.cutoff_ratio,
        aggregation_threshold: cmd.params.aggregation_threshold,
        no_aggregation: cmd.params.no_aggregation,
        trees: cmd.params.trees.clone(),
        verbose: cmd.params.verbose,
    };
    let search_params = overrides.build_params(&ctx.config.search);

    let config = ctx.config.clone();

    let searcher = match ctx.searcher(None, true) {
        Ok(s) => s,
        Err(code) => return code,
    };

    let is_chunk_id = is_chunk_id_format(&cmd.source);

    if cmd.explain.explain {
        return cmd_likethis_explain(
            &cmd.source,
            is_chunk_id,
            &mlt_params,
            &search_params,
            searcher,
            cmd.output.json,
        );
    }

    let results = if is_chunk_id {
        match searcher.search_more_like_this_by_id(&cmd.source, &mlt_params, &search_params) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        match search_more_like_this_by_file(
            &cmd.source,
            &mlt_params,
            &search_params,
            searcher,
            &config,
        ) {
            Ok(r) => r,
            Err(code) => return code,
        }
    };

    let display_source = if is_chunk_id {
        cmd.source.clone()
    } else {
        format!("file:{}", cmd.source)
    };

    output_aggregated_results(
        &results,
        &display_source,
        cmd.output.list,
        cmd.output.matches,
        cmd.output.json,
        cmd.params.verbose,
        searcher,
        None,
    )
}

/// Determines if a source string looks like a chunk ID (contains ':').
fn is_chunk_id_format(source: &str) -> bool {
    if source.contains(':') {
        if source.len() >= 2 && source.chars().nth(1) == Some(':') {
            return false;
        }
        return true;
    }
    false
}

/// Searches for similar documents using a file path as source.
fn search_more_like_this_by_file(
    file_path: &str,
    mlt_params: &MoreLikeThisParams,
    search_params: &SearchParams,
    searcher: &mut Searcher,
    config: &Config,
) -> Result<Vec<SearchResult>, ExitCode> {
    let path = Path::new(file_path);

    if !path.exists() {
        eprintln!("error: file not found: {file_path}");
        return Err(ExitCode::FAILURE);
    }

    if ra_index::is_binary_file(path) {
        eprintln!("error: binary file not supported: {file_path}");
        return Err(ExitCode::FAILURE);
    }

    let parsed = match parse_file(path, "likethis") {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: failed to parse file: {e}");
            return Err(ExitCode::FAILURE);
        }
    };

    let doc = &parsed.document;

    let mut fields: Vec<(&str, String)> = Vec::new();

    if !doc.title.is_empty() {
        fields.push(("title", doc.title.clone()));
    }

    let chunks = doc.extract_chunks();
    let body: String = chunks
        .iter()
        .map(|c| c.body.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    if !body.is_empty() {
        fields.push(("body", body));
    }

    if !doc.tags.is_empty() {
        fields.push(("tags", doc.tags.join(" ")));
    }

    if fields.is_empty() {
        eprintln!("error: no content extracted from file");
        return Err(ExitCode::FAILURE);
    }

    let exclude_doc_ids = compute_exclude_doc_ids_for_file(path, config);

    searcher
        .search_more_like_this_by_fields(fields, mlt_params, search_params, &exclude_doc_ids)
        .map_err(|e| {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        })
}

/// Computes doc IDs to exclude based on whether a file path is in a configured tree.
fn compute_exclude_doc_ids_for_file(path: &Path, config: &Config) -> HashSet<String> {
    let mut exclude = HashSet::new();

    let abs_path = match path.canonicalize() {
        Ok(p) => p,
        Err(_) => return exclude,
    };

    for tree in &config.trees {
        let tree_path = match tree.path.canonicalize() {
            Ok(p) => p,
            Err(_) => continue,
        };

        if let Ok(rel_path) = abs_path.strip_prefix(&tree_path) {
            let doc_id = format!("{}:{}", tree.name, rel_path.display());
            exclude.insert(doc_id);
            break;
        }
    }

    exclude
}

/// Handles --explain mode for likethis command.
fn cmd_likethis_explain(
    source: &str,
    is_chunk_id: bool,
    mlt_params: &MoreLikeThisParams,
    search_params: &SearchParams,
    searcher: &Searcher,
    json: bool,
) -> ExitCode {
    if is_chunk_id {
        let explanation = match searcher.explain_more_like_this(source, mlt_params) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
        };

        output_likethis_explain(&explanation, search_params, json)
    } else {
        let path = Path::new(source);

        if !path.exists() {
            eprintln!("error: file not found: {source}");
            return ExitCode::FAILURE;
        }

        if ra_index::is_binary_file(path) {
            eprintln!("error: binary file not supported: {source}");
            return ExitCode::FAILURE;
        }

        let parsed = match parse_file(path, "likethis") {
            Ok(p) => p,
            Err(e) => {
                eprintln!("error: failed to parse file: {e}");
                return ExitCode::FAILURE;
            }
        };

        let doc = &parsed.document;
        let chunks = doc.extract_chunks();
        let body: String = chunks
            .iter()
            .map(|c| c.body.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");

        let explanation = MoreLikeThisExplanation {
            source_id: format!("file:{source}"),
            source_title: doc.title.clone(),
            source_body_preview: body.chars().take(200).collect(),
            mlt_params: mlt_params.clone(),
            query_repr: "(query built from file content)".to_string(),
        };

        output_likethis_explain(&explanation, search_params, json)
    }
}

/// Outputs the explain information for likethis command.
fn output_likethis_explain(
    explanation: &MoreLikeThisExplanation,
    search_params: &SearchParams,
    json: bool,
) -> ExitCode {
    if json {
        let json_output = JsonLikeThisExplain {
            source_id: explanation.source_id.clone(),
            source_title: explanation.source_title.clone(),
            source_body_preview: explanation.source_body_preview.clone(),
            mlt_params: JsonMltParams {
                min_doc_frequency: explanation.mlt_params.min_doc_frequency,
                max_doc_frequency: explanation.mlt_params.max_doc_frequency,
                min_term_frequency: explanation.mlt_params.min_term_frequency,
                max_query_terms: explanation.mlt_params.max_query_terms,
                min_word_length: explanation.mlt_params.min_word_length,
                max_word_length: explanation.mlt_params.max_word_length,
                boost_factor: explanation.mlt_params.boost_factor,
            },
            search_params: JsonSearchParams::from_params(search_params),
        };

        match serde_json::to_string_pretty(&json_output) {
            Ok(json_str) => println!("{json_str}"),
            Err(e) => {
                eprintln!("error: failed to serialize JSON: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        println!("{}", subheader("Source:"));
        println!("   ID:    {}", explanation.source_id);
        println!("   Title: {}", explanation.source_title);
        println!();

        println!("{}", subheader("Content preview:"));
        println!("   {}...", explanation.source_body_preview);
        println!();

        println!("{}", subheader("MoreLikeThis Parameters:"));
        println!(
            "   min_doc_frequency:  {}",
            explanation.mlt_params.min_doc_frequency
        );
        println!(
            "   max_doc_frequency:  {}",
            explanation.mlt_params.max_doc_frequency
        );
        println!(
            "   min_term_frequency: {}",
            explanation.mlt_params.min_term_frequency
        );
        println!(
            "   max_query_terms:    {}",
            explanation.mlt_params.max_query_terms
        );
        println!(
            "   min_word_length:    {}",
            explanation.mlt_params.min_word_length
        );
        println!(
            "   max_word_length:    {}",
            explanation.mlt_params.max_word_length
        );
        println!(
            "   boost_factor:       {}",
            explanation.mlt_params.boost_factor
        );
        println!();

        println!("{}", subheader("Search Parameters:"));
        println!(
            "   Phase 1: candidate_limit = {}",
            search_params.effective_candidate_limit()
        );
        println!("   Phase 2: cutoff_ratio = {}", search_params.cutoff_ratio);
        println!(
            "   Phase 2: aggregation_pool_size = {}",
            search_params.aggregation_pool_size
        );
        println!(
            "   Phase 3: aggregation_threshold = {}",
            search_params.aggregation_threshold
        );
        println!("   Phase 4: limit = {}", search_params.limit);
        println!(
            "   Aggregation = {}",
            if search_params.disable_aggregation {
                "disabled"
            } else {
                "enabled"
            }
        );
        if !search_params.trees.is_empty() {
            println!("   Trees: {}", search_params.trees.join(", "));
        }
    }

    ExitCode::SUCCESS
}

#[derive(Serialize)]
/// JSON output for likethis explain mode.
struct JsonLikeThisExplain {
    /// Source document or file ID.
    source_id: String,
    /// Title of the source document.
    source_title: String,
    /// Preview of the source body content.
    source_body_preview: String,
    /// MoreLikeThis parameters used.
    mlt_params: JsonMltParams,
    /// Search parameters used.
    search_params: JsonSearchParams,
}

#[derive(Serialize)]
/// JSON output for MLT parameters.
struct JsonMltParams {
    /// Minimum document frequency for terms.
    min_doc_frequency: u64,
    /// Maximum document frequency for terms.
    max_doc_frequency: u64,
    /// Minimum term frequency in source.
    min_term_frequency: usize,
    /// Maximum query terms to use.
    max_query_terms: usize,
    /// Minimum word length.
    min_word_length: usize,
    /// Maximum word length.
    max_word_length: usize,
    /// Boost factor for terms.
    boost_factor: f32,
}

#[derive(Serialize)]
/// JSON output for resolved search parameters.
struct JsonSearchParams {
    /// Maximum candidates to retrieve in Phase 1.
    candidate_limit: usize,
    /// Score ratio threshold for elbow detection.
    cutoff_ratio: f32,
    /// Maximum results after elbow cutoff.
    aggregation_pool_size: usize,
    /// Sibling ratio threshold for aggregation.
    aggregation_threshold: f32,
    /// Whether aggregation is disabled.
    disable_aggregation: bool,
    /// Final result limit after aggregation.
    limit: usize,
    /// Trees to limit results to.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    trees: Vec<String>,
}

impl JsonSearchParams {
    /// Creates from resolved search parameters.
    fn from_params(params: &SearchParams) -> Self {
        Self {
            candidate_limit: params.effective_candidate_limit(),
            cutoff_ratio: params.cutoff_ratio,
            aggregation_pool_size: params.aggregation_pool_size,
            aggregation_threshold: params.aggregation_threshold,
            disable_aggregation: params.disable_aggregation,
            limit: params.limit,
            trees: params.trees.clone(),
        }
    }
}
