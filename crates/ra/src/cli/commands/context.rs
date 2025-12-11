//! Implementation of `ra context`.

use std::{path::Path, process::ExitCode};

use comfy_table::{Cell, Table, presets::UTF8_FULL_CONDENSED};
use ra_config::MatchedRules;
use ra_index::{ContextAnalysisResult, ContextSearch, ContextWarning, PipelineStats, SearchParams};
use serde::Serialize;

use super::shared::SearchParamsOverrides;
use crate::cli::{
    args::ContextCommand,
    context::CommandContext,
    output::{dim, format_elbow_reason, output_aggregated_results, subheader},
};

/// Analyzes source files and searches for relevant context.
pub fn run(ctx: &mut CommandContext, cmd: &ContextCommand) -> ExitCode {
    let context_settings = ctx.config.context.clone();
    let search_defaults = ctx.config.search.clone();

    let max_terms = cmd.terms.unwrap_or(context_settings.terms);
    let algorithm = cmd.algorithm.unwrap_or_default();

    let searcher = match ctx.searcher(cmd.fuzzy, true) {
        Ok(s) => s,
        Err(code) => return code,
    };

    let mut context_search =
        match ContextSearch::with_algorithm(searcher, &context_settings, max_terms, algorithm) {
            Ok(cs) => cs,
            Err(e) => {
                eprintln!("error: failed to initialize context search: {e}");
                return ExitCode::FAILURE;
            }
        };

    let mut file_paths: Vec<&Path> = Vec::new();
    for file_str in &cmd.files {
        let path = Path::new(file_str.as_str());
        if !path.exists() {
            eprintln!("error: file not found: {file_str}");
            return ExitCode::FAILURE;
        }
        if ra_index::is_binary_file(path) {
            eprintln!("warning: skipping binary file: {file_str}");
            continue;
        }
        file_paths.push(path);
    }

    let analysis = context_search.analyze(&file_paths, &cmd.params.trees);
    print_context_warnings(&analysis.warnings);

    if analysis.is_empty() {
        eprintln!("error: no analyzable files provided");
        return ExitCode::FAILURE;
    }

    let overrides = SearchParamsOverrides {
        limit: cmd.params.limit,
        aggregation_pool_size: cmd.params.aggregation_pool_size,
        cutoff_ratio: cmd.params.cutoff_ratio,
        aggregation_threshold: cmd.params.aggregation_threshold,
        no_aggregation: cmd.params.no_aggregation,
        trees: cmd.params.trees.clone(),
        verbose: cmd.params.verbose,
    };

    let params =
        overrides.build_params_with_rule_overrides(&search_defaults, &analysis.merged_rules.search);

    let (results, analysis, stats) = if analysis.query_expr.is_some() {
        match context_search.search_with_analysis_stats(analysis, &params) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: context search failed: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        let empty_stats = PipelineStats::empty(params.cutoff_ratio, params.aggregation_pool_size);
        (Vec::new(), analysis, empty_stats)
    };

    if cmd.explain.explain {
        return output_context_explain(&analysis, &params, &stats, cmd.output.json);
    }

    if analysis.query_expr.is_none() {
        println!("{}", dim("No context terms extracted."));
        return ExitCode::SUCCESS;
    }

    let query_display = analysis
        .query_string()
        .unwrap_or_else(|| String::from("(empty)"));
    output_aggregated_results(
        &results,
        &query_display,
        cmd.output.list,
        cmd.output.matches,
        cmd.output.json,
        cmd.params.verbose,
        context_search.searcher(),
        Some(&stats),
    )
}

/// Prints context analysis warnings to stderr.
fn print_context_warnings(warnings: &[ContextWarning]) {
    for warning in warnings {
        eprintln!(
            "warning: failed to read {}: {}",
            warning.path, warning.reason
        );
    }
}

/// Prints search overrides from matched rules, if any.
fn print_search_overrides(overrides: &ra_config::SearchOverrides, indent: &str) {
    if overrides.is_empty() {
        return;
    }
    let mut parts = Vec::new();
    if let Some(limit) = overrides.limit {
        parts.push(format!("limit={limit}"));
    }
    if let Some(aggregation_pool_size) = overrides.aggregation_pool_size {
        parts.push(format!("aggregation_pool_size={aggregation_pool_size}"));
    }
    if let Some(cutoff_ratio) = overrides.cutoff_ratio {
        parts.push(format!("cutoff_ratio={cutoff_ratio}"));
    }
    if let Some(aggregation_threshold) = overrides.aggregation_threshold {
        parts.push(format!("aggregation_threshold={aggregation_threshold}"));
    }
    println!("{indent}Search: {}", parts.join(", "));
}

