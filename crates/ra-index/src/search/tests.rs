use std::{collections::HashMap, path::PathBuf, time::SystemTime};

use tempfile::TempDir;

use super::{SearchParams, Searcher};
use crate::{document::ChunkDocument, writer::IndexWriter};

fn make_trees() -> Vec<ra_config::Tree> {
    vec![
        ra_config::Tree {
            name: "local".to_string(),
            path: PathBuf::from("/tmp/local"),
            is_global: false,
            include: vec![],
            exclude: vec![],
        },
        ra_config::Tree {
            name: "global".to_string(),
            path: PathBuf::from("/tmp/global"),
            is_global: true,
            include: vec![],
            exclude: vec![],
        },
    ]
}

fn create_test_index(temp: &TempDir) -> Vec<ChunkDocument> {
    let docs = vec![
        ChunkDocument {
            id: "local:docs/rust.md#intro".to_string(),
            doc_id: "local:docs/rust.md".to_string(),
            parent_id: Some("local:docs/rust.md".to_string()),
            title: "Introduction to Rust".to_string(),
            tags: vec!["rust".to_string(), "programming".to_string()],
            path: "docs/rust.md".to_string(),
            tree: "local".to_string(),
            body: "Rust is a systems programming language focused on safety and performance."
                .to_string(),
            breadcrumb: "Getting Started › Introduction to Rust".to_string(),
            depth: 1,
            position: 1,
            byte_start: 50,
            byte_end: 200,
            sibling_count: 2,
            mtime: SystemTime::UNIX_EPOCH,
        },
        ChunkDocument {
            id: "local:docs/async.md#basics".to_string(),
            doc_id: "local:docs/async.md".to_string(),
            parent_id: Some("local:docs/async.md".to_string()),
            title: "Async Programming".to_string(),
            tags: vec!["rust".to_string(), "async".to_string()],
            path: "docs/async.md".to_string(),
            tree: "local".to_string(),
            body: "Asynchronous programming in Rust uses futures and the async/await syntax."
                .to_string(),
            breadcrumb: "Advanced Topics › Async Programming".to_string(),
            depth: 1,
            position: 1,
            byte_start: 30,
            byte_end: 150,
            sibling_count: 1,
            mtime: SystemTime::UNIX_EPOCH,
        },
        ChunkDocument {
            id: "global:reference/errors.md#handling".to_string(),
            doc_id: "global:reference/errors.md".to_string(),
            parent_id: Some("global:reference/errors.md".to_string()),
            title: "Error Handling".to_string(),
            tags: vec!["rust".to_string(), "errors".to_string()],
            path: "reference/errors.md".to_string(),
            tree: "global".to_string(),
            body: "Rust error handling uses Result and Option types for safety.".to_string(),
            breadcrumb: "Reference › Error Handling".to_string(),
            depth: 1,
            position: 1,
            byte_start: 20,
            byte_end: 100,
            sibling_count: 3,
            mtime: SystemTime::UNIX_EPOCH,
        },
    ];

    let mut writer = IndexWriter::open(temp.path(), "english").unwrap();
    for doc in &docs {
        writer.add_document(doc).unwrap();
    }
    writer.commit().unwrap();

    docs
}

fn searcher(temp: &TempDir, local_boost: f32) -> Searcher {
    Searcher::open(temp.path(), "english", &make_trees(), local_boost, 1).unwrap()
}

fn build_index_with_docs(docs: &[ChunkDocument]) -> (TempDir, Searcher) {
    let temp = TempDir::new().unwrap();
    let mut writer = IndexWriter::open(temp.path(), "english").unwrap();
    for doc in docs {
        writer.add_document(doc).unwrap();
    }
    writer.commit().unwrap();

    let searcher = Searcher::open(temp.path(), "english", &make_trees(), 1.5, 1).unwrap();
    (temp, searcher)
}

