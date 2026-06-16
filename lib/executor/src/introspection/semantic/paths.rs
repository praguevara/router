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
    fn path_index_does_not_fabricate_paths_for_unknown_fields() {
        let index = PathIndex::build(&metadata(), &["Query"]);
        // `Area` is reachable, but it has no `doesNotExist` field, so no path is
        // fabricated for it.
        assert!(index.paths_to_root("Area.doesNotExist").is_empty());
        // A field on an unreachable parent type also yields nothing.
        assert!(index.paths_to_root("Unknown.field").is_empty());
    }
}
