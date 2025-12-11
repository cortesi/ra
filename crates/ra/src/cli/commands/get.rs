//! Implementation of `ra get`.

use std::process::ExitCode;

use ra_document::ChunkId;
use ra_index::{SearchCandidate, SearchResult};

use crate::cli::{args::GetCommand, context::CommandContext, output::output_aggregated_results};

/// Retrieves a chunk or document by ID.
pub fn run(ctx: &mut CommandContext, cmd: &GetCommand) -> ExitCode {
    let chunk_id: ChunkId = match cmd.id.parse() {
        Ok(id) => id,
        Err(_) => {
            eprintln!("error: invalid ID format: {}", cmd.id);
            eprintln!("Expected format: tree:path#slug or tree:path");
            return ExitCode::FAILURE;
        }
    };

    let tree = &chunk_id.doc_id.tree;
    let path = &chunk_id.doc_id.path;
    let slug = &chunk_id.slug;

    let searcher = match ctx.searcher(None, false) {
        Ok(s) => s,
        Err(code) => return code,
    };

    let results: Vec<SearchCandidate> = if cmd.full_document || slug.is_none() {
        match searcher.get_by_path(tree, path) {
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