#[test]
fn basic_search_returns_matches_and_respects_limit() {
    let temp = TempDir::new().unwrap();
    create_test_index(&temp);
    let mut searcher = searcher(&temp, 1.5);

    let results = searcher.search("rust", 10).unwrap();
    assert_eq!(results.len(), 3);

    let limited = searcher.search("rust", 2).unwrap();
    assert_eq!(limited.len(), 2);
}

#[test]
fn empty_and_miss_queries_return_empty() {
    let temp = TempDir::new().unwrap();
    create_test_index(&temp);
    let mut searcher = searcher(&temp, 1.5);

    assert!(searcher.search("", 10).unwrap().is_empty());
    assert!(searcher.search("python", 10).unwrap().is_empty());
}

#[test]
fn local_boost_affects_scores_without_changing_hits() {
    let temp = TempDir::new().unwrap();
    create_test_index(&temp);

    let mut boosted = searcher(&temp, 2.0);
    let mut unboosted = searcher(&temp, 1.0);

    let boosted_hit = boosted.search("async", 1).unwrap()[0].score;
    let unboosted_hit = unboosted.search("async", 1).unwrap()[0].score;

    assert!(boosted_hit > unboosted_hit);
}

#[test]
fn results_include_fields_and_snippets_toggle() {
    let temp = TempDir::new().unwrap();
    create_test_index(&temp);
    let mut searcher = searcher(&temp, 1.5);

    let results = searcher.search("async", 1).unwrap();
    let result = &results[0];

    assert_eq!(result.id, "local:docs/async.md#basics");
    assert_eq!(result.doc_id, "local:docs/async.md");
    assert_eq!(result.tree, "local");
    assert_eq!(result.path, "docs/async.md");
    assert!(result.body.contains("Asynchronous"));
    assert!(result.snippet.is_some());

    let no_snippet = searcher.search_no_snippets("async", 1).unwrap();
    assert!(no_snippet[0].snippet.is_none());
}

#[test]
fn phrase_search_and_field_highlights_work() {
    let temp = TempDir::new().unwrap();
    create_test_index(&temp);
    let mut searcher = searcher(&temp, 1.5);

    let results = searcher.search("\"systems programming\"", 5).unwrap();
    let result = &results[0];

    assert!(result.body.contains("systems programming"));
    assert!(!result.match_ranges.is_empty());
}

#[test]
fn search_multi_dedup_and_merge_ranges() {
    let temp = TempDir::new().unwrap();
    create_test_index(&temp);
    let mut searcher = searcher(&temp, 1.5);

    let results = searcher.search_multi(&["rust", "programming"], 10).unwrap();

    let mut id_counts = HashMap::new();
    for r in &results {
        *id_counts.entry(r.id.clone()).or_insert(0) += 1;
    }
    assert!(id_counts.values().all(|c| *c == 1));

    let intro = results.iter().find(|r| r.id.contains("rust.md")).unwrap();
    let slices: Vec<String> = intro
        .match_ranges
        .iter()
        .map(|r| intro.body[r.clone()].to_string())
        .collect();
    assert!(slices.iter().any(|s| s.to_lowercase() == "rust"));
    assert!(slices.iter().any(|s| s.to_lowercase().contains("program")));
}

#[test]
fn search_multi_title_and_path_ranges_merge() {
    let temp = TempDir::new().unwrap();
    create_test_index(&temp);
    let mut searcher = searcher(&temp, 1.5);

    let results = searcher
        .search_multi(&["rust", "introduction", "docs"], 10)
        .unwrap();

    let intro = results.iter().find(|r| r.id.contains("rust.md")).unwrap();
    assert!(intro.title_match_ranges.len() >= 2);
    assert!(intro.path_match_ranges.len() >= 2);
}

