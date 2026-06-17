//! Semantic introspection search.
//!
//! Backs the `__search` / `__definitions` introspection meta-fields described in
//! `docs/design/semantic-introspection/main.md`. Search is abstracted behind the
//! [`SemanticSearchProvider`] trait so the ranking backend is pluggable; the
//! default is [`Bm25Provider`], built once per supergraph load (alongside
//! [`SchemaMetadata`]) from the consumer schema and held behind an
//! `Arc<dyn SemanticSearchProvider>` in `SupergraphData`.
//!
//! `Bm25Provider` scope: a BM25 corpus over named types and object/interface
//! fields, plus precomputed shortest paths-to-root. Argument, input-field and
//! enum-value coordinates, embedding/vector backends, and auth-aware filtering
//! are extension points (see the design doc).

mod bm25;
mod paths;
mod tokenize;

use std::cmp::Ordering;

use graphql_tools::static_graphql::schema::{Definition, Document, Field, TypeDefinition};

use crate::introspection::schema::SchemaMetadata;
use bm25::{Bm25Builder, Bm25Index};
pub use paths::PathIndex;
use tokenize::tokenize;

/// Options for a [`SemanticSearchProvider::search`] call, mapped from the `__search`
/// field arguments.
#[derive(Debug, Clone)]
pub struct SearchOptions {
    /// Maximum number of results to return (`first`).
    pub first: usize,
    /// Global offset to resume from, decoded from the opaque `after` cursor.
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

/// A single ranked search hit.
#[derive(Debug, Clone)]
pub struct SearchHit {
    /// Schema coordinate of the match, e.g. `"Area.availableTaxis"`.
    pub coordinate: String,
    /// Normalized relevance score in `[0, 1]` (the top hit for a query is `1.0`).
    pub score: f64,
    /// Zero-based global rank of this hit; the resolver encodes `rank + 1` as the
    /// opaque `cursor` so the client can resume via `after`.
    pub rank: usize,
}

/// A pluggable semantic-search backend. Held behind an
/// `Arc<dyn SemanticSearchProvider>` in `SupergraphData`; the default
/// implementation is [`Bm25Provider`].
///
/// A plugin can replace the backend in its `on_supergraph_reload` end hook,
/// building from the consumer schema and swapping the index in place. `search`
/// is async, so a provider may call out to an external search API or vector
/// store per query. Use [`PathIndex`] for `paths_to_root` rather than
/// reimplementing the traversal:
///
/// ```ignore
/// fn on_supergraph_reload<'a>(
///     &'a self,
///     start: OnSupergraphLoadStartHookPayload,
/// ) -> OnSupergraphLoadStartHookResult<'a> {
///     start.on_end(|mut payload| {
///         let sd = &mut payload.new_supergraph_data;
///         let paths = PathIndex::build(&sd.metadata, &["Query", "Mutation", "Subscription"]);
///         sd.semantic_index =
///             Arc::new(MyProvider::new(&sd.planner.consumer_schema.document, paths));
///         payload.proceed()
///     })
/// }
/// ```
#[async_trait::async_trait]
pub trait SemanticSearchProvider: Send + Sync {
    /// Returns ranked hits for a natural-language `query`, ordered by score
    /// descending. Scores are normalized so the top hit of a query is `1.0`.
    async fn search(&self, query: &str, opts: &SearchOptions) -> Vec<SearchHit>;

