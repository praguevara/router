use std::borrow::Cow;
use std::sync::Arc;

use graphql_tools::parser::query::Value as QueryValue;
use graphql_tools::static_graphql::schema::{
    Definition, Directive, DirectiveDefinition, Document, EnumValue, Field, InputValue, Type,
    TypeDefinition,
};

use hive_router_query_planner::ast::{
    operation::OperationDefinition,
    selection_item::SelectionItem,
    selection_set::{FieldSelection, SelectionSet},
    value::Value as AstValue,
};
use hive_router_query_planner::state::supergraph_state::OperationKind;

use crate::introspection::schema::SchemaMetadata;
use crate::introspection::semantic::{SearchOptions, SemanticIndex};
use crate::response::value::Value;

pub struct IntrospectionContext {
    pub query: Option<Arc<OperationDefinition>>,
    pub schema: Arc<Document>,
    pub metadata: Arc<SchemaMetadata>,
    /// Index backing the `__search` / `__definitions` semantic-introspection
    /// meta-fields.
    pub index: Arc<SemanticIndex>,
}

fn get_deprecation_reason(directives: &[Directive]) -> Option<&str> {
    directives
        .iter()
        .find(|d| d.name == "deprecated")
        .and_then(|d| {
            d.arguments
                .iter()
                .find(|(name, _)| name.as_str() == "reason")
        })
        .and_then(|(_, value)| {
            if let QueryValue::String(s) = value {
                Some(s.as_str())
            } else {
                None
            }
        })
}

fn is_deprecated(directives: &[Directive]) -> bool {
    directives.iter().any(|d| d.name == "deprecated")
}

fn is_deprecated_enum(enum_val: &EnumValue) -> bool {
    is_deprecated(&enum_val.directives)
}

fn get_specified_by_url(directives: &[Directive]) -> Option<&str> {
    directives
        .iter()
        .find(|d| d.name == "specifiedBy")
        .and_then(|d| d.arguments.iter().find(|(name, _)| name.as_str() == "url"))
        .and_then(|(_, value)| {
            if let QueryValue::String(s) = value {
                Some(s.as_str())
            } else {
                None
            }
        })
}

fn is_one_of(directives: &[Directive]) -> bool {
    directives.iter().any(|d| d.name == "oneOf")
}

fn kind_to_str<'exec>(type_def: &'exec TypeDefinition) -> Cow<'exec, str> {
    (match type_def {
        TypeDefinition::Scalar(_) => "SCALAR",
        TypeDefinition::Object(_) => "OBJECT",
        TypeDefinition::Interface(_) => "INTERFACE",
        TypeDefinition::Union(_) => "UNION",
        TypeDefinition::Enum(_) => "ENUM",
        TypeDefinition::InputObject(_) => "INPUT_OBJECT",
    })
    .into()
}

fn resolve_input_value<'exec>(
    iv: &'exec InputValue,
    selections: &'exec SelectionSet,
    ctx: &'exec IntrospectionContext,
) -> Value<'exec> {
    let mut iv_data = resolve_input_value_selections(iv, &selections.items, ctx);
    iv_data.sort_by_key(|(k, _)| *k);
    Value::Object(iv_data)
}