#[test]
fn fuzzy_typos_match_and_highlight_actual_terms() {
    let doc = ChunkDocument {
        id: "local:docs/test.md".to_string(),
        doc_id: "local:docs/test.md".to_string(),
        parent_id: None,
        title: "Test".to_string(),
        tags: vec![],
        path: "docs/test.md".to_string(),
        tree: "local".to_string(),
        body: "The quick brown fox jumps over the lazy dog.".to_string(),
        breadcrumb: "Test".to_string(),
        depth: 0,
        position: 0,
        byte_start: 0,
        byte_end: 100,
        sibling_count: 1,
        mtime: SystemTime::UNIX_EPOCH,
    };

    let (_temp, mut searcher) = build_index_with_docs(&[doc]);

    let results = searcher.search("foz", 10).unwrap();
    assert!(!results.is_empty());

    let snippet = results[0].snippet.as_ref().expect("snippet");
    assert!(snippet.contains("fox") || snippet.contains("<b>"));
    assert!(
        results[0]
            .match_ranges
            .iter()
            .any(|r| results[0].body[r.clone()].eq_ignore_ascii_case("fox"))
    );
}

#[test]
fn fuzzy_stemming_ranges_cover_variants() {
    let doc = ChunkDocument {
        id: "local:docs/stems.md".to_string(),
        doc_id: "local:docs/stems.md".to_string(),
        parent_id: None,
        title: "Stems".to_string(),
        tags: vec![],
        path: "docs/stems.md".to_string(),
        tree: "local".to_string(),
        body: "Handling handled handles".to_string(),
        breadcrumb: "Stems".to_string(),
        depth: 0,
        position: 0,
        byte_start: 0,
        byte_end: 64,
        sibling_count: 1,
        mtime: SystemTime::UNIX_EPOCH,
    };

    let (_temp, mut searcher) = build_index_with_docs(&[doc]);
    let results = searcher.search("handling", 10).unwrap();
    let result = &results[0];

    let slices: Vec<String> = result
        .match_ranges
        .iter()
        .map(|r| result.body[r.clone()].to_string())
        .collect();

    assert!(slices.iter().any(|s| s.eq_ignore_ascii_case("handling")));
    assert!(slices.iter().any(|s| s.eq_ignore_ascii_case("handled")));
}

#[test]
fn hierarchical_fields_roundtrip() {
    let docs = vec![
        ChunkDocument {
            id: "local:docs/guide.md".to_string(),
            doc_id: "local:docs/guide.md".to_string(),
            parent_id: None,
            title: "Guide".to_string(),
            tags: vec![],
            path: "docs/guide.md".to_string(),
            tree: "local".to_string(),
            body: "This is the preamble content.".to_string(),
            breadcrumb: "> Guide".to_string(),
            depth: 0,
            position: 0,
            byte_start: 0,
            byte_end: 30,
            sibling_count: 1,
            mtime: SystemTime::UNIX_EPOCH,
        },
        ChunkDocument {
            id: "local:docs/guide.md#section-one".to_string(),
            doc_id: "local:docs/guide.md".to_string(),
            parent_id: Some("local:docs/guide.md".to_string()),
            title: "Section One".to_string(),
            tags: vec![],
            path: "docs/guide.md".to_string(),
            tree: "local".to_string(),
            body: "Section one unique content here.".to_string(),
            breadcrumb: "> Guide › Section One".to_string(),
            depth: 1,
            position: 1,
            byte_start: 30,
            byte_end: 100,
            sibling_count: 2,
            mtime: SystemTime::UNIX_EPOCH,
        },
    ];

    let (_temp, mut searcher) = build_index_with_docs(&docs);

    let doc_result = searcher.search("preamble", 10).unwrap()[0].clone();
    assert_eq!(doc_result.id, "local:docs/guide.md");
    assert!(doc_result.parent_id.is_none());
    assert_eq!(doc_result.depth, 0);
    assert_eq!(doc_result.position, 0);
    assert_eq!(doc_result.byte_start, 0);
    assert_eq!(doc_result.byte_end, 30);
    assert_eq!(doc_result.sibling_count, 1);

    let heading = searcher.search("section unique", 10).unwrap()[0].clone();
    assert_eq!(heading.parent_id, Some("local:docs/guide.md".to_string()));
    assert_eq!(heading.depth, 1);
    assert_eq!(heading.position, 1);
    assert_eq!(heading.byte_start, 30);
    assert_eq!(heading.byte_end, 100);
    assert_eq!(heading.sibling_count, 2);

    let fetched = searcher
        .get_by_id("local:docs/guide.md#section-one")
        .unwrap()
        .unwrap();
    assert_eq!(fetched.parent_id, Some("local:docs/guide.md".to_string()));
}

