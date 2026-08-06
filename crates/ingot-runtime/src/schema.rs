//! Ingot types to JSON Schema.
//!
//! This is where the type system stops being decorative. `ask<string[]>` becomes
//! a schema the provider constrains its output to, and a response that does not
//! match is an error rather than a best-effort parse.
//!
//! Two rules shape the mapping:
//!
//! * **Prose is not constrained.** `text` and `markdown` ask for writing, so
//!   they get a plain completion. Constraining them to a JSON string would mean
//!   receiving markdown wrapped in JSON escaping — worse than not asking.
//! * **Non-object schemas are wrapped.** Provider structured-output
//!   implementations generally expect an object at the schema root, so a scalar
//!   or array schema is nested under `value` and unwrapped on the way back.

use std::collections::BTreeMap;

use ingot_ir::RecordType;
use serde_json::{json, Value};

/// How a declared response type is requested from a provider.
#[derive(Debug, Clone, PartialEq)]
pub enum ResponseShape {
    /// Take the completion text verbatim. Used for `text` and `markdown`.
    Prose,
    /// Parse the completion as JSON without constraining it. Used for `json`,
    /// which by definition has no fixed shape to constrain to.
    FreeJson,
    /// Constrain the completion to a schema.
    Schema {
        /// The schema sent to the provider. Always an object at the root.
        schema: Value,
        /// True when the real type was nested under `value` and must be
        /// unwrapped from the response.
        wrapped: bool,
    },
}

/// A response type that cannot be requested from a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedResponseType {
    pub ty: String,
    pub reason: &'static str,
}

/// Work out how to request `ty`, resolving record types against `types`.
pub fn response_shape(
    ty: &str,
    types: &BTreeMap<String, RecordType>,
) -> Result<ResponseShape, UnsupportedResponseType> {
    match ty {
        "text" | "markdown" => return Ok(ResponseShape::Prose),
        "json" => return Ok(ResponseShape::FreeJson),
        "bytes" | "file" => {
            return Err(UnsupportedResponseType {
                ty: ty.to_string(),
                reason: "a model cannot produce binary content directly; \
                         use a tool that writes the file and return its handle",
            })
        }
        _ => {}
    }

    let schema = type_schema(ty, types)?;
    let is_object = schema.get("type").and_then(Value::as_str) == Some("object");
    if is_object {
        Ok(ResponseShape::Schema {
            schema,
            wrapped: false,
        })
    } else {
        Ok(ResponseShape::Schema {
            schema: json!({
                "type": "object",
                "properties": { "value": schema },
                "required": ["value"],
                "additionalProperties": false,
            }),
            wrapped: true,
        })
    }
}

/// The JSON Schema for one Ingot type.
pub fn type_schema(
    ty: &str,
    types: &BTreeMap<String, RecordType>,
) -> Result<Value, UnsupportedResponseType> {
    if let Some(element) = ty.strip_suffix("[]") {
        return Ok(json!({ "type": "array", "items": type_schema(element, types)? }));
    }

    let scalar = match ty {
        "string" | "text" | "markdown" => Some(json!({ "type": "string" })),
        "int" => Some(json!({ "type": "integer" })),
        "float" => Some(json!({ "type": "number" })),
        "bool" => Some(json!({ "type": "boolean" })),
        // A schema for `json` would have to permit anything, which is the same
        // as not constraining at all.
        "json" => Some(json!({})),
        _ => None,
    };
    if let Some(scalar) = scalar {
        return Ok(scalar);
    }

    let Some(record) = types.get(ty) else {
        return Err(UnsupportedResponseType {
            ty: ty.to_string(),
            reason: "not a known type; the artifact does not declare this record",
        });
    };

    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for field in &record.fields {
        properties.insert(field.name.clone(), type_schema(&field.ty, types)?);
        required.push(Value::String(field.name.clone()));
    }
    Ok(json!({
        "type": "object",
        "properties": Value::Object(properties),
        "required": Value::Array(required),
        // Required by provider structured-output implementations, and the right
        // default anyway: an extra field is a model mistake, not a bonus.
        "additionalProperties": false,
    }))
}

