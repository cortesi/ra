//! Rendering and JSON serialization for CLI output.

use std::{collections::HashSet, ops::Range, process::ExitCode};

pub use ra_highlight::{breadcrumb, dim, header, subheader, warning};
use ra_highlight::{format_body, theme};
use ra_index::{ElbowReason, PipelineStats, SearchResult, Searcher, merge_ranges};
use serde::Serialize;

use crate::cli::args::{OutputMode, OutputOptions};

/// JSON output for a single query's results.
#[derive(Serialize)]
struct JsonQueryResults {
    /// The original query string.
    query: String,
    /// Results for this query.
    results: Vec<SearchResult>,
    /// Total matches returned.
    total_matches: usize,
}

/// JSON output for search-like commands.
#[derive(Serialize)]
struct JsonSearchOutput {
    /// Results grouped by query.
    queries: Vec<JsonQueryResults>,
}

/// Rendering style for aggregated search results.
#[derive(Clone, Copy)]
enum DisplayMode {
    /// Full content output.
    Full,
    /// Header-only listing output.
    List,
    /// Matches-only line output.
    Matches,
}

/// Retrieves the full body for a search result, falling back to the indexed body.
fn read_full_body(result: &SearchResult, searcher: &Searcher) -> String {
    let c = result.candidate();
    searcher
        .read_full_content(&c.tree, &c.path, c.byte_start, c.byte_end)
        .unwrap_or_else(|_| c.body.clone())
}

/// Outputs aggregated search-like results in various formats.
pub fn output_aggregated_results(
    results: &[SearchResult],
    query: &str,
    output: &OutputOptions,
    verbose: u8,
    searcher: &Searcher,
    stats: Option<&PipelineStats>,
) -> ExitCode {
    if matches!(output.mode, OutputMode::Json) {
        let json_output = JsonSearchOutput {
            queries: vec![JsonQueryResults {
                query: query.to_string(),
                total_matches: results.len(),
                results: results.to_vec(),
            }],
        };
        match serde_json::to_string_pretty(&json_output) {
            Ok(json_str) => println!("{json_str}"),
            Err(e) => {
                eprintln!("error: failed to serialize JSON: {e}");
                return ExitCode::FAILURE;
            }
        }
        return ExitCode::SUCCESS;
    }

    let mode = match output.mode {
        OutputMode::List => DisplayMode::List,
        OutputMode::Matches => DisplayMode::Matches,
        OutputMode::Full | OutputMode::Json => DisplayMode::Full,
    };

    output_text_results(results, verbose, searcher, stats, mode)
}

/// Renders non-JSON results for the selected display mode.
fn output_text_results(
    results: &[SearchResult],
    verbose: u8,
    searcher: &Searcher,
    stats: Option<&PipelineStats>,
    mode: DisplayMode,
) -> ExitCode {
    if results.is_empty() {
        if verbose > 0
            && let Some(stats) = stats
        {
            print_pipeline_stats(stats);
        }
        println!("{}", dim("No results found."));
        if matches!(mode, DisplayMode::List) {
            println!();
        }
        return ExitCode::SUCCESS;
    }

    let collect_totals = verbose > 0 && !matches!(mode, DisplayMode::Matches);
    let mut total_words = 0;
    let mut total_chars = 0;

    for result in results {
        let full_body = read_full_body(result, searcher);
        if collect_totals {
            total_words += full_body.split_whitespace().count();
            total_chars += full_body.len();
        }

        let formatted = format_aggregated_result(result, verbose, &full_body, mode);
        match mode {
            DisplayMode::Full => {
                print!("{formatted}");
                println!();
            }
            _ => print!("{formatted}"),
        }
    }

    if collect_totals {
        let mut summary_parts = vec![
            format!("{} results", results.len()),
            format!("{} words", total_words),
            format!("{} chars", total_chars),
        ];

        if let Some(stats) = stats {
            summary_parts.push(format_elbow_summary(&stats.elbow.reason));
        }

        println!("{}", dim(&format!("─── {} ───", summary_parts.join(", "))));
    }

    if matches!(mode, DisplayMode::List) {
        println!();
    }

    ExitCode::SUCCESS
}