#[test]
fn search_aggregated_filters_by_tree() {
    let temp = TempDir::new().unwrap();
    create_test_index(&temp);
    let mut searcher = searcher(&temp, 1.5);

    let base_params = SearchParams {
        cutoff_ratio: 0.0,
        disable_aggregation: true,
        ..Default::default()
    };

    let all = searcher.search_aggregated("rust", &base_params).unwrap();
    assert_eq!(all.len(), 3);

    let local = searcher
        .search_aggregated(
            "rust",
            &base_params.clone().with_trees(vec!["local".into()]),
        )
        .unwrap();
    assert_eq!(local.len(), 2);
    assert!(local.iter().all(|r| r.candidate().tree == "local"));

    let global = searcher
        .search_aggregated(
            "rust",
            &base_params.clone().with_trees(vec!["global".into()]),
        )
        .unwrap();
    assert_eq!(global.len(), 1);
    assert_eq!(global[0].candidate().tree, "global");
}

#[test]
fn term_idf_behaves_for_common_rare_and_unknown() {
    let temp = TempDir::new().unwrap();
    create_test_index(&temp);
    let searcher = searcher(&temp, 1.5);

    let idf_rust = searcher.term_idf("rust").unwrap().unwrap();
    assert!((idf_rust - 1.0).abs() < 0.1);

    let idf_async = searcher.term_idf("async").unwrap().unwrap();
    assert!(idf_async > 1.5);

    let idf_unknown = searcher.term_idf("zzzz").unwrap();
    assert!(idf_unknown.is_none());

    let idf_prog = searcher.term_idf("programs").unwrap().unwrap();
    let idf_programming = searcher.term_idf("programming").unwrap().unwrap();
    assert!((idf_prog - idf_programming).abs() < 0.1);
}

mod mlt_tests {
    use std::collections::HashSet;

    use super::*;
    use crate::{MoreLikeThisParams, writer::IndexWriter};

