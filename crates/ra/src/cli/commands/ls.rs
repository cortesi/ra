//! Implementation of `ra ls`.

use std::{collections::HashMap, process::ExitCode};

use crate::cli::{
    args::{LsCommand, LsWhat},
    context::CommandContext,
    output::{breadcrumb, dim, header},
};

/// Lists configured trees, documents, or chunks.
pub fn run(ctx: &mut CommandContext, cmd: &LsCommand) -> ExitCode {
    match cmd.what {
        LsWhat::Trees => cmd_ls_trees(ctx, cmd.long),
        LsWhat::Docs => cmd_ls_docs(ctx, cmd.long),
        LsWhat::Chunks => cmd_ls_chunks(ctx, cmd.long),
    }
}

/// Lists all configured trees.
fn cmd_ls_trees(ctx: &CommandContext, long: bool) -> ExitCode {
    let config = &ctx.config;

    if config.trees.is_empty() {
        println!("{}", dim("No trees configured."));
        return ExitCode::SUCCESS;
    }

    for tree in &config.trees {
        let scope = if tree.is_global { "global" } else { "local" };
        println!(
            "{} {} {}",
            header(&tree.name),
            dim(&format!("({scope})")),
            dim(&format!("→ {}", tree.path.display()))
        );

        if long {
            for pattern in &tree.include {
                println!("  {} {}", dim("+"), pattern);
            }
            for pattern in &tree.exclude {
                println!("  {} {}", dim("-"), pattern);
            }
            println!();
        }
    }

    ExitCode::SUCCESS
}

/// Document info collected for listing.
struct DocInfo {
    /// Tree name.
    tree: String,
    /// File path.
    path: String,
    /// Document title.
    title: String,
    /// Number of chunks in this document.
    chunk_count: usize,
    /// Total body size across all chunks.
    total_size: usize,
}

/// Lists all indexed documents.
fn cmd_ls_docs(ctx: &mut CommandContext, long: bool) -> ExitCode {
    let searcher = match ctx.searcher(None, false) {
        Ok(s) => s,
        Err(code) => return code,
    };

    let chunks = match searcher.list_all() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: failed to list chunks: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut docs: Vec<DocInfo> = Vec::new();
    let mut doc_map: HashMap<String, usize> = HashMap::new();

    for chunk in &chunks {
        let doc_key = format!("{}:{}", chunk.tree, chunk.path);
        if let Some(&idx) = doc_map.get(&doc_key) {
            docs[idx].chunk_count += 1;
            docs[idx].total_size += chunk.body.len();
        } else {
            doc_map.insert(doc_key, docs.len());
            docs.push(DocInfo {
                tree: chunk.tree.clone(),
                path: chunk.path.clone(),
                title: chunk.title().to_string(),
                chunk_count: 1,
                total_size: chunk.body.len(),
            });
        }
    }

    if docs.is_empty() {
        println!("{}", dim("No documents indexed."));
        return ExitCode::SUCCESS;
    }

    for doc in &docs {
        println!(
            "{} {} {}",
            header(&format!("{}:{}", doc.tree, doc.path)),
            dim("—"),
            breadcrumb(&doc.title)
        );
        if long {
            println!(
                "  {}",
                dim(&format!(
                    "{} chunks, {} chars",
                    doc.chunk_count, doc.total_size
                ))
            );
            println!();
        }
    }

    ExitCode::SUCCESS
}

/// Lists all indexed chunks.
fn cmd_ls_chunks(ctx: &mut CommandContext, long: bool) -> ExitCode {
    let searcher = match ctx.searcher(None, false) {
        Ok(s) => s,
        Err(code) => return code,
    };

    let chunks = match searcher.list_all() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: failed to list chunks: {e}");
            return ExitCode::FAILURE;
        }
    };

    if chunks.is_empty() {
        println!("{}", dim("No chunks indexed."));
        return ExitCode::SUCCESS;
    }

    for chunk in &chunks {
        println!(
            "{} {} {}",
            header(&chunk.id),
            dim("—"),
            breadcrumb(&chunk.breadcrumb())
        );
        if long {
            println!("  {}", dim(&format!("{} chars", chunk.body.len())));
            println!();
        }
    }

    ExitCode::SUCCESS
}