/// Prints detailed pipeline statistics.
fn print_pipeline_stats(stats: &PipelineStats) {
    println!("{}", dim("Pipeline:"));
    println!(
        "{}",
        dim(&format!(
            "  Raw candidates: {} → After aggregation: {} → After elbow: {} → Final: {}",
            stats.raw_candidate_count,
            stats.post_aggregation_count,
            stats.post_elbow_count,
            stats.final_count
        ))
    );
    println!(
        "{}",
        dim(&format!(
            "  Elbow: {}",
            format_elbow_reason(&stats.elbow.reason)
        ))
    );
    println!();
}

/// Formats the elbow reason as a short summary for display/JSON.
pub fn format_elbow_summary(reason: &ElbowReason) -> String {
    match reason {
        ElbowReason::RatioBelowThreshold { ratio, .. } => {
            format!("elbow at ratio {:.2}", ratio)
        }
        ElbowReason::ZeroOrNegativeScore { .. } => "elbow at zero score".to_string(),
        ElbowReason::MaxResultsReached => "no elbow (pool limit)".to_string(),
        ElbowReason::TooFewCandidates => "no elbow (few candidates)".to_string(),
    }
}

/// Formats the elbow reason for detailed display.
pub fn format_elbow_reason(reason: &ElbowReason) -> String {
    match reason {
        ElbowReason::RatioBelowThreshold {
            ratio,
            score_before,
            score_after,
        } => {
            format!(
                "ratio {:.3} < threshold (scores {:.2} → {:.2})",
                ratio, score_before, score_after
            )
        }
        ElbowReason::ZeroOrNegativeScore { score } => {
            format!("zero/negative score encountered ({:.2})", score)
        }
        ElbowReason::MaxResultsReached => "no elbow found, hit pool size limit".to_string(),
        ElbowReason::TooFewCandidates => "too few candidates for elbow detection".to_string(),
    }
}

/// Formats an aggregated search result for the given display mode.
fn format_aggregated_result(
    result: &SearchResult,
    verbose: u8,
    full_body: &str,
    mode: DisplayMode,
) -> String {
    let mut output = String::new();

    let c = result.candidate();
    let header_id = highlight_id_with_path(&c.id, &c.path_match_ranges);
    if verbose > 0 && result.is_aggregated() {
        let count = result.constituents().unwrap().len();
        output.push_str(&format!(
            "─── {} [aggregated: {} matches] ───\n",
            header_id, count
        ));
    } else {
        output.push_str(&format!("─── {} ───\n", header_id));
    }

    let breadcrumb_line =
        highlight_breadcrumb_title(&c.breadcrumb(), c.title(), &c.hierarchy_match_ranges);
    if matches!(mode, DisplayMode::Matches) {
        output.push_str(&format!("{breadcrumb_line}\n\n"));
    } else {
        output.push_str(&format!("{breadcrumb_line}\n"));
    }

    if verbose > 0 && !matches!(mode, DisplayMode::Matches) {
        let word_count = full_body.split_whitespace().count();
        let stats = format!(
            "{} words, {} chars, score {:.2}",
            word_count,
            full_body.len(),
            c.score
        );
        output.push_str(&format!("{}\n", dim(&stats)));
    }

    if verbose > 0 {
        output.push_str(&format_match_details(result, verbose));
    }

    match mode {
        DisplayMode::Full => {
            if !full_body.is_empty() {
                let ranges = aggregated_match_ranges(result, full_body);
                output.push('\n');
                output.push_str(&format_body(full_body, &ranges));
                output.push('\n');
            }
        }
        DisplayMode::List => output.push('\n'),
        DisplayMode::Matches => {
            let ranges = aggregated_match_ranges(result, full_body);
            if !ranges.is_empty() {
                output.push_str(&extract_matching_lines(full_body, &ranges));
                output.push('\n');
            }
            output.push('\n');
        }
    }

    output
}