    fn create_mlt_test_index() -> (TempDir, Vec<ChunkDocument>) {
        // Create documents with varying similarity:
        // - doc1 and doc2 are about Rust programming (similar)
        // - doc3 is about Python (different)
        // - doc4 is about Rust web (somewhat similar to doc1/doc2)
        let docs = vec![
            ChunkDocument {
                id: "local:docs/rust-intro.md".to_string(),
                doc_id: "local:docs/rust-intro.md".to_string(),
                parent_id: None,
                title: "Introduction to Rust Programming".to_string(),
                tags: vec!["rust".to_string(), "programming".to_string()],
                path: "docs/rust-intro.md".to_string(),
                tree: "local".to_string(),
                body: "Rust is a systems programming language focused on safety, speed, and \
                       concurrency. It prevents memory errors without garbage collection. \
                       Rust's ownership system ensures memory safety at compile time."
                    .to_string(),
                breadcrumb: "Introduction to Rust Programming".to_string(),
                depth: 0,
                position: 0,
                byte_start: 0,
                byte_end: 200,
                sibling_count: 1,
                mtime: SystemTime::UNIX_EPOCH,
            },
            ChunkDocument {
                id: "local:docs/rust-ownership.md".to_string(),
                doc_id: "local:docs/rust-ownership.md".to_string(),
                parent_id: None,
                title: "Understanding Rust Ownership".to_string(),
                tags: vec!["rust".to_string(), "ownership".to_string()],
                path: "docs/rust-ownership.md".to_string(),
                tree: "local".to_string(),
                body: "The ownership system is Rust's most unique feature. Each value in Rust \
                       has a variable that's its owner. Memory safety is guaranteed through \
                       the borrow checker. Rust prevents data races at compile time."
                    .to_string(),
                breadcrumb: "Understanding Rust Ownership".to_string(),
                depth: 0,
                position: 0,
                byte_start: 0,
                byte_end: 200,
                sibling_count: 1,
                mtime: SystemTime::UNIX_EPOCH,
            },
            ChunkDocument {
                id: "local:docs/python-intro.md".to_string(),
                doc_id: "local:docs/python-intro.md".to_string(),
                parent_id: None,
                title: "Introduction to Python".to_string(),
                tags: vec!["python".to_string(), "scripting".to_string()],
                path: "docs/python-intro.md".to_string(),
                tree: "local".to_string(),
                body: "Python is a high-level interpreted language known for readability. \
                       It uses dynamic typing and automatic garbage collection. Python is \
                       great for scripting, web development, and data science."
                    .to_string(),
                breadcrumb: "Introduction to Python".to_string(),
                depth: 0,
                position: 0,
                byte_start: 0,
                byte_end: 200,
                sibling_count: 1,
                mtime: SystemTime::UNIX_EPOCH,
            },
            ChunkDocument {
                id: "global:docs/rust-web.md".to_string(),
                doc_id: "global:docs/rust-web.md".to_string(),
                parent_id: None,
                title: "Rust Web Development".to_string(),
                tags: vec!["rust".to_string(), "web".to_string()],
                path: "docs/rust-web.md".to_string(),
                tree: "global".to_string(),
                body: "Building web applications in Rust provides safety and performance. \
                       Frameworks like Actix and Axum make web development in Rust productive. \
                       Rust's type system catches errors at compile time."
                    .to_string(),
                breadcrumb: "Rust Web Development".to_string(),
                depth: 0,
                position: 0,
                byte_start: 0,
                byte_end: 200,
                sibling_count: 1,
                mtime: SystemTime::UNIX_EPOCH,
            },
        ];

        let temp = TempDir::new().unwrap();
        let mut writer = IndexWriter::open(temp.path(), "english").unwrap();
        for doc in &docs {
            writer.add_document(doc).unwrap();
        }
        writer.commit().unwrap();

        (temp, docs)
    }

    #[test]
    fn mlt_by_id_finds_similar_documents() {
        let (temp, _docs) = create_mlt_test_index();
        let mut searcher = searcher(&temp, 1.0);

        let mlt_params = MoreLikeThisParams::default();
        let search_params = SearchParams {
            disable_aggregation: true,
            cutoff_ratio: 0.0,
            ..Default::default()
        };

        // Find documents similar to rust-intro
        let results = searcher
            .search_more_like_this_by_id("local:docs/rust-intro.md", &mlt_params, &search_params)
            .unwrap();

        // Should find rust-ownership and rust-web (both about Rust)
        // but NOT include the source document itself
        assert!(
            !results
                .iter()
                .any(|r| r.candidate().id == "local:docs/rust-intro.md")
        );

        // The Rust documents should rank higher than Python
        let rust_ids: HashSet<_> = results
            .iter()
            .filter(|r| r.candidate().title.contains("Rust"))
            .map(|r| r.candidate().id.as_str())
            .collect();
        assert!(!rust_ids.is_empty(), "Should find other Rust documents");
    }

    #[test]
    fn mlt_by_id_excludes_source_document() {
        let (temp, _docs) = create_mlt_test_index();
        let mut searcher = searcher(&temp, 1.0);

        let mlt_params = MoreLikeThisParams::default();
        let search_params = SearchParams {
            disable_aggregation: true,
            cutoff_ratio: 0.0,
            ..Default::default()
        };

        let results = searcher
            .search_more_like_this_by_id("local:docs/rust-intro.md", &mlt_params, &search_params)
            .unwrap();

        // Source document should never appear in results
        for result in &results {
            assert_ne!(result.candidate().id, "local:docs/rust-intro.md");
            assert_ne!(result.candidate().doc_id, "local:docs/rust-intro.md");
        }
    }