/// Outputs explain mode information for context analysis.
fn output_context_explain(
    analysis_result: &ContextAnalysisResult,
    params: &SearchParams,
    stats: &PipelineStats,
    json: bool,
) -> ExitCode {
    if json {
        let json_output = JsonContextExplain {
            merged_rules: analysis_result.merged_rules.clone(),
            search_params: params.clone(),
            files: analysis_result
                .files
                .iter()
                .map(|fa| JsonFileAnalysis {
                    file: fa.path.clone(),
                    terms: fa.analysis.ranked_terms.clone(),
                    query: fa.analysis.query_string().map(str::to_string),
                    matched_rules: fa.matched_rules.clone(),
                })
                .collect(),
            pipeline: stats.clone(),
        };

        match serde_json::to_string_pretty(&json_output) {
            Ok(json_str) => println!("{json_str}"),
            Err(e) => {
                eprintln!("error: failed to serialize JSON: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        println!("{}", subheader("Applied context rules:"));
        if analysis_result.merged_rules.is_empty() {
            println!("  {}", dim("(none)"));
        } else {
            if !analysis_result.merged_rules.terms.is_empty() {
                println!(
                    "  Terms:   {}",
                    analysis_result.merged_rules.terms.join(", ")
                );
            }
            if !analysis_result.merged_rules.trees.is_empty() {
                println!(
                    "  Trees:   {}",
                    analysis_result.merged_rules.trees.join(", ")
                );
            }
            if !analysis_result.merged_rules.include.is_empty() {
                println!(
                    "  Include: {}",
                    analysis_result.merged_rules.include.join(", ")
                );
            }
            print_search_overrides(&analysis_result.merged_rules.search, "  ");
        }
        println!();

        println!("{}", subheader("Search Parameters:"));
        println!(
            "   Phase 1: candidate_limit = {}",
            params.effective_candidate_limit()
        );
        println!("   Phase 2: cutoff_ratio = {}", params.cutoff_ratio);
        println!(
            "   Phase 2: aggregation_pool_size = {}",
            params.aggregation_pool_size
        );
        println!(
            "   Phase 3: aggregation_threshold = {}",
            params.aggregation_threshold
        );
        println!("   Phase 4: limit = {}", params.limit);
        println!(
            "   Aggregation = {}",
            if params.disable_aggregation {
                "disabled"
            } else {
                "enabled"
            }
        );
        println!();

        for fa in &analysis_result.files {
            println!("{}", subheader(&format!("File: {}", fa.path)));
            println!();

            if !fa.matched_rules.is_empty() {
                println!("{}", dim("Matched rules:"));
                if !fa.matched_rules.terms.is_empty() {
                    println!("  Terms:   {}", fa.matched_rules.terms.join(", "));
                }
                if !fa.matched_rules.trees.is_empty() {
                    println!("  Trees:   {}", fa.matched_rules.trees.join(", "));
                }
                if !fa.matched_rules.include.is_empty() {
                    println!("  Include: {}", fa.matched_rules.include.join(", "));
                }
                print_search_overrides(&fa.matched_rules.search, "  ");
                println!();
            }

            let algo_name = fa.analysis.algorithm.to_string();
            println!("{}", subheader(&format!("Ranked terms ({algo_name}):")));
            if fa.analysis.ranked_terms.is_empty() {
                println!("  {}", dim("(none)"));
            } else {
                let mut table = Table::new();
                table.load_preset(UTF8_FULL_CONDENSED);

                let is_tfidf = fa.analysis.algorithm == ra_context::KeywordAlgorithm::TfIdf;

                if is_tfidf {
                    table.set_header(vec!["Term", "Source", "Weight", "Freq", "IDF", "Score"]);
                    for rt in &fa.analysis.ranked_terms {
                        table.add_row(vec![
                            Cell::new(&rt.term.term),
                            Cell::new(rt.term.source.to_string()),
                            Cell::new(format!("{:.1}", rt.term.weight)),
                            Cell::new(rt.term.frequency.to_string()),
                            Cell::new(format!("{:.2}", rt.idf)),
                            Cell::new(format!("{:.2}", rt.score)),
                        ]);
                    }
                } else {
                    table.set_header(vec!["Term", "Score"]);
                    for rt in &fa.analysis.ranked_terms {
                        table.add_row(vec![
                            Cell::new(&rt.term.term),
                            Cell::new(format!("{:.2}", rt.score)),
                        ]);
                    }
                }

                println!("{table}");
            }
            println!();

            println!("{}", subheader("Generated query:"));
            if let Some(expr) = fa.analysis.query_expr() {
                let tree = expr.to_string();
                for line in tree.lines() {
                    println!("  {line}");
                }
            } else {
                println!("  {}", dim("(no query generated)"));
            }
            println!();
        }

        println!("{}", subheader("Pipeline Statistics:"));
        println!("  Raw candidates:      {}", stats.raw_candidate_count);
        println!("  After aggregation:   {}", stats.post_aggregation_count);
        println!("  After elbow cutoff:  {}", stats.post_elbow_count);
        println!("  Final results:       {}", stats.final_count);
        println!();
        println!("  Elbow: {}", format_elbow_reason(&stats.elbow.reason));
        println!();
    }

    ExitCode::SUCCESS
}

/// JSON output for context explain mode.
#[derive(Serialize)]
struct JsonContextExplain {
    /// Merged context rules across all files.
    merged_rules: MatchedRules,
    /// Search parameters used for the query.
    search_params: SearchParams,
    /// Per-file analysis details.
    files: Vec<JsonFileAnalysis>,
    /// Pipeline statistics from executing the search.
    pipeline: PipelineStats,
}

/// JSON output for a single file analysis.
#[derive(Serialize)]
struct JsonFileAnalysis {
    /// File path analyzed.
    file: String,
    /// Ranked terms extracted from the file.
    terms: Vec<ra_context::RankedTerm>,
    /// Generated query string, if any.
    query: Option<String>,
    /// Context rules matched for this file.
    matched_rules: MatchedRules,
}
