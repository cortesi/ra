//! Implementation of `ra search`.

use std::process::ExitCode;

use ra_index::parse_query;

use super::shared::SearchParamsOverrides;
use crate::cli::{
    args::SearchCommand,
    context::CommandContext,
    output::{dim, format_elbow_reason, output_aggregated_results, subheader},
};

/// Searches the index and prints matching chunks.
pub fn run(ctx: &mut CommandContext, cmd: &SearchCommand) -> ExitCode {
    let overrides = SearchParamsOverrides {
        limit: cmd.params.limit,
        aggregation_pool_size: cmd.params.aggregation_pool_size,
        cutoff_ratio: cmd.params.cutoff_ratio,
        aggregation_threshold: cmd.params.aggregation_threshold,
        no_aggregation: cmd.params.no_aggregation,
        trees: cmd.params.trees.clone(),
        verbose: cmd.params.verbose,
    };

    let params = overrides.build_params(&ctx.config.search);

    let searcher = match ctx.searcher(cmd.fuzzy, true) {
        Ok(s) => s,
        Err(code) => return code,
    };

    let combined_query = if cmd.queries.len() == 1 {
        cmd.queries[0].clone()
    } else {
        cmd.queries
            .iter()
            .map(|q| format!("({q})"))
            .collect::<Vec<_>>()
            .join(" OR ")
    };

    let (results, stats) = match searcher.search_aggregated_with_stats(&combined_query, &params) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: search failed: {e}");
            return ExitCode::FAILURE;
        }
    };

    if cmd.explain.explain {
        println!("{}", subheader("Query:"));
        println!("   {combined_query}");
        println!();

        match parse_query(&combined_query) {
            Ok(Some(expr)) => {
                println!("{}", subheader("Parsed AST:"));
                let expr_str = expr.to_string();
                for line in expr_str.lines() {
                    println!("   {line}");
                }
                println!();
            }
            Ok(None) => {
                println!("{}", dim("(empty query)"));
                println!();
            }
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
        }

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

        println!("{}", subheader("Pipeline Statistics:"));
        println!("  Raw candidates:      {}", stats.raw_candidate_count);
        println!("  After aggregation:   {}", stats.post_aggregation_count);
        println!("  After elbow cutoff:  {}", stats.post_elbow_count);
        println!("  Final results:       {}", stats.final_count);
        println!();
        println!("  Elbow: {}", format_elbow_reason(&stats.elbow.reason));
        println!();

        return ExitCode::SUCCESS;
    }

    output_aggregated_results(
        &results,
        &combined_query,
        &cmd.output,
        cmd.params.verbose,
        searcher,
        Some(&stats),
    )
}