    /// Returns the shortest field-coordinate paths from a root type to
    /// `coordinate`, or an empty list when none exist.
    fn paths_to_root(&self, coordinate: &str) -> Vec<Vec<String>>;
}

/// Default BM25 search backend: a per-schema corpus over named types and
/// object/interface fields, with precomputed shortest paths-to-root.
#[derive(Debug, Default)]
pub struct Bm25Provider {
    /// Document index -> schema coordinate (parallel to the BM25 corpus).
    coordinates: Vec<String>,
    bm25: Bm25Index,
    /// Shortest field-coordinate paths from a root type to each reachable type.
    paths: PathIndex,
}

impl Bm25Provider {
    /// Builds the index from the consumer schema document and its derived
    /// metadata.
    pub fn build(schema: &Document, metadata: &SchemaMetadata) -> Self {
        let mut builder = Bm25Builder::new();
        let mut coordinates: Vec<String> = Vec::new();

        for def in &schema.definitions {
            let Definition::TypeDefinition(type_def) = def else {
                continue;
            };
            let type_name = type_def.name();
            // Skip introspection machinery (`__Schema`, `__SearchResult`, ...).
            if type_name.starts_with("__") {
                continue;
            }

            // Type-level document: name + description, enriched with member names
            // for kinds that have no separate field documents.
            let mut type_tokens = tokenize(type_name);
            if let Some(desc) = type_description(type_def) {
                type_tokens.extend(tokenize(desc));
            }

            match type_def {
                TypeDefinition::Object(o) => {
                    for f in &o.fields {
                        if f.name.starts_with("__") {
                            continue;
                        }
                        builder.add(field_tokens(type_name, f));
                        coordinates.push(format!("{type_name}.{}", f.name));
                    }
                }
                TypeDefinition::Interface(i) => {
                    for f in &i.fields {
                        if f.name.starts_with("__") {
                            continue;
                        }
                        builder.add(field_tokens(type_name, f));
                        coordinates.push(format!("{type_name}.{}", f.name));
                    }
                }
                TypeDefinition::Enum(e) => {
                    for v in &e.values {
                        type_tokens.extend(tokenize(&v.name));
                        if let Some(d) = &v.description {
                            type_tokens.extend(tokenize(d));
                        }
                    }
                }
                TypeDefinition::InputObject(io) => {
                    for f in &io.fields {
                        type_tokens.extend(tokenize(&f.name));
                    }
                }
                TypeDefinition::Union(u) => {
                    for member in &u.types {
                        type_tokens.extend(tokenize(member));
                    }
                }
                TypeDefinition::Scalar(_) => {}
            }

            builder.add(type_tokens);
            coordinates.push(type_name.to_string());
        }

        let mut roots: Vec<&str> = vec![schema.query_type_name()];
        if let Some(m) = schema.mutation_type_name() {
            roots.push(m);
        }
        if let Some(s) = schema.subscription_type_name() {
            roots.push(s);
        }
        let paths = PathIndex::build(metadata, &roots);

        Self {
            coordinates,
            bm25: builder.build(),
            paths,
        }
    }
}

#[async_trait::async_trait]
impl SemanticSearchProvider for Bm25Provider {
    async fn search(&self, query: &str, opts: &SearchOptions) -> Vec<SearchHit> {
        if opts.first == 0 {
            return Vec::new();
        }

        let mut terms = tokenize(query);
        terms.sort();
        terms.dedup();
        if terms.is_empty() {
            return Vec::new();
        }

        let mut scored = self.bm25.score(&terms);
        if scored.is_empty() {
            return Vec::new();
        }

        // Sort by score descending; break ties on coordinate for deterministic,
        // stable pagination.
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(Ordering::Equal)
                .then_with(|| self.coordinates[a.0].cmp(&self.coordinates[b.0]))
        });

        let max = scored[0].1;
        let inv_max = if max > 0.0 { 1.0 / max } else { 0.0 };
        let min_score = opts.min_score.unwrap_or(0.0);
        let start = opts.after.unwrap_or(0);

        let mut hits = Vec::new();
        for (rank, (doc, raw)) in scored.iter().enumerate() {
            let score = raw * inv_max;
            // Sorted descending, so once below the threshold nothing else qualifies.
            if score < min_score {
                break;
            }
            if rank < start {
                continue;
            }
            if hits.len() >= opts.first {
                break;
            }
            hits.push(SearchHit {
                coordinate: self.coordinates[*doc].clone(),
                score,
                rank,
            });
        }
        hits
    }

    fn paths_to_root(&self, coordinate: &str) -> Vec<Vec<String>> {
        self.paths.paths_to_root(coordinate)
    }
}

fn type_description(type_def: &TypeDefinition) -> Option<&str> {
    match type_def {
        TypeDefinition::Scalar(s) => s.description.as_deref(),
        TypeDefinition::Object(o) => o.description.as_deref(),
        TypeDefinition::Interface(i) => i.description.as_deref(),
        TypeDefinition::Union(u) => u.description.as_deref(),
        TypeDefinition::Enum(e) => e.description.as_deref(),
        TypeDefinition::InputObject(io) => io.description.as_deref(),
    }
}