fn resolve_input_value_selections<'exec>(
    iv: &'exec InputValue,
    selection_items: &'exec Vec<SelectionItem>,
    ctx: &'exec IntrospectionContext,
) -> Vec<(&'exec str, Value<'exec>)> {
    let mut iv_data: Vec<(&str, Value<'_>)> = Vec::with_capacity(selection_items.len());
    for item in selection_items {
        if let SelectionItem::Field(field) = item {
            let value = match field.name.as_str() {
                "name" => Value::String(iv.name.as_str().into()),
                "description" => iv
                    .description
                    .as_ref()
                    .map_or(Value::Null, |s| Value::String(s.into())),
                "type" => resolve_type(&iv.value_type, &field.selections, ctx),
                "defaultValue" => iv
                    .default_value
                    .as_ref()
                    .map_or_else(|| Value::Null, |ast| Value::String(ast.to_string().into())), // TODO: support default values
                "isDeprecated" => Value::Bool(is_deprecated(&iv.directives)),
                "deprecationReason" => get_deprecation_reason(&iv.directives)
                    .map_or(Value::Null, |s| Value::String(s.into())),
                "__typename" => Value::String("__InputValue".into()),
                _ => Value::Null,
            };
            iv_data.push((field.selection_identifier(), value));
        } else if let SelectionItem::InlineFragment(_) = item {
            let selection_items = item.selections();
            if let Some(selection_items) = selection_items {
                let new_data = resolve_input_value_selections(iv, selection_items, ctx);
                iv_data.extend(new_data);
            }
        }
    }
    iv_data
}

fn resolve_field<'exec>(
    f: &'exec Field,
    selections: &'exec SelectionSet,
    ctx: &'exec IntrospectionContext,
) -> Value<'exec> {
    let mut field_data = resolve_field_selections(f, &selections.items, ctx);
    field_data.sort_by_key(|(k, _)| *k);
    Value::Object(field_data)
}

fn resolve_field_selections<'exec>(
    f: &'exec Field,
    selection_items: &'exec Vec<SelectionItem>,
    ctx: &'exec IntrospectionContext,
) -> Vec<(&'exec str, Value<'exec>)> {
    let mut field_data = Vec::with_capacity(selection_items.len());
    for item in selection_items {
        if let SelectionItem::Field(field) = item {
            let value = match field.name.as_str() {
                "name" => Value::String(f.name.as_str().into()),
                "description" => f
                    .description
                    .as_ref()
                    .map_or(Value::Null, |s| Value::String(s.into())),
                "args" => {
                    let args: Vec<_> = f
                        .arguments
                        .iter()
                        .map(|arg| resolve_input_value(arg, &field.selections, ctx))
                        .collect();
                    Value::Array(args)
                }
                "type" => resolve_type(&f.field_type, &field.selections, ctx),
                "isDeprecated" => Value::Bool(is_deprecated(&f.directives)),
                "deprecationReason" => get_deprecation_reason(&f.directives)
                    .map_or(Value::Null, |s| Value::String(s.into())),
                "__typename" => Value::String("__Field".into()),
                _ => Value::Null,
            };
            field_data.push((field.selection_identifier(), value));
        } else if let SelectionItem::InlineFragment(_) = item {
            let selection_items = item.selections();
            if let Some(selection_items) = selection_items {
                let new_data = resolve_field_selections(f, selection_items, ctx);
                field_data.extend(new_data);
            }
        }
    }
    field_data
}

fn resolve_enum_value<'exec>(
    ev: &'exec EnumValue,
    selections: &'exec SelectionSet,
) -> Value<'exec> {
    let mut ev_data = resolve_enum_value_selections(ev, &selections.items);
    ev_data.sort_by_key(|(k, _)| *k);
    Value::Object(ev_data)
}

fn resolve_enum_value_selections<'exec>(
    ev: &'exec EnumValue,
    selection_items: &'exec Vec<SelectionItem>,
) -> Vec<(&'exec str, Value<'exec>)> {
    let mut ev_data = Vec::with_capacity(selection_items.len());
    for item in selection_items {
        if let SelectionItem::Field(field) = item {
            let value = match field.name.as_str() {
                "name" => Value::String(ev.name.as_str().into()),
                "description" => ev
                    .description
                    .as_ref()
                    .map_or(Value::Null, |s| Value::String(s.into())),
                "isDeprecated" => Value::Bool(is_deprecated_enum(ev)),
                "deprecationReason" => get_deprecation_reason(&ev.directives)
                    .map_or(Value::Null, |s| Value::String(s.into())),
                "__typename" => Value::String("__EnumValue".into()),
                _ => Value::Null,
            };
            ev_data.push((field.selection_identifier(), value));
        } else if let SelectionItem::InlineFragment(_) = item {
            let selection_items = item.selections();
            if let Some(selection_items) = selection_items {
                let new_data = resolve_enum_value_selections(ev, selection_items);
                ev_data.extend(new_data);
            }
        }
    }
    ev_data
}

