//! Precomputed shortest field-coordinate paths from a root type to every
//! reachable type.
//!
//! A breadth-first traversal of the field graph (type -> field -> output type)
//! from the root operation types yields, for each reachable type, the shortest
//! list of field coordinates that navigates to it. `pathsToRoot` for a field
//! `T.f` is then `shortestPathTo(T) ++ ["T.f"]`.

use std::collections::VecDeque;

use ahash::{HashMap, HashSet};

use crate::introspection::schema::SchemaMetadata;

/// Builds `type name -> shortest field-coordinate path from a root type`.
///
/// Root types map to an empty path. Types only reachable as inputs (or only via
/// scalar leaves) are absent from the map.
pub fn build_paths_to_root(
    metadata: &SchemaMetadata,
    roots: &[&str],
) -> HashMap<String, Vec<String>> {
    let mut shortest: HashMap<String, Vec<String>> = HashMap::default();
    let mut queue: VecDeque<String> = VecDeque::new();

    for root in roots {
        if !shortest.contains_key(*root) {
            shortest.insert((*root).to_string(), Vec::new());
            queue.push_back((*root).to_string());
        }
    }

    while let Some(type_name) = queue.pop_front() {
        // Path already discovered for `type_name` (BFS guarantees it is shortest).
        let base = shortest.get(&type_name).cloned().unwrap_or_default();

        let Some(fields) = metadata.get_type_fields(&type_name) else {
            continue;
        };

        // Iterate fields in a stable order: when several equal-length paths reach
        // the same type, the recorded shortest path must be deterministic across
        // builds, but `metadata`'s field map and possible-types set are unordered.
        let mut field_names: Vec<&String> = fields.keys().collect();
        field_names.sort();

        for field_name in field_names {
            if field_name.starts_with("__") {
                continue;
            }
            let info = &fields[field_name];
            let coordinate = format!("{type_name}.{field_name}");
            let output = &info.output_type_name;

            // The field can navigate to its output type, and — when that output
            // is abstract — to any of its possible concrete/member types.
            let mut candidates: Vec<&str> = vec![output.as_str()];
            if metadata.is_interface_type(output) || metadata.is_union_type(output) {
                if let Some(possible) = metadata.get_possible_types(output) {
                    let mut members: Vec<&str> = possible.iter().map(String::as_str).collect();
                    members.sort();
                    candidates.extend(members);
                }
            }

            for next in candidates {
                // Scalars are leaves with no outgoing edges; don't assign them a
                // navigation path.
                if metadata.is_scalar_type(next) {
                    continue;
                }
                if !shortest.contains_key(next) {
                    let mut path = base.clone();
                    path.push(coordinate.clone());
                    shortest.insert(next.to_string(), path);
                    queue.push_back(next.to_string());
                }
            }
        }
    }

    shortest
}

/// A reusable index of shortest field-coordinate paths from the root operation
/// types to each reachable type.
///
/// Custom [`SemanticSearchProvider`](crate::introspection::semantic::SemanticSearchProvider)
/// implementations can build this once (e.g. in `on_supergraph_reload`) and
/// delegate their `paths_to_root` to it instead of reimplementing the traversal.
#[derive(Debug, Default, Clone)]
pub struct PathIndex {
    /// Type name -> shortest field-coordinate path from a root type.
    type_shortest_path: HashMap<String, Vec<String>>,
    /// Field coordinates (`T.f`) that actually exist on a reachable type, so a
    /// field path is only built for a real field rather than fabricated for any
    /// `<reachableType>.<anything>`.
    field_coordinates: HashSet<String>,
}

impl PathIndex {
    /// Builds the index by BFS over the field graph in `metadata`, starting from
    /// `roots` (the schema's root operation type names, e.g.
    /// `Query`/`Mutation`/`Subscription`).
    pub fn build(metadata: &SchemaMetadata, roots: &[&str]) -> Self {
        let type_shortest_path = build_paths_to_root(metadata, roots);

        // Record the field coordinates of every reachable type so `paths_to_root`
        // can distinguish a real field from a bogus one.
        let mut field_coordinates: HashSet<String> = HashSet::default();
        for type_name in type_shortest_path.keys() {
            if let Some(fields) = metadata.get_type_fields(type_name) {
                for field_name in fields.keys() {
                    if field_name.starts_with("__") {
                        continue;
                    }
                    field_coordinates.insert(format!("{type_name}.{field_name}"));
                }
            }
        }

        Self {
            type_shortest_path,
            field_coordinates,
        }
    }

    /// Returns the depth of `coordinate` from the nearest root type: `0` for a
    /// root type, `n` for a type reached by `n` field hops, and `parentDepth + 1`
    /// for a field coordinate. Returns `None` when the coordinate is unreachable
    /// from any root (e.g. an input type, or a non-existent field). Useful for
    /// boosting shallower (closer-to-root) results in ranking.
    pub fn depth(&self, coordinate: &str) -> Option<u16> {
        if let Some((parent, _field)) = coordinate.split_once('.') {
            if !self.field_coordinates.contains(coordinate) {
                return None;
            }
            self.type_shortest_path
                .get(parent)
                .map(|path| path.len() as u16 + 1)
        } else {
            self.type_shortest_path
                .get(coordinate)
                .map(|path| path.len() as u16)
        }
    }