fn field_tokens(parent_type: &str, field: &Field) -> Vec<String> {
    let mut tokens = tokenize(&field.name);
    tokens.extend(tokenize(parent_type));
    if let Some(desc) = &field.description {
        tokens.extend(tokenize(desc));
    }
    for arg in &field.arguments {
        tokens.extend(tokenize(&arg.name));
        if let Some(desc) = &arg.description {
            tokens.extend(tokenize(desc));
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphql_tools::parser::schema::parse_schema;
    use hive_router_query_planner::consumer_schema::ConsumerSchema;

    use crate::introspection::schema::SchemaWithMetadata;

    /// Block on the async provider `search` for synchronous unit tests.
    fn search(index: &Bm25Provider, query: &str, opts: &SearchOptions) -> Vec<SearchHit> {
        futures::executor::block_on(SemanticSearchProvider::search(index, query, opts))
    }

    const SDL: &str = r#"
        type Query {
          "Look up an area by its name."
          areaByName(name: String!): Area
        }

        "A geographic area in the city."
        type Area {
          name: String!
          "Returns the number of available taxis in the area."
          availableTaxis: Int!
          nearestStation: Station
        }

        type Station {
          name: String!
        }
    "#;

    fn build_index() -> (
        Bm25Provider,
        std::sync::Arc<graphql_tools::static_graphql::schema::Document>,
    ) {
        let supergraph = parse_schema(SDL).unwrap();
        // Reuse the production consumer-schema build so the index sees exactly
        // what introspection exposes (including the injected meta-fields).
        let consumer = ConsumerSchema::new_from_supergraph(&supergraph);
        let metadata = consumer.schema_metadata();
        let index = Bm25Provider::build(&consumer.document, &metadata);
        (index, consumer.document.clone())
    }

    #[test]
    fn finds_field_by_description() {
        let (index, _doc) = build_index();
        let hits = search(&index, "available taxis", &SearchOptions::default());
        assert!(!hits.is_empty(), "expected at least one hit");
        assert_eq!(hits[0].coordinate, "Area.availableTaxis");
        assert!(
            (hits[0].score - 1.0).abs() < f64::EPSILON,
            "top hit normalizes to 1.0"
        );
    }

    #[test]
    fn does_not_index_introspection_types() {
        let (index, _doc) = build_index();
        let hits = search(
            &index,
            "search definitions schema",
            &SearchOptions {
                first: 50,
                ..Default::default()
            },
        );
        assert!(
            hits.iter().all(|h| !h.coordinate.starts_with("__")),
            "introspection coordinates must not be searchable"
        );
    }

    #[test]
    fn paths_to_root_match_navigation() {
        let (index, _doc) = build_index();
        assert_eq!(
            index.paths_to_root("Area.availableTaxis"),
            vec![vec![
                "Query.areaByName".to_string(),
                "Area.availableTaxis".to_string()
            ]]
        );
        assert_eq!(
            index.paths_to_root("Query.areaByName"),
            vec![vec!["Query.areaByName".to_string()]]
        );
    }

    #[test]
    fn pagination_with_first_and_after() {
        let (index, _doc) = build_index();
        let page1 = search(
            &index,
            "name",
            &SearchOptions {
                first: 1,
                ..Default::default()
            },
        );
        assert_eq!(page1.len(), 1);
        let next = page1[0].rank + 1;
        let page2 = search(
            &index,
            "name",
            &SearchOptions {
                first: 1,
                after: Some(next),
                ..Default::default()
            },
        );
        if let Some(hit) = page2.first() {
            assert_ne!(hit.coordinate, page1[0].coordinate);
            assert!(hit.rank >= next);
        }
    }

    #[test]
    fn min_score_filters_weak_matches() {
        let (index, _doc) = build_index();
        let all = search(
            &index,
            "name",
            &SearchOptions {
                first: 50,
                ..Default::default()
            },
        );
        let filtered = search(
            &index,
            "name",
            &SearchOptions {
                first: 50,
                min_score: Some(0.99),
                ..Default::default()
            },
        );
        assert!(filtered.len() <= all.len());
        assert!(filtered.iter().all(|h| h.score >= 0.99));
    }
}
