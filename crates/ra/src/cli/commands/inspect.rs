//! Implementation of `ra inspect`.

use std::{fs, path::Path, process::ExitCode};

use ra_config::CompiledContextRules;
use ra_document::parse_file;
use ra_highlight::{breadcrumb, dim, header, indent_content, subheader};

use crate::cli::{args::InspectWhat, context::CommandContext};

/// Inspects document parsing or context signals.
pub fn run(ctx: &CommandContext, what: InspectWhat) -> ExitCode {
    match what {
        InspectWhat::Doc {
            file,
            algorithm,
            limit,
        } => cmd_inspect_doc(&file, algorithm, limit),
        InspectWhat::Ctx { file } => cmd_inspect_ctx(ctx, &file),
    }
}

/// Implements `ra inspect doc` - show how ra parses a document.
fn cmd_inspect_doc(
    file: &str,
    algorithm: Option<ra_context::KeywordAlgorithm>,
    limit: Option<usize>,
) -> ExitCode {
    let path = Path::new(file);

    if !path.exists() {
        eprintln!("error: file not found: {file}");
        return ExitCode::FAILURE;
    }

    let file_type = match path.extension().and_then(|e| e.to_str()) {
        Some("md" | "markdown") => "markdown",
        Some("txt") => "text",
        Some(ext) => {
            eprintln!("error: unsupported file type: .{ext}");
            eprintln!("Supported types: .md, .markdown, .txt");
            return ExitCode::FAILURE;
        }
        None => {
            eprintln!("error: file has no extension");
            eprintln!("Supported types: .md, .markdown, .txt");
            return ExitCode::FAILURE;
        }
    };

    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: failed to read file: {e}");
            return ExitCode::FAILURE;
        }
    };

    let result = match parse_file(path, "inspect") {
        Ok(result) => result,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    let doc = &result.document;

    println!(
        "--- {} ---",
        header(&format!("{} ({})", path.display(), file_type))
    );
    println!("{}", breadcrumb(&doc.title));
    if !doc.tags.is_empty() {
        println!("{}", dim(&format!("tags: {}", doc.tags.join(", "))));
    }

    let chunks = doc.extract_chunks();

    let chunk_info = format!(
        "hierarchical chunking -> {} nodes, {} chunks",
        doc.node_count(),
        chunks.len()
    );
    println!("{}", dim(&chunk_info));
    println!();

    let algo = algorithm.unwrap_or(ra_context::KeywordAlgorithm::TextRank);
    let kw_limit = limit.unwrap_or(20);

    println!("--- {} ---", header(&format!("keywords ({})", algo)));

    let extracted = extract_keywords(&content, algo);

    if extracted.is_empty() {
        println!("{}", dim("  (no keywords extracted)"));
    } else {
        let max_score = extracted
            .iter()
            .take(kw_limit)
            .map(|k| k.score)
            .fold(0.0_f32, f32::max);
        let score_width = format!("{:.2}", max_score).len();

        for kw in extracted.iter().take(kw_limit) {
            println!("  {:>width$.2}  {}", kw.score, kw.term, width = score_width);
        }
    }
    println!();

    for chunk in &chunks {
        let chunk_label = if chunk.depth == 0 {
            format!("{} (document)", chunk.id)
        } else {
            format!("{} (depth {})", chunk.id, chunk.depth)
        };
        println!("--- {} ---", header(&chunk_label));
        let bc = chunk.hierarchy.join(" > ");
        println!("{}", breadcrumb(&bc));
        println!("{}", dim(&format!("{} chars", chunk.body.len())));
        println!();

        let preview = chunk_preview(&chunk.body, 200);
        println!("{}", indent_content(&preview));
        println!();
    }

    ExitCode::SUCCESS
}

/// Extracts keywords from text using the specified algorithm.
fn extract_keywords(
    content: &str,
    algorithm: ra_context::KeywordAlgorithm,
) -> Vec<ra_context::ScoredKeyword> {
    use ra_context::{KeywordAlgorithm, RakeExtractor, TextRankExtractor, YakeExtractor};

    match algorithm {
        KeywordAlgorithm::TfIdf => {
            let extractor = RakeExtractor::new();
            extractor.extract(content)
        }
        KeywordAlgorithm::Rake => {
            let extractor = RakeExtractor::new();
            extractor.extract(content)
        }
        KeywordAlgorithm::TextRank => {
            let extractor = TextRankExtractor::new();
            extractor.extract(content)
        }
        KeywordAlgorithm::Yake => {
            let extractor = YakeExtractor::new();
            extractor.extract(content)
        }
    }
}

/// Implements `ra inspect ctx` - show context signals for a file.
fn cmd_inspect_ctx(ctx: &CommandContext, file: &str) -> ExitCode {
    use ra_context::{ContextAnalyzer, is_binary_file};

    let path = Path::new(file);

    if !path.exists() {
        eprintln!("error: file not found: {file}");
        return ExitCode::FAILURE;
    }

    if is_binary_file(path) {
        eprintln!("error: binary file: {file}");
        return ExitCode::FAILURE;
    }

    let rules = match CompiledContextRules::compile(&ctx.config.context) {
        Ok(rules) => rules,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    let analyzer = ContextAnalyzer::new(&ctx.config.context, rules);
    let signals = match analyzer.analyze_file(path) {
        Ok(signals) => signals,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    println!("{}", subheader("Context signals:"));
    println!("   {}", path.display());
    println!();

    println!("{}", subheader("Path terms:"));
    if signals.path_terms.is_empty() {
        println!("   {}", dim("(none)"));
    } else {
        for term in &signals.path_terms {
            println!("   {term}");
        }
    }
    println!();

    println!("{}", subheader("Pattern terms:"));
    if signals.pattern_terms.is_empty() {
        println!("   {}", dim("(none)"));
    } else {
        for term in &signals.pattern_terms {
            println!("   {term}");
        }
    }
    println!();

    println!("{}", subheader("Combined search terms:"));
    let all_terms = signals.all_terms();
    if all_terms.is_empty() {
        println!("   {}", dim("(none)"));
    } else {
        println!("   {}", all_terms.join(", "));
    }

    ExitCode::SUCCESS
}

/// Creates a preview of chunk content, truncating if necessary.
fn chunk_preview(content: &str, max_len: usize) -> String {
    let first_line = content.lines().next().unwrap_or("");
    let preview = if first_line.len() > max_len {
        format!("{}...", &first_line[..max_len])
    } else {
        first_line.to_string()
    };
    preview.replace('\n', " ")
}