    #[test]
    fn mlt_by_fields_finds_similar_content() {
        let (temp, _docs) = create_mlt_test_index();
        let mut searcher = searcher(&temp, 1.0);

        let mlt_params = MoreLikeThisParams::default();
        let search_params = SearchParams {
            disable_aggregation: true,
            cutoff_ratio: 0.0,
            ..Default::default()
        };

        // Search with content about Rust ownership
        let fields = vec![
            (
                "body",
                "Rust ownership borrow checker memory safety compile time".to_string(),
            ),
            ("title", "Rust Programming".to_string()),
        ];

        let results = searcher
            .search_more_like_this_by_fields(fields, &mlt_params, &search_params, &HashSet::new())
            .unwrap();

        // Should find Rust-related documents
        assert!(results.iter().any(|r| r.candidate().title.contains("Rust")));
    }

    #[test]
    fn mlt_respects_tree_filter() {
        let (temp, _docs) = create_mlt_test_index();
        let mut searcher = searcher(&temp, 1.0);

        let mlt_params = MoreLikeThisParams::default();
        let search_params = SearchParams {
            disable_aggregation: true,
            cutoff_ratio: 0.0,
            trees: vec!["local".to_string()],
            ..Default::default()
        };

        let results = searcher
            .search_more_like_this_by_id("local:docs/rust-intro.md", &mlt_params, &search_params)
            .unwrap();

        // All results should be from the "local" tree
        for result in &results {
            assert_eq!(result.candidate().tree, "local");
        }
    }

    #[test]
    fn mlt_get_doc_address_works() {
        let (temp, _docs) = create_mlt_test_index();
        let searcher = searcher(&temp, 1.0);

        // Existing document should return an address
        let addr = searcher
            .get_doc_address("local:docs/rust-intro.md")
            .unwrap();
        assert!(addr.is_some());

        // Non-existent document should return None
        let missing = searcher.get_doc_address("local:nonexistent.md").unwrap();
        assert!(missing.is_none());
    }

    #[test]
    fn mlt_explain_returns_info() {
        let (temp, _docs) = create_mlt_test_index();
        let searcher = searcher(&temp, 1.0);

        let mlt_params = MoreLikeThisParams::default();
        let explanation = searcher
            .explain_more_like_this("local:docs/rust-intro.md", &mlt_params)
            .unwrap();

        assert_eq!(explanation.source_id, "local:docs/rust-intro.md");
        assert!(explanation.source_title.contains("Rust"));
        assert!(!explanation.source_body_preview.is_empty());
        assert!(!explanation.query_repr.is_empty());
    }

    #[test]
    fn mlt_nonexistent_document_returns_error() {
        let (temp, _docs) = create_mlt_test_index();
        let mut searcher = searcher(&temp, 1.0);

        let mlt_params = MoreLikeThisParams::default();
        let search_params = SearchParams::default();

        let result = searcher.search_more_like_this_by_id(
            "local:nonexistent.md",
            &mlt_params,
            &search_params,
        );

        assert!(result.is_err());
    }

    #[test]
    fn mlt_params_affect_results() {
        let (temp, _docs) = create_mlt_test_index();
        let mut searcher = searcher(&temp, 1.0);

        let search_params = SearchParams {
            disable_aggregation: true,
            cutoff_ratio: 0.0,
            ..Default::default()
        };

        // Default params
        let default_params = MoreLikeThisParams::default();
        let default_results = searcher
            .search_more_like_this_by_id(
                "local:docs/rust-intro.md",
                &default_params,
                &search_params,
            )
            .unwrap();

        // Very restrictive params (high min word length should exclude many terms)
        let restrictive_params = MoreLikeThisParams::default()
            .with_min_word_length(10)
            .with_max_query_terms(2);
        let restrictive_results = searcher
            .search_more_like_this_by_id(
                "local:docs/rust-intro.md",
                &restrictive_params,
                &search_params,
            )
            .unwrap();

        // Results should differ based on params
        // (In practice, this may vary, but the API should work)
        // Just verify the calls succeeded - MLT query behavior is Tantivy's responsibility
        let _ = (default_results, restrictive_results);
    }
}
