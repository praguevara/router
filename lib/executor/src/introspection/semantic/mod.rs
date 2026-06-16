//! Semantic introspection index.
//!
//! Backs the `__search` / `__definitions` introspection meta-fields described in
//! `docs/design/semantic-introspection/main.md`. The index is built once per
//! supergraph load (alongside [`SchemaMetadata`]) from the consumer schema and
//! held immutably behind an `Arc` in `SupergraphData`.
//!
//! Phase 0 ships the type and wiring with an empty implementation; later phases
//! fill in the BM25 corpus and precomputed paths-to-root.

use graphql_tools::static_graphql::schema::Document;

use crate::introspection::schema::SchemaMetadata;

/// Options for a [`SemanticIndex::search`] call, mapped from the `__search`
/// field arguments.
#[derive(Debug, Clone)]
pub struct SearchOptions {
    /// Maximum number of results to return (`first`).
    pub first: usize,
    /// Offset to resume from, decoded from the opaque `after` cursor.
    pub after: Option<usize>,
    /// Minimum normalized score in `[0, 1]` (`minScore`).
    pub min_score: Option<f64>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            first: 10,
            after: None,
            min_score: None,
        }
    }
}

/// A single ranked search hit. `coordinate` borrows from the index, which lives
/// as long as the surrounding [`crate::introspection::resolve::IntrospectionContext`].
#[derive(Debug, Clone)]
pub struct SearchHit<'idx> {
    /// Schema coordinate of the match, e.g. `"Area.availableTaxis"`.
    pub coordinate: &'idx str,
    /// Normalized relevance score in `[0, 1]`.
    pub score: f64,
    /// Zero-based global rank of this hit; the resolver encodes it as the
    /// opaque `cursor`.
    pub rank: usize,
}

/// An immutable, per-schema semantic search index.
#[derive(Debug, Default)]
pub struct SemanticIndex {
    // Phase 1a populates a BM25 corpus and precomputed paths-to-root here.
}

impl SemanticIndex {
    /// Builds the index from the consumer schema document and its derived
    /// metadata. Cheap no-op until Phase 1a.
    pub fn build(schema: &Document, metadata: &SchemaMetadata) -> Self {
        let _ = (schema, metadata);
        Self::default()
    }

    /// Returns ranked hits for a natural-language `query`. Empty until Phase 1a.
    pub fn search(&self, query: &str, opts: &SearchOptions) -> Vec<SearchHit<'_>> {
        let _ = (query, opts);
        Vec::new()
    }

    /// Returns the precomputed shortest field-coordinate paths from a root type
    /// to `coordinate`, or an empty list when none exist. Empty until Phase 1a.
    pub fn paths_to_root(&self, coordinate: &str) -> Vec<Vec<&str>> {
        let _ = coordinate;
        Vec::new()
    }
}
