# Semantic Introspection in Hive Router

Semantic Introspection lets a client — typically an LLM agent — discover what a
GraphQL API can do **by intent** instead of downloading and reading the entire
schema. The router exposes two new introspection meta-fields:

* `__search(query: String!, …)` — natural-language search over the schema that
  returns a ranked list of schema coordinates (e.g. `Area.availableTaxis`).
* `__definitions(coordinates: [String!]!)` — direct lookup of the full
  introspection details for a set of coordinates.

The agent does two cheap round-trips — "what can answer this question?" then
"give me the exact shape of these fields" — and builds a precise query, instead
of paying to load the whole schema (or an OpenAPI document) into its context on
every turn.

This follows the GraphQL AI-WG
[Semantic Introspection RFC](https://github.com/graphql/ai-wg/blob/main/rfcs/semantic-introspection.md)
and the design demonstrated in ChilliCream's
[Semantic Introspection](https://chillicream.com/blog/2026/04/22/semantic-introspection/)
post. The RFC leaves the indexing/search strategy implementation-defined; the
reference implementation defaults to BM25, which is what this MVP ships.

## The exposed surface

The following is added to the schema the router exposes (it is injected the same
way `__schema` / `__type` are today):

```graphql
extend type Query {
  __search(
    query: String!
    first: Int! = 10
    after: String
    minScore: Float
  ): [__SearchResult!]!

  __definitions(coordinates: [String!]!): [__SchemaDefinition!]!
}

type __SearchResult {
  "Schema coordinate, e.g. \"Area.availableTaxis\""
  coordinate: String!
  "The matched definition (type, field, argument, enum value, or directive)."
  definition: __SchemaDefinition!
  "Field-coordinate paths from a root type to the match, e.g. [[\"Query.areaByName\", \"Area.availableTaxis\"]]."
  pathsToRoot: [[String!]!]!
  "Relevance score in [0, 1], descending."
  score: Float
  "Opaque pagination cursor."
  cursor: String!
}

union __SchemaDefinition = __Type | __Field | __InputValue | __EnumValue | __Directive
```

Example:

```graphql
{
  __search(query: "available taxis near an area", first: 5) {
    coordinate
    score
    pathsToRoot
    definition {
      __typename
      ... on __Field { name description type { name kind } }
    }
  }
}
```

## Why this fits Hive Router with almost no new pipeline code

The router already resolves introspection **on the router side** (it does not
forward `__schema`/`__type` to subgraphs). Every pipeline stage already treats
any field whose name starts with `__` (except `__typename`) as router-resolved
introspection, so `__search`/`__definitions` flow through the existing machinery
for free:

| Stage | Behavior for `__`-prefixed fields | File |
| --- | --- | --- |
| Validation | `if field.name.starts_with("__") { return; }` — not validated against parent type | `lib/graphql-tools/src/validation/rules/fields_on_correct_type.rs` |
| Normalization / type-expand | `__`-fields **and their whole sub-selection** pass through untouched | `lib/query-planner/src/ast/normalization/pipeline/type_expand.rs` |
| Partition | any `__`-field except `__typename` → `introspection_operation` | `lib/executor/src/introspection/partition.rs` |
| Introspection policy gate | enforced whenever `operation_for_introspection.is_some()` | `bin/router/src/pipeline/mod.rs` |
| Resolve | `match` on `__schema` / `__type` / `__typename` | `lib/executor/src/introspection/resolve.rs` |
| Projection | resolves field/union types from `SchemaMetadata` (built from the consumer schema) | `lib/executor/src/projection/plan.rs`, `lib/executor/src/projection/response.rs` |

The consumer schema is assembled in
`lib/query-planner/src/consumer_schema/mod.rs` by merging
`introspection_schema.graphql` into the supergraph and **extending `Query`** with
the introspection meta-fields. `SchemaMetadata`
(`lib/executor/src/introspection/schema.rs`) is then derived from that merged
document.

Consequently, **adding the new types/fields to `introspection_schema.graphql`**
makes validation, projection, and union `possibleTypes` resolution work
automatically. The genuinely new code is:

1. the `__search` / `__definitions` resolver arms (+ a union dispatcher), and
2. the search backend they read from.

## Architecture

Search is abstracted behind a `SemanticSearchProvider` trait. The default
implementation is `Bm25Provider`; the provider is held in `SupergraphData`
behind an `Arc<dyn SemanticSearchProvider>`, so a downstream plugin can swap in
a custom backend (see "Pluggable search backend" below).

```
supergraph load ─▶ build ConsumerSchema (+ introspection SDL)
                     │
                     ├─▶ SchemaMetadata                  (existing)
                     └─▶ Bm25Provider::build()           (default impl) ── BM25 corpus + PathIndex
                                                            │  stored as Arc<dyn SemanticSearchProvider>
request ─▶ … ─▶ resolve_introspection(ctx).await  where ctx: IntrospectionContext {
                                                  schema, metadata, index   ← Arc<dyn SemanticSearchProvider>
                                                }
                     ├─ "__search"      → index.search().await → [__SearchResult]
                     └─ "__definitions" → resolve coordinates  → [__SchemaDefinition]
```

### The provider

`SemanticSearchProvider` is the central abstraction
(`lib/executor/src/introspection/semantic/mod.rs`):

```rust
#[async_trait::async_trait]
pub trait SemanticSearchProvider: Send + Sync {
    async fn search(&self, query: &str, opts: &SearchOptions) -> Vec<SearchHit>;
    fn paths_to_root(&self, coordinate: &str) -> Vec<Vec<String>>;
}
```

`search` is **async**, so a provider may call out to an external search API or
vector store per query. `SearchHit.coordinate` is an owned `String` (rather than
borrowing from the index), which keeps the async `dyn` boundary clean.

The provider is built once per supergraph load (alongside `SchemaMetadata`) and
held in `SupergraphData.semantic_index: Arc<dyn SemanticSearchProvider>`,
immutable behind the `Arc` for the lifetime of that schema.

### The default backend (`Bm25Provider`)

`Bm25Provider::build(schema, metadata)` constructs the default backend, which
contains:

* **A BM25 corpus.** One document per searchable coordinate. The MVP indexes
  object/interface/union/enum/input/scalar **types** and their **fields**;
  document text is the coordinate name + description (with camelCase/snake_case
  split into tokens, lowercased). Ranking is a small hand-rolled BM25
  (term/document frequencies + average document length) — no heavyweight search
  dependency. The raw BM25 score is normalized into `[0, 1]` so `minScore` is
  meaningful.
* **A `PathIndex`** (`lib/executor/src/introspection/semantic/paths.rs`). A BFS
  from each root type (`Query`, `Mutation`, `Subscription`) over the field graph
  already available in `SchemaMetadata.type_fields` (type → field → output type)
  yields the shortest field-coordinate path to each reachable type. For a field
  coordinate `T.f`, `pathsToRoot = shortestPathTo(T) ++ ["T.f"]`; the MVP returns
  the single shortest path (the field is a list, so 0–1 entries is spec-valid).
  `PathIndex` is a standalone, reusable building block: any custom provider can
  build one and delegate `paths_to_root` to it instead of reimplementing the
  traversal. `Bm25Provider` holds one and delegates to it.

### Resolver

`IntrospectionContext` holds an `index: Arc<dyn SemanticSearchProvider>`, and the
resolve chain (`resolve_introspection` → `resolve_root_introspection_selections`
→ `resolve_search`) is **async** to accommodate `search().await`. Two arms are
added to `resolve_root_introspection_selections`:

* `__search` — read `query`/`first`/`after`/`minScore` from the parsed
  arguments, `await` `index.search`, then for each hit build a `__SearchResult`
  by walking the selection set (`coordinate`, `score`, `cursor`, `pathsToRoot`,
  and `definition`).
* `__definitions` — read `coordinates`, resolve each to a definition.

`definition` is a **union**, so it needs a dedicated dispatcher
(`resolve_schema_definition`): from a coordinate it locates the underlying
definition (`Type`, `Type.field`, `Type.field(arg)`, enum value, or directive),
emits the correct `__typename`, and only recurses into matching inline-fragment
type conditions — reusing the existing `resolve_type_definition` /
`resolve_field` / `resolve_input_value` / `resolve_enum_value` builders. (The
existing builders intentionally walk every inline fragment regardless of type
condition, which is correct for a concrete type but wrong for a union, hence the
dispatcher.)

### Client usage note: aliasing across `__SchemaDefinition` members

Because validation runs against the consumer schema (which knows the union and
its members), a query that selects the same field across members with different
response shapes is correctly rejected. The clearest case is `name`:
`__Type.name` is `String` while `__Field.name` / `__InputValue.name` are
`String!`, so selecting an unaliased `name` under both `... on __Type` and
`... on __Field` is an `OverlappingFieldsCanBeMerged` conflict (differing
nullability). Clients must alias such fields per member, e.g.
`... on __Type { typeName: name }` and `... on __Field { fieldName: name }`.

## Configuration

```yaml
semantic_introspection:
  enabled: true   # default: false (experimental)
```

Semantic introspection is already covered by the existing `introspection` policy
(it is an introspection operation). The extra `enabled` flag lets operators turn
the feature on/off independently; when disabled the resolver returns a clear
GraphQL error. For the MVP the SDL is injected unconditionally (the new types are
`__`-prefixed and hidden from `Query.fields`, exactly like `__Schema`);
conditional injection is a later refinement.

## Implementation phases

Each phase is a self-contained commit (or small group of commits) on the feature
branch.

* **Phase 0 — plumbing, no behavior.** Add the SDL; add an empty default
  provider; thread `Arc<dyn SemanticSearchProvider>` through
  `SupergraphData → IntrospectionContext`; resolver arms return empty arrays. A
  `__search`/`__definitions` query parses → validates → normalizes → partitions →
  projects end-to-end with `[]` and no errors. Proves the wiring before any
  algorithm work.
* **Phase 1a — index.** BM25 corpus + tokenizer + score normalization +
  paths-to-root, with unit tests.
* **Phase 1b — resolvers.** `__search` / `__definitions` arms + union dispatcher,
  with unit tests.
* **Phase 2 — config + e2e.** `semantic_introspection.enabled`, the disabled-path
  error, and e2e tests through the full pipeline; changeset.

## Production-readiness extension points

The integration is structured so the following can be added without reshaping it.

* **Pluggable search backend. ✅ Done.** Search runs behind the
  `SemanticSearchProvider` trait (above); `Bm25Provider` is the default impl and
  `SupergraphData.semantic_index` is `Arc<dyn SemanticSearchProvider>`. A
  downstream plugin supplies a custom backend **without a new hook**: in its
  existing `on_supergraph_reload` **end** hook (the end payload is passed by
  value, so `new_supergraph_data` is mutable) it builds its provider from the
  reachable inputs — `new_supergraph_data.planner.consumer_schema.document` and
  `new_supergraph_data.metadata` — and assigns
  `new_supergraph_data.semantic_index = Arc::new(provider)`. A custom backend can
  reuse `PathIndex` for `paths_to_root` instead of reimplementing the field-graph
  traversal. The whole API (`SemanticSearchProvider`, `Bm25Provider`,
  `PathIndex`, `SearchHit`, `SearchOptions`) is re-exported from the `hive_router`
  crate, so a plugin needs no direct dependency on the executor crate. Because
  the router builds the default `Bm25Provider` before the end hook runs, a
  replacement provider is built-then-discarded — accepted for now; a lazy default
  build is a possible later mitigation.

  A downstream plugin looks like this:

  ```rust
  use std::sync::Arc;
  use hive_router::{
      PathIndex, SearchHit, SearchOptions, SemanticSearchProvider,
      // ... plugin trait + on_supergraph_reload payload types
  };

  #[derive(Debug)]
  struct MyProvider {
      paths: PathIndex,
      // ... e.g. an embedding client, a vector-store handle, a tuned corpus
  }

  #[async_trait::async_trait]
  impl SemanticSearchProvider for MyProvider {
      async fn search(&self, query: &str, opts: &SearchOptions) -> Vec<SearchHit> {
          // e.g. embed `query` and query an external vector store, then map
          // results into owned-coordinate `SearchHit`s with normalized scores.
          todo!()
      }

      fn paths_to_root(&self, coordinate: &str) -> Vec<Vec<String>> {
          self.paths.paths_to_root(coordinate) // reuse the shared building block
      }
  }

  // Inside the plugin's on_supergraph_reload:
  fn on_supergraph_reload<'a>(
      &'a self,
      start: OnSupergraphLoadStartHookPayload,
  ) -> OnSupergraphLoadStartHookResult<'a> {
      start.on_end(|mut payload| {
          let sd = &mut payload.new_supergraph_data;
          let roots = ["Query", "Mutation", "Subscription"];
          let paths = PathIndex::build(&sd.metadata, &roots);
          let provider = MyProvider::new(&sd.planner.consumer_schema.document, paths);
          sd.semantic_index = Arc::new(provider);
          payload.proceed()
      })
  }
  ```

* **Auth-aware results.** Filter results per-request using the existing
  authorization metadata / `@inaccessible`, so search never reveals fields the
  caller cannot use.
* **Richer paths.** Multiple / k-shortest paths, configurable depth.
* **Richer indexing.** Field weights, argument and enum-value documents,
  language/stemming options.
* **Embeddings / vector backend (concrete impl).** The provider trait above is
  the seam; a concrete embedding/vector `SemanticSearchProvider` (query embedding
  + vector-store lookup, async at search time) — bundled, or selectable via
  config — is the follow-up.
* **Observability & caching.** A tracing span + metrics (latency, result counts)
  and a result cache keyed by `(schema_hash, query, args)`.
* **Optional RFC types.** `__Example` / `__Prompt` from the RFC's optional
  section.

## Risks / open questions

* **Argument validation** of the new fields must resolve via the consumer schema
  (where the fields now exist) rather than reject them. `__type(name:)` already
  works with a required argument, which indicates this is fine — verified in
  Phase 0.
* **Schema-surface noise:** `__SearchResult` / `__SchemaDefinition` appear in
  `__schema { types }` for everyone (like `__Schema`). Acceptable for the MVP;
  conditional injection removes it later.
* **Index build cost** on very large supergraphs (built once per load) — measure
  and, if needed, move off the hot load path.
* **Relevance quality** of pure BM25 vs embeddings — mitigated by the provider
  trait above.