fn resolve_type_definition<'exec>(
    type_def: &'exec TypeDefinition,
    selections: &'exec SelectionSet,
    ctx: &'exec IntrospectionContext,
) -> Value<'exec> {
    let mut type_data = resolve_type_definition_selections(type_def, &selections.items, ctx);
    type_data.sort_by_key(|(k, _)| *k);
    Value::Object(type_data)
}

fn resolve_type_definition_selections<'exec>(
    type_def: &'exec TypeDefinition,
    selection_items: &'exec Vec<SelectionItem>,
    ctx: &'exec IntrospectionContext,
) -> Vec<(&'exec str, Value<'exec>)> {
    let mut type_data = Vec::with_capacity(selection_items.len());

    for item in selection_items {
        if let SelectionItem::Field(field) = item {
            let value = match field.name.as_str() {
                "kind" => Value::String(kind_to_str(type_def)),
                "name" => match type_def {
                    TypeDefinition::Scalar(s) => Some(&s.name),
                    TypeDefinition::Object(o) => Some(&o.name),
                    TypeDefinition::Interface(i) => Some(&i.name),
                    TypeDefinition::Union(u) => Some(&u.name),
                    TypeDefinition::Enum(e) => Some(&e.name),
                    TypeDefinition::InputObject(io) => Some(&io.name),
                }
                .map(|s| Value::String(s.into()))
                .unwrap_or(Value::Null),
                "description" => match type_def {
                    TypeDefinition::Scalar(s) => s.description.as_ref(),
                    TypeDefinition::Object(o) => o.description.as_ref(),
                    TypeDefinition::Interface(i) => i.description.as_ref(),
                    TypeDefinition::Union(u) => u.description.as_ref(),
                    TypeDefinition::Enum(e) => e.description.as_ref(),
                    TypeDefinition::InputObject(io) => io.description.as_ref(),
                }
                .map_or(Value::Null, |s| Value::String(s.into())),
                "specifiedByURL" => {
                    if let TypeDefinition::Scalar(scalar) = type_def {
                        get_specified_by_url(&scalar.directives)
                            .map_or(Value::Null, |url| Value::String(url.into()))
                    } else {
                        Value::Null
                    }
                }
                "isOneOf" => {
                    if let TypeDefinition::InputObject(type_def) = type_def {
                        Value::Bool(is_one_of(&type_def.directives))
                    } else {
                        Value::Null
                    }
                }
                "fields" => {
                    let fields = match type_def {
                        TypeDefinition::Object(o) => Some(&o.fields),
                        TypeDefinition::Interface(i) => Some(&i.fields),
                        _ => None,
                    };
                    if let Some(fields) = fields {
                        let include_deprecated = field
                            .arguments
                            .as_ref()
                            .and_then(|a| a.get_argument("includeDeprecated"))
                            .and_then(|v| {
                                if let AstValue::Boolean(b) = v {
                                    Some(*b)
                                } else {
                                    None
                                }
                            })
                            .unwrap_or(false);

                        let fields_values: Vec<Value<'exec>> = fields
                            .iter()
                            .filter(|f| {
                                !f.name.starts_with("__")
                                    && (include_deprecated || !is_deprecated(&f.directives))
                            })
                            .map(|f| resolve_field(f, &field.selections, ctx))
                            .collect();
                        Value::Array(fields_values)
                    } else {
                        Value::Null
                    }
                }
                "interfaces" => {
                    if let TypeDefinition::Object(obj) = type_def {
                        let interface_values: Vec<_> = obj
                            .implements_interfaces
                            .iter()
                            .filter_map(|iface_name| ctx.schema.type_by_name(iface_name))
                            .map(|t| resolve_type_definition(t, &field.selections, ctx))
                            .collect();
                        Value::Array(interface_values)
                    } else {
                        Value::Null
                    }
                }
                "possibleTypes" => {
                    if let TypeDefinition::Interface(_) | TypeDefinition::Union(_) = type_def {
                        let possible_types: Vec<Value<'exec>> = ctx
                            .metadata
                            .possible_types
                            .get_possible_types(type_def.name())
                            .into_iter()
                            .filter(|v| v != type_def.name())
                            .filter_map(|name| ctx.schema.type_by_name(name.as_str()))
                            .map(|t| resolve_type_definition(t, &field.selections, ctx))
                            .collect();
                        Value::Array(possible_types)
                    } else {
                        Value::Null
                    }
                }
                "enumValues" => {
                    if let TypeDefinition::Enum(enum_type) = type_def {
                        let include_deprecated = field
                            .arguments
                            .as_ref()
                            .and_then(|a| a.get_argument("includeDeprecated"))
                            .and_then(|v| {
                                if let AstValue::Boolean(b) = v {
                                    Some(*b)
                                } else {
                                    None
                                }
                            })
                            .unwrap_or(false);

                        let enum_values: Vec<_> = enum_type
                            .values
                            .iter()
                            .filter(|v| include_deprecated || !is_deprecated_enum(v))
                            .map(|v| resolve_enum_value(v, &field.selections))
                            .collect();
                        Value::Array(enum_values)
                    } else {
                        Value::Null
                    }
                }
                "inputFields" => match type_def {
                    TypeDefinition::InputObject(io) => {
                        let fields_values: Vec<_> = io
                            .fields
                            .iter()
                            .map(|f| resolve_input_value(f, &field.selections, ctx))
                            .collect();
                        Value::Array(fields_values)
                    }
                    _ => Value::Null,
                },
                "ofType" => Value::Null,
                "__typename" => Value::String("__Type".into()),
                _ => Value::Null,
            };
            type_data.push((field.selection_identifier(), value));
        } else if let SelectionItem::InlineFragment(_) = item {
            let selection_items = item.selections();
            if let Some(selection_items) = selection_items {
                let new_data = resolve_type_definition_selections(type_def, selection_items, ctx);
                type_data.extend(new_data);
            }
        }
    }
    type_data
}
fn resolve_wrapper_type<'exec>(
    kind: &'exec str,
    inner_type: &'exec Type,
    selections: &'exec SelectionSet,
    ctx: &'exec IntrospectionContext,
) -> Value<'exec> {
    let mut type_data = resolve_wrapper_type_selections(kind, inner_type, &selections.items, ctx);
    type_data.sort_by_key(|(k, _)| *k);
    Value::Object(type_data)
}