/// Check a value against an Ingot type. Returns the first mismatch found.
///
/// Deliberately shallow-but-strict rather than a general JSON Schema validator:
/// it only has to cover the types Ingot can express, and it produces messages
/// naming the Ingot type rather than a schema path.
pub fn validate(
    value: &Value,
    ty: &str,
    types: &BTreeMap<String, RecordType>,
) -> Result<(), String> {
    if let Some(element) = ty.strip_suffix("[]") {
        let Some(items) = value.as_array() else {
            return Err(format!("expected `{ty}`, found {}", describe(value)));
        };
        for (index, item) in items.iter().enumerate() {
            validate(item, element, types).map_err(|error| format!("at index {index}: {error}"))?;
        }
        return Ok(());
    }

    let ok = match ty {
        "string" | "text" | "markdown" => value.is_string(),
        "int" => value.is_i64() || value.is_u64(),
        "float" => value.is_number(),
        "bool" => value.is_boolean(),
        "json" => true,
        _ => {
            let Some(record) = types.get(ty) else {
                return Err(format!("unknown type `{ty}`"));
            };
            let Some(object) = value.as_object() else {
                return Err(format!("expected `{ty}`, found {}", describe(value)));
            };
            for field in &record.fields {
                let Some(field_value) = object.get(&field.name) else {
                    return Err(format!("`{ty}` is missing field `{}`", field.name));
                };
                validate(field_value, &field.ty, types)
                    .map_err(|error| format!("in field `{}`: {error}", field.name))?;
            }
            return Ok(());
        }
    };

    if ok {
        Ok(())
    } else {
        Err(format!("expected `{ty}`, found {}", describe(value)))
    }
}

fn describe(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ingot_ir::FieldType;

    fn record_types() -> BTreeMap<String, RecordType> {
        [(
            "search_result".to_string(),
            RecordType {
                fields: vec![
                    FieldType {
                        name: "title".into(),
                        ty: "string".into(),
                    },
                    FieldType {
                        name: "score".into(),
                        ty: "int".into(),
                    },
                ],
            },
        )]
        .into_iter()
        .collect()
    }

    #[test]
    fn prose_types_are_not_constrained() {
        let types = BTreeMap::new();
        assert_eq!(
            response_shape("markdown", &types).unwrap(),
            ResponseShape::Prose
        );
        assert_eq!(
            response_shape("text", &types).unwrap(),
            ResponseShape::Prose
        );
    }

    #[test]
    fn scalars_and_lists_are_wrapped() {
        let types = BTreeMap::new();
        let ResponseShape::Schema { schema, wrapped } = response_shape("string[]", &types).unwrap()
        else {
            panic!("expected a constrained shape");
        };
        assert!(wrapped);
        assert_eq!(schema["properties"]["value"]["type"], "array");
        assert_eq!(schema["properties"]["value"]["items"]["type"], "string");
    }

    #[test]
    fn records_are_not_wrapped() {
        let types = record_types();
        let ResponseShape::Schema { schema, wrapped } =
            response_shape("search_result", &types).unwrap()
        else {
            panic!("expected a constrained shape");
        };
        assert!(!wrapped, "an object schema is already valid at the root");
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["required"], json!(["title", "score"]));
    }

    #[test]
    fn binary_content_cannot_be_asked_for() {
        let types = BTreeMap::new();
        let error = response_shape("bytes", &types).unwrap_err();
        assert_eq!(error.ty, "bytes");
    }

    #[test]
    fn validation_accepts_matching_values() {
        let types = record_types();
        assert!(validate(&json!("hello"), "string", &types).is_ok());
        assert!(validate(&json!([1, 2]), "int[]", &types).is_ok());
        assert!(validate(&json!({"title": "t", "score": 3}), "search_result", &types).is_ok());
    }

    #[test]
    fn validation_names_the_offending_field() {
        let types = record_types();
        let error = validate(
            &json!({"title": "t", "score": "three"}),
            "search_result",
            &types,
        )
        .unwrap_err();
        assert!(error.contains("score"), "{error}");
        assert!(error.contains("expected `int`"), "{error}");
    }

    #[test]
    fn validation_reports_the_offending_index() {
        let types = BTreeMap::new();
        let error = validate(&json!(["a", 2]), "string[]", &types).unwrap_err();
        assert!(error.contains("at index 1"), "{error}");
    }

    #[test]
    fn a_missing_field_is_reported_by_name() {
        let types = record_types();
        let error = validate(&json!({"title": "t"}), "search_result", &types).unwrap_err();
        assert!(error.contains("missing field `score`"), "{error}");
    }
}