    /// Returns the shortest field-coordinate paths from a root type to
    /// `coordinate`, or an empty list when none exist. For a field coordinate
    /// `T.f` this is `shortestPathTo(T) ++ ["T.f"]`; for a type coordinate it is
    /// the navigation path to the type (empty for root types).
    pub fn paths_to_root(&self, coordinate: &str) -> Vec<Vec<String>> {
        if let Some((parent, _field)) = coordinate.split_once('.') {
            // Field coordinate: path to the parent type, then the field itself —
            // only when the field actually exists on a reachable type.
            if !self.field_coordinates.contains(coordinate) {
                return Vec::new();
            }
            match self.type_shortest_path.get(parent) {
                Some(base) => {
                    let mut path = base.clone();
                    path.push(coordinate.to_string());
                    vec![path]
                }
                None => Vec::new(),
            }
        } else {
            // Type coordinate: the navigation path to the type (empty for roots).
            match self.type_shortest_path.get(coordinate) {
                Some(path) if !path.is_empty() => vec![path.clone()],
                _ => Vec::new(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::introspection::schema::{FieldNullability, FieldTypeInfo, SchemaMetadata};

    fn field(output: &str) -> FieldTypeInfo {
        FieldTypeInfo {
            output_type_name: output.to_string(),
            nullability: FieldNullability::Leaf { non_null: false },
        }
    }

    fn metadata() -> SchemaMetadata {
        let mut m = SchemaMetadata::default();
        m.object_types.insert("Query".into());
        m.object_types.insert("Area".into());
        m.object_types.insert("Station".into());
        m.scalar_types.insert("Int".into());

        let mut query_fields = ahash::HashMap::default();
        query_fields.insert("areaByName".to_string(), field("Area"));
        m.type_fields.insert("Query".into(), query_fields);

        let mut area_fields = ahash::HashMap::default();
        area_fields.insert("availableTaxis".to_string(), field("Int"));
        area_fields.insert("nearestStation".to_string(), field("Station"));
        m.type_fields.insert("Area".into(), area_fields);

        m.type_fields
            .insert("Station".into(), ahash::HashMap::default());
        m
    }

    #[test]
    fn root_type_has_empty_path() {
        let paths = build_paths_to_root(&metadata(), &["Query"]);
        assert_eq!(paths.get("Query"), Some(&Vec::<String>::new()));
    }

    #[test]
    fn nested_type_path_is_shortest_chain() {
        let paths = build_paths_to_root(&metadata(), &["Query"]);
        assert_eq!(
            paths.get("Area"),
            Some(&vec!["Query.areaByName".to_string()])
        );
        assert_eq!(
            paths.get("Station"),
            Some(&vec![
                "Query.areaByName".to_string(),
                "Area.nearestStation".to_string()
            ])
        );
    }

    #[test]
    fn scalars_get_no_path() {
        let paths = build_paths_to_root(&metadata(), &["Query"]);
        assert!(!paths.contains_key("Int"));
    }

    #[test]
    fn path_index_resolves_real_field_coordinate() {
        let index = PathIndex::build(&metadata(), &["Query"]);
        assert_eq!(
            index.paths_to_root("Area.availableTaxis"),
            vec![vec![
                "Query.areaByName".to_string(),
                "Area.availableTaxis".to_string()
            ]]
        );
    }

    #[test]
    fn depth_is_zero_at_root_and_grows_with_distance() {
        let index = PathIndex::build(&metadata(), &["Query"]);
        assert_eq!(index.depth("Query"), Some(0)); // root type
        assert_eq!(index.depth("Query.areaByName"), Some(1)); // field on root
        assert_eq!(index.depth("Area"), Some(1)); // reached in one hop
        assert_eq!(index.depth("Area.availableTaxis"), Some(2)); // field one hop deeper
        assert_eq!(index.depth("Station"), Some(2)); // reached in two hops
        // Unreachable / non-existent coordinates have no depth.
        assert_eq!(index.depth("Unknown"), None);
        assert_eq!(index.depth("Area.doesNotExist"), None);
    }

    #[test]
    fn path_index_does_not_fabricate_paths_for_unknown_fields() {
        let index = PathIndex::build(&metadata(), &["Query"]);
        // `Area` is reachable, but it has no `doesNotExist` field, so no path is
        // fabricated for it.
        assert!(index.paths_to_root("Area.doesNotExist").is_empty());
        // A field on an unreachable parent type also yields nothing.
        assert!(index.paths_to_root("Unknown.field").is_empty());
    }
}