fn resolve_wrapper_type_selections<'exec>(
    kind: &'exec str,
    inner_type: &'exec Type,
    selection_items: &'exec Vec<SelectionItem>,
    ctx: &'exec IntrospectionContext,
) -> Vec<(&'exec str, Value<'exec>)> {
    let mut type_data = Vec::with_capacity(selection_items.len());
    for item in selection_items {
        if let SelectionItem::Field(field) = item {
            let value = match field.name.as_str() {
                "kind" => Value::String(kind.into()),
                "name" => Value::Null,
                "ofType" => resolve_type(inner_type, &field.selections, ctx),
                "__typename" => Value::String("__Type".into()),
                _ => Value::Null,
            };
            type_data.push((field.selection_identifier(), value));
        } else if let SelectionItem::InlineFragment(_) = item {
            let selection_items = item.selections();
            if let Some(selection_items) = selection_items {
                let new_data =
                    resolve_wrapper_type_selections(kind, inner_type, selection_items, ctx);
                type_data.extend(new_data);
            }
        }
    }
    type_data
}

fn resolve_type<'exec>(
    t: &'exec Type,
    selections: &'exec SelectionSet,
    ctx: &'exec IntrospectionContext,
) -> Value<'exec> {
    match t {
        Type::NamedType(name) => {
            let type_def = ctx.schema.type_by_name(name).unwrap_or_else(|| {
                panic!(
                    "Type '{}' not found in the schema unexpectedly during introspection",
                    name
                );
            });
            resolve_type_definition(type_def, selections, ctx)
        }
        Type::ListType(inner_t) => resolve_wrapper_type("LIST", inner_t, selections, ctx),
        Type::NonNullType(inner_t) => resolve_wrapper_type("NON_NULL", inner_t, selections, ctx),
    }
}

