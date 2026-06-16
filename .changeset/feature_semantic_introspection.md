---
hive-router: minor
hive-router-query-planner: minor
hive-router-plan-executor: minor
hive-router-config: minor
---

# Semantic introspection (`__search` / `__definitions`)

Hive Router now supports [Semantic Introspection](https://github.com/graphql/ai-wg/blob/main/rfcs/semantic-introspection.md), letting clients — typically LLM agents — discover a schema by intent instead of downloading and reading the whole schema.

Two router-resolved meta-fields are added to the schema:

- `__search(query: String!, first: Int! = 10, after: String, minScore: Float): [__SearchResult!]!` — natural-language search over the schema that returns ranked coordinates (e.g. `Area.availableTaxis`), each with a relevance `score`, a pagination `cursor`, the `pathsToRoot` needed to reach it, and its full `definition`.
- `__definitions(coordinates: [String!]!): [__SchemaDefinition!]!` — direct lookup of the introspection details for a set of coordinates.

Search is backed by an in-memory BM25 index over type and field names, descriptions, and arguments, built from the schema the router exposes. It is disabled by default while experimental and enabled via configuration:

```yaml
semantic_introspection:
  enabled: true
```

When disabled, requests using `__search` / `__definitions` are rejected; regular introspection is governed separately by the `introspection` setting.
