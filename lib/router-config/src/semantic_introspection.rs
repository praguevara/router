use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Configuration for semantic introspection — the experimental `__search` and
/// `__definitions` meta-fields that let clients discover the schema by intent.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticIntrospectionConfig {
    /// Enables the `__search` / `__definitions` meta-fields. Disabled by default
    /// while the feature is experimental; requests using them are rejected when
    /// disabled. Regular introspection is governed separately by `introspection`.
    #[serde(default)]
    pub enabled: bool,
}