fn resolve_directive<'exec>(
    d: &'exec DirectiveDefinition,
    selections: &'exec SelectionSet,
    ctx: &'exec IntrospectionContext,
) -> Value<'exec> {
    let mut directive_data = resolve_directive_selections(d, &selections.items, ctx);
    directive_data.sort_by_key(|(k, _)| *k);
    Value::Object(directive_data)
}

fn resolve_directive_selections<'exec>(
    d: &'exec DirectiveDefinition,
    selection_items: &'exec Vec<SelectionItem>,
    ctx: &'exec IntrospectionContext,
) -> Vec<(&'exec str, Value<'exec>)> {
    let mut directive_data = Vec::with_capacity(selection_items.len());
    for item in selection_items {
        if let SelectionItem::Field(field) = item {
            let value = match field.name.as_str() {
                "name" => Value::String(d.name.as_str().into()),
                "description" => d
                    .description
                    .as_ref()
                    .map_or(Value::Null, |s| Value::String(s.into())),
                "locations" => {
                    let locs: Vec<_> = d
                        .locations
                        .iter()
                        .map(|l| Value::String(l.as_str().into()))
                        .collect();
                    Value::Array(locs)
                }
                "args" => {
                    let args: Vec<_> = d
                        .arguments
                        .iter()
                        .map(|arg| resolve_input_value(arg, &field.selections, ctx))
                        .collect();
                    Value::Array(args)
                }
                "isRepeatable" => Value::Bool(d.repeatable),
                "__typename" => Value::String("__Directive".into()),
                _ => Value::Null,
            };
            directive_data.push((field.selection_identifier(), value));
        } else if let SelectionItem::InlineFragment(_) = item {
            let selection_items = item.selections();
            if let Some(selection_items) = selection_items {
                let new_data = resolve_directive_selections(d, selection_items, ctx);
                directive_data.extend(new_data);
            }
        }
    }
    directive_data
}

fn resolve_schema_field<'exec>(
    field: &'exec FieldSelection,
    ctx: &'exec IntrospectionContext,
) -> Value<'exec> {
    let mut schema_data = resolve_schema_selections(&field.selections.items, ctx);

    schema_data.sort_by_key(|(k, _)| *k);
    Value::Object(schema_data)
}

fn resolve_schema_selections<'exec>(
    items: &'exec Vec<SelectionItem>,
    ctx: &'exec IntrospectionContext,
) -> Vec<(&'exec str, Value<'exec>)> {
    let mut schema_data = Vec::with_capacity(items.len());

    for item in items {
        if let SelectionItem::Field(inner_field) = item {
            let value = match inner_field.name.as_str() {
                "description" => Value::Null,
                "types" => {
                    let types = ctx
                        .schema
                        .type_map()
                        .values()
                        .map(|t| resolve_type_definition(t, &inner_field.selections, ctx))
                        .collect();
                    Value::Array(types)
                }
                "queryType" => {
                    let query_type = ctx
                        .schema
                        .type_by_name(ctx.schema.query_type_name())
                        .expect("Query type not found");
                    resolve_type_definition(query_type, &inner_field.selections, ctx)
                }
                "mutationType" => ctx
                    .schema
                    .mutation_type_name()
                    .and_then(|name| ctx.schema.type_by_name(name))
                    .map_or(Value::Null, |t| {
                        resolve_type_definition(t, &inner_field.selections, ctx)
                    }),
                "subscriptionType" => ctx
                    .schema
                    .subscription_type_name()
                    .and_then(|name| ctx.schema.type_by_name(name))
                    .map_or(Value::Null, |t| {
                        resolve_type_definition(t, &inner_field.selections, ctx)
                    }),
                "directives" => {
                    let directives = ctx
                        .schema
                        .definitions
                        .iter()
                        .filter_map(|d| match d {
                            Definition::DirectiveDefinition(d) => Some(d),
                            _ => None,
                        })
                        .map(|d| resolve_directive(d, &inner_field.selections, ctx))
                        .collect();
                    Value::Array(directives)
                }
                "__typename" => Value::String("__Schema".into()),
                _ => Value::Null,
            };
            schema_data.push((inner_field.selection_identifier(), value));
        } else if let SelectionItem::FragmentSpread(_) = item {
            let selection_items = item.selections();
            if let Some(selection_items) = selection_items {
                let new_data = resolve_schema_selections(selection_items, ctx);
                schema_data.extend(new_data);
            }
        }
    }
    schema_data
}

