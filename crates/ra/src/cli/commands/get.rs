//! Implementation of `ra get`.

use std::process::ExitCode;

use ra_index::{SearchCandidate, SearchResult};

use crate::cli::{args::GetCommand, context::CommandContext, output::output_aggregated_results};

/// Retrieves a chunk or document by ID.
pub fn run(ctx: &mut CommandContext, cmd: &GetCommand) -> ExitCode {
    let Some((tree, path, slug)) = parse_chunk_id(&cmd.id) else {
        eprintln!("error: invalid ID format: {}", cmd.id);
        eprintln!("Expected format: tree:path#slug or tree:path");
        return ExitCode::FAILURE;
    };

    let searcher = match ctx.searcher(None, false) {
        Ok(s) => s,
        Err(code) => return code,
    };

    let results: Vec<SearchCandidate> = if cmd.full_document || slug.is_none() {
        match searcher.get_by_path(&tree, &path) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: failed to retrieve document: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        match searcher.get_by_id(&cmd.id) {
            Ok(Some(r)) => vec![r],
            Ok(None) => vec![],
            Err(e) => {
                eprintln!("error: failed to retrieve chunk: {e}");
                return ExitCode::FAILURE;
            }
        }
    };

    if results.is_empty() {
        eprintln!("error: not found: {}", cmd.id);
        return ExitCode::FAILURE;
    }

    let aggregated: Vec<SearchResult> = results.into_iter().map(SearchResult::Single).collect();

    output_aggregated_results(
        &aggregated,
        &cmd.id,
        false,
        false,
        cmd.json,
        0,
        searcher,
        None,
    )
}

/// Parses a chunk ID into (tree, path, optional slug).
fn parse_chunk_id(id: &str) -> Option<(String, String, Option<String>)> {
    let colon_pos = id.find(':')?;
    let tree = id[..colon_pos].to_string();
    let rest = &id[colon_pos + 1..];

    if let Some(hash_pos) = rest.find('#') {
        let path = rest[..hash_pos].to_string();
        let slug = rest[hash_pos + 1..].to_string();
        Some((tree, path, Some(slug)))
    } else {
        Some((tree, rest.to_string(), None))
    }
}
