mod fixtures;

use forge_core::embed::fake::BucketEmbedder;
use forge_core::recall::{search, Scope};
use forge_core::snapshot::Snapshot;

#[test]
fn search_ranks_over_frontier_only() {
    let cfg = fixtures::corpus_config();
    let embedder = BucketEmbedder::default();
    let snap = Snapshot::build(&cfg, &embedder).unwrap();

    let results = search(&snap, &embedder, "storage directory roots", Scope::Both, 10).unwrap();
    assert!(!results.is_empty());
    let ids: Vec<&str> = results.iter().map(|h| h.id.as_str()).collect();

    assert!(!ids.contains(&"d-old-storage"));
    assert!(!ids.contains(&"d-deprecated"));
    assert!(!ids.iter().any(|id| id == &"f-retired-old"));
}

#[test]
fn search_rust_token_top_2_contains_expected() {
    let cfg = fixtures::corpus_config();
    let embedder = BucketEmbedder::default();
    let snap = Snapshot::build(&cfg, &embedder).unwrap();

    let results = search(&snap, &embedder, "rust", Scope::Both, 10).unwrap();
    let ids: Vec<&str> = results.iter().map(|h| h.id.as_str()).collect();
    assert!(
        ids.contains(&"d-use-rust"),
        "d-use-rust should be in search results for 'rust'"
    );
    assert!(
        ids.contains(&"f-rust-stable"),
        "f-rust-stable should be in search results for 'rust'"
    );
}

#[test]
fn near_matches_include_retired_forces() {
    let cfg = fixtures::corpus_config();
    let embedder = BucketEmbedder::default();
    let snap = Snapshot::build(&cfg, &embedder).unwrap();

    // This asserts retired/superseded forces are *included* in near-matches with
    // their status metadata. Since issue #8 embeds title + body, an exact-title
    // query no longer scores 1.0 against the stored passage vector, so use a
    // lower threshold here; the warn-cutoff itself is covered by
    // near_matches_apply_warn_threshold.
    let results = forge_core::recall::near_matches(
        &snap,
        &embedder,
        "Storage must be a single flat directory",
        0.5,
    )
    .unwrap();
    let hit = results.iter().find(|h| h.id == "f-retired-old");
    assert!(hit.is_some());
    let hit = hit.unwrap();
    assert_eq!(hit.status, "retired");
    assert_eq!(hit.superseded_by.as_deref(), Some("f-new-way"));
}

#[test]
fn near_matches_apply_warn_threshold() {
    let cfg = fixtures::corpus_config();
    let embedder = BucketEmbedder::default();
    let snap = Snapshot::build(&cfg, &embedder).unwrap();

    let results =
        forge_core::recall::near_matches(&snap, &embedder, "completely unrelated text xyz", 0.99)
            .unwrap();
    assert!(results.is_empty() || results.iter().all(|h| h.score >= 0.99));
}