// ---------------------------------------------------------------------------
// Semantic introspection: `__search` and `__definitions`.
// See `docs/design/semantic-introspection/main.md`.
// ---------------------------------------------------------------------------

/// A located member of the schema, used to resolve the `__SchemaDefinition`
/// union. Holds borrows into the consumer schema (`'exec`).
#[derive(Clone, Copy)]
enum SchemaMember<'exec> {
    Type(&'exec TypeDefinition),
    Field(&'exec Field),
    InputValue(&'exec InputValue),
    EnumValue(&'exec EnumValue),
    Directive(&'exec DirectiveDefinition),
}

impl SchemaMember<'_> {
    fn typename(&self) -> &'static str {
        match self {
            SchemaMember::Type(_) => "__Type",
            SchemaMember::Field(_) => "__Field",
            SchemaMember::InputValue(_) => "__InputValue",
            SchemaMember::EnumValue(_) => "__EnumValue",
            SchemaMember::Directive(_) => "__Directive",
        }
    }
}

/// Resolves a schema coordinate to the underlying definition. Supported shapes:
/// `Type`, `Type.field` (object/interface field, input field, or enum value),
/// `Type.field.arg` (field argument), and `@directive`.
fn locate_member<'exec>(
    coordinate: &str,
    ctx: &'exec IntrospectionContext,
) -> Option<SchemaMember<'exec>> {
    if let Some(directive_name) = coordinate.strip_prefix('@') {
        return ctx
            .schema
            .directive_by_name(directive_name)
            .map(SchemaMember::Directive);
    }

    let mut parts = coordinate.split('.');
    let type_name = parts.next()?;
    let type_def = ctx.schema.type_by_name(type_name)?;

    match (parts.next(), parts.next(), parts.next()) {
        (None, _, _) => Some(SchemaMember::Type(type_def)),
        (Some(member_name), None, _) => match type_def {
            TypeDefinition::Object(_) | TypeDefinition::Interface(_) => {
                type_def.field_by_name(member_name).map(SchemaMember::Field)
            }
            TypeDefinition::InputObject(_) => type_def
                .input_field_by_name(member_name)
                .map(SchemaMember::InputValue),
            TypeDefinition::Enum(enum_type) => enum_type
                .values
                .iter()
                .find(|v| v.name == member_name)
                .map(SchemaMember::EnumValue),
            _ => None,
        },
        (Some(field_name), Some(arg_name), None) => {
            let field = type_def.field_by_name(field_name)?;
            field
                .arguments
                .iter()
                .find(|a| a.name == arg_name)
                .map(SchemaMember::InputValue)
        }
        // More than three segments is not a valid schema coordinate.
        (Some(_), Some(_), Some(_)) => None,
    }
}

