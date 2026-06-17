# Phase 2 — Pluggable / customizable semantic search

> Status: **2a + 2b implemented** (provider trait + generic injection; see "Suggested
> phasing"). 2c (auth-aware filtering) and 2d (a concrete embedding backend) remain future.
> The implemented architecture is documented canonically in `main.md`
> ("Production-readiness extension points → Pluggable search backend"); this doc retains the
> requirements/decision history. Builds on `main.md`. Goal: let a downstream plugin customize how
> the `__search` index is built and queried, behind a general trait, so the default (BM25) is
> unchanged and an embeddings/vector/custom backend can be supplied by a plugin.

## Motivation

The MVP ships one hard-wired backend: `SemanticIndex` (BM25) is built at supergraph load and
read by the resolver. Lexical BM25 has a hard ceiling — it cannot bridge intent synonyms
(`get`/`fetch`/`list` → `query`, `remove` → `delete`), it has no domain vocabulary, and
generic audit fields (`createdAt`/`createdBy`) pollute any "create…" query.

An operator embedding the router may want to **process its own schema** for search — inject
synonyms/keywords, weight or demote fields, and rank with **embeddings or an external vector
service** — *without* leaking any of that into the public schema (`__schema`/`__type`), and
*without* forking the resolver pipeline.

## Seams at the start of this phase (grounded)

> Historical: this table captures the pre-2a starting point (a concrete `SemanticIndex`
> struct, **sync** `search`). 2a/2b have since landed — `SemanticIndex` is now the
> `Bm25Provider` default impl behind the `SemanticSearchProvider` trait, `search` is async, and
> `SupergraphData.semantic_index` is `Arc<dyn SemanticSearchProvider>`. See `main.md` for the
> current shape.

| Concern | Where | Shape |
| --- | --- | --- |
| Index type | `lib/executor/.../semantic/mod.rs:63` | concrete `struct SemanticIndex` (BM25 corpus + paths) |
| Build | `bin/router/src/schema_state.rs:285` | `SemanticIndex::build(&consumer_schema.document, &metadata)` |
| Stored | `lib/executor/.../on_supergraph_load.rs` | `SupergraphData.semantic_index: Arc<SemanticIndex>` |
| Queried | `lib/executor/.../resolve.rs:766` | `ctx.index.search(query, &opts) -> Vec<SearchHit>` (**sync**) |
| Plugin lifecycle hook | `plugin_trait.rs` | `fn on_supergraph_reload(&self, start)` (**sync**); start has `new_ast: Document` (mutable), end has `new_supergraph_data: SupergraphData` (`pub`, mutable) |
| Plugin registration | `bin/router/src/plugins/registry.rs:44` | `PluginRegistry::register::<P: RouterPlugin>()`; per-plugin `type Config` |
| Feature config | `lib/router-config/src/semantic_introspection.rs` | `SemanticIntrospectionConfig { enabled: bool }`, `#[serde(deny_unknown_fields)]` |

Two facts shape the design: (1) the ranking backend has **no abstraction** — it's a struct;
(2) a plugin already runs in this lifecycle and already receives the built `SupergraphData`
in the `on_supergraph_reload` end hook.

## Goals / non-goals

**Goals**
- G1. Default behavior is byte-for-byte unchanged when no plugin customizes (BM25 stays default; zero-config).
- G2. A plugin can **supply a custom ranking backend** (embeddings / external vector service / custom) via a general trait.
- G3. A plugin can **customize the default backend's inputs** (synonyms, weights, demotions, stopwords) without writing a new backend from scratch.
- G4. All customization targets the **search index only** — never mutates the public schema.
- G5. A plugin can **filter / re-rank results per request** (auth-aware), with access to request-scoped authorization.
- G6. The trait/API is **general and upstream-shaped**; a downstream plugin supplies the impl.

**Non-goals (this phase)**
- `__definitions` changes — it resolves from the schema, not the index; untouched.
- Solving config sealing (`deny_unknown_fields`) for downstream config extension — orthogonal.
- Shipping a specific embedding model — Layer 2 defines the seam; a concrete embedding impl is a follow-up.

## Resolved decisions

- **`search` is async.** Confirmed: a backend may call an external embedding API or query a
  vector service per request, so the trait method is `async`. See "Async ripple" below.
- **Injection reuses `on_supergraph_reload`.** No new dedicated hook. The plugin builds/replaces
  the provider in the existing end hook (it already receives `new_supergraph_data`). See
  "Injection mechanism".

## Async ripple (cost of async `search`)

The whole introspection resolve chain is synchronous today: `resolve_introspection`
(`resolve.rs:869`) → `resolve_root_introspection_selections` (`:895`) → `resolve_search`
(`:718`) → `ctx.index.search(...)` (`:766`), no `.await` anywhere. `resolve_introspection`
is called **synchronously** at `execution/plan.rs:384`, which already sits inside an
`async fn` (it `.await`s `execute_query_plan_with_data` a few lines later).

Consequences of making `search` async:
- The call site at `plan.rs:384` gains a `.await` trivially (already async context).
- But the sync chain `resolve_introspection → resolve_root_introspection_selections →
  resolve_search` must all become `async`. `__schema`/`__type` arms gain no benefit but pay
  the async-fn overhead (acceptable). Scope the change to the `__search` arm if a clean split
  is cheap; otherwise the chain goes async wholesale.
- **`SearchHit` ownership.** Today `SearchHit { coordinate: &'idx str }` borrows from the
  index. Returning borrowed-from-`self` data across an `#[async_trait]` `dyn` boundary threads
  awkwardly. Recommendation: make `SearchHit.coordinate` an owned `String` (bounded clone, ≤
  `first` hits) so the async trait stays clean.

## Layered requirements

### Layer 1 — Default-backend customization (corpus & scoring inputs)

- R1.1 Expose a public builder on the default backend so a plugin can construct a customized
  `Bm25Provider`: a synonym/alias map (token → extra tokens), per-coordinate or per-pattern
  **weight multipliers** (boost/demote), an **exclusion** predicate (drop coordinates), and
  **tokenizer options** (extra stopwords, stemming on/off).
- R1.2 These apply to the **indexed tokens**, never to consumer-schema descriptions →
  satisfies G4 (no public leak). Enriching descriptions in the SDL is explicitly rejected
  because it would surface in `__schema`/`__type` for every client.
- R1.3 Determinism: same schema + same customization → same index (cacheable).

### Layer 2 — Pluggable backend (`SemanticSearchProvider` trait)

- R2.1 Introduce a trait (illustrative; `#[async_trait]` since `search` is async and the value
  is held as `dyn`):
  ```rust
  #[async_trait::async_trait]
  pub trait SemanticSearchProvider: Send + Sync + std::fmt::Debug {
      async fn search(&self, query: &str, opts: &SearchOptions) -> Vec<SearchHit>; // owned hits
      fn paths_to_root(&self, coordinate: &str) -> Vec<Vec<String>>;
  }
  ```
- R2.2 `SupergraphData.semantic_index` becomes `Arc<dyn SemanticSearchProvider>`;
  `Bm25Provider` (today's `SemanticIndex`) is the default impl. Resolver call shape unchanged
  apart from `.await`.
- R2.3 A **hybrid** provider (BM25 recall → embedding rerank) and an **external-service**
  provider (embed query + query a vector store, both async at search time) must be expressible.
- R2.4 Build failures (model load, bad config) must degrade gracefully — fall back to BM25 and
  log, never panic on the load path.

### Layer 3 — Per-request result post-processing

- R3.1 A seam to **filter and/or re-rank** the hits per request, with access to request-scoped
  authorization (`SupergraphData.authorization` / `@inaccessible`) so search never reveals
  fields the caller cannot use.
- R3.2 Re-ranking must preserve the `[0,1]` score and the `minScore`/`first`/`after` contract
  (cursor/rank semantics stay coherent after the hook).
- R3.3 Default (no hook) = today's behavior.

## Injection mechanism (reusing `on_supergraph_reload`)

The plugin customizes the index in the **end hook** of `on_supergraph_reload`, which already
receives `new_supergraph_data: SupergraphData` (`pub`, mutable):

1. Build inputs are reachable from `new_supergraph_data`: `metadata` (`Arc<SchemaMetadata>`) and
   `planner.consumer_schema.document` (`Arc<Document>`), both `pub`.
2. The plugin constructs its provider (a customized `Bm25Provider` for Layer 1, or any
   `dyn SemanticSearchProvider` for Layer 2) and assigns
   `new_supergraph_data.semantic_index = Arc::new(provider)`.

Tradeoffs:
- **Build-then-discard.** The default BM25 index is built by the router before the end hook,
  then dropped if the plugin replaces it. Accepted for now; a lazy default build is a possible
  later mitigation.
- **Sync build.** `on_supergraph_reload` (and its `on_end` closure) is synchronous, so provider
  construction is sync. This is usually fine: with async `search` hitting an external service,
  the index itself is light (endpoints + coordinate list), so little heavy work happens at
  build. A genuinely heavy/async build (local model load, bulk pre-embedding) must use blocking
  or the `BackgroundTasksManager` available at plugin init.

For Layer 3, a request-scoped seam is still needed (the provider is per-schema, auth is
per-request) — either pass a request context into `search`, or add a small result hook. Left
open (D2 below).

## Open decisions to confirm

- **D1 — async scope.** Make the whole introspection resolve chain async, or split so only the
  `__search` arm is async? Recommendation: whole chain if the split isn't trivially clean.
- **D2 — Layer 3 surface.** Pass request/auth context into `search`, vs. a dedicated
  result-filter hook. Recommendation: dedicated hook so auth filtering is provider-agnostic.
- **D3 — embedding model provisioning** (when a local embedding impl lands): bundled vs.
  downloaded vs. configurable path. Recommendation: bundled + configurable path; never
  network-at-startup.
- **D4 — build cost/caching** on large supergraphs: cache the built index by
  `(schema_hash, customization_hash)`; consider moving build off the hot load path.

## Suggested phasing

- **2a — provider trait, no behavior change. ✅ Done.** Extracted
  `Bm25Provider: SemanticSearchProvider` from `SemanticIndex`; `search` is async;
  `Arc<dyn …>` in `SupergraphData`; `.await` threaded through the resolve chain; `SearchHit`
  owned. Default identical.
- **2b — generic provider injection (minimal seam). ✅ Done.** Confirmed injection needs **no
  hook change**: a plugin swaps `new_supergraph_data.semantic_index` (a `pub` field) in the
  `on_supergraph_reload` end hook (payload passed by value). Exposed `PathIndex` (build +
  `paths_to_root`) — the one reusable building block a custom API/vector/BM25 provider needs;
  `Bm25Provider` now uses it. Proven by an e2e test that installs a non-BM25 stub provider.
  **Deferred:** BM25 corpus tuning (synonyms/weights/exclude) and corpus-enumeration /
  hit-normalization helpers — a custom provider reads `&Document` + `&SchemaMetadata` directly
  and owns its score/cursor semantics.
- **2c — Layer 3 auth-aware filtering.** Result hook / context + auth plumbing.
- **2d — embedding / external-service provider.** A concrete backend behind the trait;
  provisioning per D3.