/// Formats verbose match details for a result.
fn format_match_details(result: &SearchResult, verbosity: u8) -> String {
    let mut output = String::new();

    let details = result.match_details();

    if let Some(details) = details {
        let matched_in_doc: HashSet<&str> = details
            .field_matches
            .values()
            .flat_map(|fm| fm.term_frequencies.keys().map(String::as_str))
            .collect();

        if verbosity >= 1 && !details.original_terms.is_empty() {
            let mut terms_output = String::new();
            for (orig, stemmed) in details
                .original_terms
                .iter()
                .zip(details.stemmed_terms.iter())
            {
                let stemmed_matched = matched_in_doc.contains(stemmed.as_str());
                let fuzzy_matches: Vec<&String> =
                    if let Some(matches) = details.term_mappings.get(stemmed) {
                        matches
                            .iter()
                            .filter(|m| *m != stemmed && matched_in_doc.contains(m.as_str()))
                            .collect()
                    } else {
                        Vec::new()
                    };

                if stemmed_matched || !fuzzy_matches.is_empty() {
                    terms_output.push_str(&format!("  {}\n", orig));

                    if orig != stemmed {
                        terms_output.push_str(&format!("    {} {}\n", dim("stem:"), stemmed));
                    }

                    if !fuzzy_matches.is_empty() {
                        terms_output.push_str(&format!(
                            "    {} {}\n",
                            dim("fuzzy:"),
                            fuzzy_matches
                                .iter()
                                .map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                }
            }
            if !terms_output.is_empty() {
                output.push_str(&format!("{}\n", dim("terms:")));
                output.push_str(&terms_output);
            }
        }

        if verbosity >= 2 {
            output.push_str(&format!(
                "{} base={:.2}, boost={:.2}\n",
                dim("scores:"),
                details.base_score,
                details.local_boost
            ));

            if !details.field_scores.is_empty() {
                let mut field_scores: Vec<String> = details
                    .field_scores
                    .iter()
                    .filter(|(_, score)| **score > 0.0)
                    .map(|(field, score)| format!("{field}={score:.2}"))
                    .collect();
                field_scores.sort();
                if !field_scores.is_empty() {
                    output.push_str(&format!(
                        "  {} {}\n",
                        dim("fields:"),
                        field_scores.join(", ")
                    ));
                }
            }

            if !details.field_matches.is_empty() {
                for (field, field_match) in &details.field_matches {
                    if field_match.term_frequencies.is_empty() {
                        continue;
                    }
                    let term_info: Vec<String> = field_match
                        .term_frequencies
                        .iter()
                        .map(|(term, freq)| format!("{term} x {freq}"))
                        .collect();
                    output.push_str(&format!(
                        "  {} {}\n",
                        dim(&format!("{field}:")),
                        term_info.join(", ")
                    ));
                }
            }
        }

        if verbosity >= 3
            && let Some(explanation) = &details.score_explanation
        {
            output.push_str(&format!("{}\n", dim("tantivy explanation:")));
            for line in explanation.lines() {
                output.push_str(&format!("  {}\n", dim(line)));
            }
        }
    } else if verbosity >= 1 {
        output.push_str(&format!("{}\n", dim("(match details not collected)")));
    }

    output
}

/// Highlights ranges within text while preserving a base style.
fn highlight_with_base(
    text: &str,
    ranges: &[Range<usize>],
    base_prefix: &str,
    base_suffix: &str,
) -> String {
    if ranges.is_empty() {
        return format!("{base_prefix}{text}{base_suffix}");
    }

    let match_prefix = theme::MATCH.prefix();
    let match_suffix = theme::MATCH.suffix();
    let mut output = String::new();
    output.push_str(base_prefix);

    let mut cursor = 0;
    for range in ranges {
        if range.start > cursor {
            output.push_str(&text[cursor..range.start]);
        }
        output.push_str(&match_prefix);
        output.push_str(&text[range.start..range.end]);
        output.push_str(match_suffix);
        output.push_str(base_prefix);
        cursor = range.end;
    }

    if cursor < text.len() {
        output.push_str(&text[cursor..]);
    }
    output.push_str(base_suffix);
    output
}

/// Highlights the path portion inside an ID using path match ranges.
fn highlight_id_with_path(id: &str, path_ranges: &[Range<usize>]) -> String {
    let Some(colon) = id.find(':') else {
        let header_prefix = theme::HEADER.prefix();
        return highlight_with_base(id, &[], &header_prefix, theme::HEADER.suffix());
    };
    let path_start = colon + 1;
    let path_end = id.find('#').unwrap_or(id.len());
    let path_len = path_end.saturating_sub(path_start);

    let header_prefix = theme::HEADER.prefix();

    let shifted: Vec<Range<usize>> = path_ranges
        .iter()
        .filter_map(|r| {
            if r.end <= path_len {
                Some((r.start + path_start)..(r.end + path_start))
            } else {
                None
            }
        })
        .collect();

    highlight_with_base(id, &shifted, &header_prefix, theme::HEADER.suffix())
}

/// Highlights the trailing title segment inside a breadcrumb.
fn highlight_breadcrumb_title(
    breadcrumb: &str,
    title: &str,
    title_ranges: &[Range<usize>],
) -> String {
    let breadcrumb_prefix = theme::BREADCRUMB.prefix();
    if title_ranges.is_empty() || title.is_empty() {
        return highlight_with_base(
            breadcrumb,
            &[],
            &breadcrumb_prefix,
            theme::BREADCRUMB.suffix(),
        );
    }

    if let Some(pos) = breadcrumb.rfind(title) {
        let shifted: Vec<Range<usize>> = title_ranges
            .iter()
            .map(|r| (r.start + pos)..(r.end + pos))
            .collect();
        highlight_with_base(
            breadcrumb,
            &shifted,
            &breadcrumb_prefix,
            theme::BREADCRUMB.suffix(),
        )
    } else {
        highlight_with_base(
            breadcrumb,
            &[],
            &breadcrumb_prefix,
            theme::BREADCRUMB.suffix(),
        )
    }
}

/// Computes highlight ranges for an aggregated result by mapping child matches into parent body.
fn aggregated_match_ranges(result: &SearchResult, full_body: &str) -> Vec<Range<usize>> {
    match result {
        SearchResult::Single(candidate) => candidate.match_ranges.clone(),
        SearchResult::Aggregated {
            parent,
            constituents,
        } => {
            let parent_start = parent.byte_start as usize;
            let mut ranges = Vec::new();
            for child in constituents {
                let shift = child.byte_start as usize;
                if shift < parent_start {
                    continue;
                }
                let offset = shift - parent_start;
                for r in &child.match_ranges {
                    let start = offset + r.start;
                    let end = offset + r.end;
                    if end <= full_body.len() && start < end {
                        ranges.push(start..end);
                    }
                }
            }
            merge_ranges(ranges, Vec::new())
        }
    }
}

/// Extracts only the lines containing matches, with highlighting.
fn extract_matching_lines(body: &str, match_ranges: &[Range<usize>]) -> String {
    use std::{collections::BTreeSet, iter};

    let mut matching_line_nums: BTreeSet<usize> = BTreeSet::new();

    let line_starts: Vec<usize> = iter::once(0)
        .chain(body.match_indices('\n').map(|(i, _)| i + 1))
        .collect();

    for range in match_ranges {
        for (line_num, &start) in line_starts.iter().enumerate() {
            let end = line_starts.get(line_num + 1).copied().unwrap_or(body.len());
            if range.start < end && range.end > start {
                matching_line_nums.insert(line_num);
            }
        }
    }

    let mut lines_with_ranges: Vec<(&str, Vec<Range<usize>>)> = Vec::new();
    for line_num in matching_line_nums {
        let line_start = line_starts[line_num];
        let line_end = line_starts.get(line_num + 1).copied().unwrap_or(body.len());

        let line_content = body[line_start..line_end].trim_end_matches('\n');

        let line_ranges: Vec<Range<usize>> = match_ranges
            .iter()
            .filter_map(|r| {
                if r.start < line_end && r.end > line_start {
                    let start = r.start.max(line_start) - line_start;
                    let end = r.end.min(line_end) - line_start;
                    let trimmed_len = line_content.len();
                    if start < trimmed_len {
                        Some(start..end.min(trimmed_len))
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        lines_with_ranges.push((line_content, line_ranges));
    }

    let combined: String = lines_with_ranges
        .iter()
        .map(|(line, _)| *line)
        .collect::<Vec<_>>()
        .join("\n");

    let mut combined_ranges: Vec<Range<usize>> = Vec::new();
    let mut offset = 0;
    for (line, ranges) in &lines_with_ranges {
        for r in ranges {
            combined_ranges.push((r.start + offset)..(r.end + offset));
        }
        offset += line.len() + 1;
    }

    format_body(&combined, &combined_ranges)
}