/// Resolves the `__SchemaDefinition` union for a coordinate against the given
/// selection set. Returns `Value::Null` when the coordinate cannot be located,
/// so callers can skip unknown coordinates.
fn resolve_schema_definition<'exec>(
    coordinate: &str,
    selections: &'exec SelectionSet,
    ctx: &'exec IntrospectionContext,
) -> Value<'exec> {
    let Some(member) = locate_member(coordinate, ctx) else {
        return Value::Null;
    };
    let typename = member.typename();

    let mut data: Vec<(&str, Value<'exec>)> = Vec::with_capacity(selections.items.len());
    for item in &selections.items {
        match item {
            // Unions expose no fields directly other than `__typename`.
            SelectionItem::Field(field) if field.name == "__typename" => {
                data.push((field.selection_identifier(), Value::String(typename.into())));
            }
            SelectionItem::Field(_) => {}
            SelectionItem::InlineFragment(frag) if frag.type_condition == typename => {
                let sub = match member {
                    SchemaMember::Type(td) => {
                        resolve_type_definition_selections(td, &frag.selections.items, ctx)
                    }
                    SchemaMember::Field(fd) => {
                        resolve_field_selections(fd, &frag.selections.items, ctx)
                    }
                    SchemaMember::InputValue(iv) => {
                        resolve_input_value_selections(iv, &frag.selections.items, ctx)
                    }
                    SchemaMember::EnumValue(ev) => {
                        resolve_enum_value_selections(ev, &frag.selections.items)
                    }
                    SchemaMember::Directive(d) => {
                        resolve_directive_selections(d, &frag.selections.items, ctx)
                    }
                };
                data.extend(sub);
            }
            SelectionItem::InlineFragment(_) | SelectionItem::FragmentSpread(_) => {}
        }
    }
    data.sort_by_key(|(k, _)| *k);
    Value::Object(data)
}

fn encode_cursor(rank: usize) -> String {
    // Opaque pagination token; carries the next offset to resume from.
    (rank + 1).to_string()
}

fn decode_cursor(cursor: &str) -> Option<usize> {
    cursor.parse::<usize>().ok()
}

fn resolve_search<'exec>(
    field: &'exec FieldSelection,
    ctx: &'exec IntrospectionContext,
) -> Value<'exec> {
    let args = field.arguments.as_ref();
    let vars = ctx.variables.variables_map.as_ref();

    let query = args
        .and_then(|a| a.get_argument("query"))
        .and_then(|v| arg_as_str(v, vars))
        .unwrap_or("");
    if query.trim().is_empty() {
        return Value::Array(Vec::new());
    }

    let first = args
        .and_then(|a| a.get_argument("first"))
        .and_then(|v| arg_as_i64(v, vars))
        .map(|i| i.max(0) as usize)
        .unwrap_or(10);

    let after = args
        .and_then(|a| a.get_argument("after"))
        .and_then(|v| arg_as_str(v, vars))
        .and_then(decode_cursor);

    let min_score = args
        .and_then(|a| a.get_argument("minScore"))
        .and_then(|v| arg_as_f64(v, vars));

    let opts = SearchOptions {
        first,
        after,
        min_score,
    };

    let results = ctx
        .index
        .search(query, &opts)
        .into_iter()
        .map(|hit| {
            resolve_search_result(hit.coordinate, hit.score, hit.rank, &field.selections, ctx)
        })
        .collect();

    Value::Array(results)
}

fn resolve_search_result<'exec>(
    coordinate: &'exec str,
    score: f64,
    rank: usize,
    selections: &'exec SelectionSet,
    ctx: &'exec IntrospectionContext,
) -> Value<'exec> {
    let mut data =
        resolve_search_result_selections(coordinate, score, rank, &selections.items, ctx);
    data.sort_by_key(|(k, _)| *k);
    Value::Object(data)
}

fn resolve_search_result_selections<'exec>(
    coordinate: &'exec str,
    score: f64,
    rank: usize,
    selection_items: &'exec Vec<SelectionItem>,
    ctx: &'exec IntrospectionContext,
) -> Vec<(&'exec str, Value<'exec>)> {
    let mut data = Vec::with_capacity(selection_items.len());
    for item in selection_items {
        if let SelectionItem::Field(field) = item {
            let value = match field.name.as_str() {
                "coordinate" => Value::String(coordinate.into()),
                "score" => Value::F64(score),
                "cursor" => Value::String(encode_cursor(rank).into()),
                "pathsToRoot" => {
                    let paths = ctx
                        .index
                        .paths_to_root(coordinate)
                        .into_iter()
                        .map(|path| {
                            Value::Array(
                                path.into_iter().map(|c| Value::String(c.into())).collect(),
                            )
                        })
                        .collect();
                    Value::Array(paths)
                }
                "definition" => resolve_schema_definition(coordinate, &field.selections, ctx),
                "__typename" => Value::String("__SearchResult".into()),
                _ => Value::Null,
            };
            data.push((field.selection_identifier(), value));
        } else if let SelectionItem::InlineFragment(_) = item {
            if let Some(selection_items) = item.selections() {
                let sub =
                    resolve_search_result_selections(coordinate, score, rank, selection_items, ctx);
                data.extend(sub);
            }
        }
    }
    data
}

fn resolve_definitions<'exec>(
    field: &'exec FieldSelection,
    ctx: &'exec IntrospectionContext,
) -> Value<'exec> {
    let vars = ctx.variables.variables_map.as_ref();
    let coordinates = field
        .arguments
        .as_ref()
        .and_then(|a| a.get_argument("coordinates"))
        .and_then(|v| arg_as_str_list(v, vars));

    let Some(coordinates) = coordinates else {
        return Value::Array(Vec::new());
    };

    let results = coordinates
        .iter()
        // Skip coordinates that don't resolve so the non-null list stays valid.
        .filter_map(
            |&coord| match resolve_schema_definition(coord, &field.selections, ctx) {
                Value::Null => None,
                value => Some(value),
            },
        )
        .collect();

    Value::Array(results)
}

pub fn resolve_introspection<'exec>(
    operation_definition: &'exec OperationDefinition,
    ctx: &'exec IntrospectionContext,
) -> Value<'exec> {
    let root_selection_set = &operation_definition.selection_set;

    let root_type_name = operation_definition
        .operation_kind
        .as_ref()
        .map(|kind| match kind {
            OperationKind::Query => ctx.schema.query_type_name(),
            OperationKind::Mutation => ctx.schema.mutation_type_name().unwrap_or("Mutation"),
            OperationKind::Subscription => ctx
                .schema
                .subscription_type_name()
                .unwrap_or("Subscription"),
        })
        .unwrap_or_else(|| ctx.schema.query_type_name());

    let mut data =
        resolve_root_introspection_selections(root_type_name, &root_selection_set.items, ctx);

    data.sort_by_key(|(k, _)| *k);
    Value::Object(data)
}

fn resolve_root_introspection_selections<'exec>(
    root_type_name: &'exec str,
    items: &'exec Vec<SelectionItem>,
    ctx: &'exec IntrospectionContext,
) -> Vec<(&'exec str, Value<'exec>)> {
    let mut data = Vec::with_capacity(items.len());
    for item in items {
        if let SelectionItem::Field(field) = item {
            let value = match field.name.as_str() {
                "__schema" => resolve_schema_field(field, ctx),
                // Semantic introspection (`docs/design/semantic-introspection/main.md`).
                "__search" => resolve_search(field, ctx),
                "__definitions" => resolve_definitions(field, ctx),
                "__type" => {
                    if let Some(args) = &field.arguments {
                        if let Some(AstValue::String(type_name)) = args.get_argument("name") {
                            ctx.schema.type_by_name(type_name).map_or(Value::Null, |t| {
                                resolve_type_definition(t, &field.selections, ctx)
                            })
                        } else {
                            Value::Null
                        }
                    } else {
                        Value::Null
                    }
                }
                "__typename" => Value::String(root_type_name.into()),
                _ => Value::Null,
            };
            data.push((field.selection_identifier(), value));
        } else if let SelectionItem::InlineFragment(_) = item {
            let selection_items = item.selections();
            if let Some(selection_items) = selection_items {
                let new_data =
                    resolve_root_introspection_selections(root_type_name, selection_items, ctx);
                data.extend(new_data);
            }
        }
    }
    data
}
