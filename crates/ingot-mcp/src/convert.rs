//! Turning an MCP tool result into a value of the Ingot type the artifact
//! declared.
//!
//! MCP describes a result as content blocks for a model to read, plus an
//! optional `structuredContent` object for a program to read. Ingot declared
//! `-> search_result[]` at compile time and means it, so this module decides
//! which of those two to believe and produces one JSON value. The interpreter
//! then validates that value against the declared type and stops the run if it
//! does not match — this module never widens a type, it only chooses a source.
//!
//! One convention is shared with the model side of the runtime: MCP requires
//! `structuredContent` to be a JSON object, so a tool whose Ingot result type is
//! a scalar or a list returns it nested under `value`, and it is unwrapped here.

use ingot_runtime::ToolError;
use serde_json::Value;

use crate::client::CallOutcome;

/// Ingot types whose JSON form is not an object, and which therefore travel
/// under a `value` key in `structuredContent`.
fn is_unwrapped(ty: &str) -> bool {
    ty.ends_with("[]")
        || matches!(
            ty,
            "string" | "int" | "float" | "bool" | "text" | "markdown" | "bytes"
        )
}

/// Types that are prose: the text blocks are the answer, verbatim.
fn is_prose(ty: &str) -> bool {
    matches!(ty, "text" | "markdown" | "string" | "bytes")
}

/// Convert one tool result to a value of `ty`.
pub fn to_ingot_value(tool: &str, ty: &str, outcome: &CallOutcome) -> Result<Value, ToolError> {
    if outcome.is_error {
        let message = outcome.text();
        let message = if message.trim().is_empty() {
            "the server reported an error but said nothing about it".to_string()
        } else {
            message
        };
        return Err(ToolError::Failed(format!("`{tool}`: {message}")));
    }

    if let Some(structured) = &outcome.structured {
        return from_structured(tool, ty, structured);
    }

    let kinds = outcome.non_text_kinds();
    let has_text = outcome
        .content
        .iter()
        .any(|block| matches!(block, crate::client::ContentBlock::Text(_)));

    if !has_text {
        if !kinds.is_empty() {
            return Err(ToolError::InvalidResult(format!(
                "`{tool}` returned only {} content, which Ingot has no type to carry; \
                 a tool declared `-> {ty}` must return text or `structuredContent`",
                kinds.join(" and ")
            )));
        }
        if is_prose(ty) {
            // A tool that legitimately produced nothing. An empty document is a
            // value; an empty record is not.
            return Ok(Value::String(String::new()));
        }
        return Err(ToolError::InvalidResult(format!(
            "`{tool}` returned no content, but is declared `-> {ty}`"
        )));
    }

    let text = outcome.text();
    if is_prose(ty) {
        return Ok(Value::String(text));
    }

    serde_json::from_str(&text).map_err(|error| {
        ToolError::InvalidResult(format!(
            "`{tool}` is declared `-> {ty}`, so its text result must be JSON, and it is not: \
             {error}\n  the server should return `structuredContent` for a typed result"
        ))
    })
}

fn from_structured(tool: &str, ty: &str, structured: &Value) -> Result<Value, ToolError> {
    if !is_unwrapped(ty) {
        return Ok(structured.clone());
    }

    match structured.get("value") {
        Some(value) => Ok(value.clone()),
        None => Err(ToolError::InvalidResult(format!(
            "`{tool}` is declared `-> {ty}`, which is not an object, so its `structuredContent` \
             must carry it under `value`; the server sent {}",
            describe(structured)
        ))),
    }
}

fn describe(value: &Value) -> String {
    match value {
        Value::Object(map) if map.is_empty() => "an empty object".to_string(),
        Value::Object(map) => {
            let keys: Vec<&str> = map.keys().map(String::as_str).take(5).collect();
            format!("an object with {}", keys.join(", "))
        }
        Value::Array(_) => "an array".to_string(),
        Value::Null => "null".to_string(),
        other => format!("`{other}`"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::ContentBlock;
    use serde_json::json;

    fn text_result(text: &str) -> CallOutcome {
        CallOutcome {
            content: vec![ContentBlock::Text(text.to_string())],
            structured: None,
            is_error: false,
        }
    }

    fn structured_result(value: Value) -> CallOutcome {
        CallOutcome {
            content: vec![],
            structured: Some(value),
            is_error: false,
        }
    }

    #[test]
    fn prose_is_taken_verbatim() {
        let value =
            to_ingot_value("fs.read_file", "text", &text_result("# Title\n{not json}")).unwrap();
        assert_eq!(value, json!("# Title\n{not json}"));
    }

    #[test]
    fn a_typed_result_is_parsed_from_text_when_there_is_no_structured_content() {
        let value = to_ingot_value("web.search", "int", &text_result("42")).unwrap();
        assert_eq!(value, json!(42));
    }

    #[test]
    fn text_that_should_be_json_and_is_not_is_reported_with_the_declared_type() {
        let error =
            to_ingot_value("web.search", "search_result[]", &text_result("sorry!")).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("search_result[]"), "{message}");
        assert!(message.contains("structuredContent"), "{message}");
    }

    #[test]
    fn a_record_takes_structured_content_as_it_stands() {
        let value = to_ingot_value(
            "web.search",
            "search_result",
            &structured_result(json!({"title": "t", "url": "u"})),
        )
        .unwrap();
        assert_eq!(value, json!({"title": "t", "url": "u"}));
    }

    #[test]
    fn a_list_is_unwrapped_from_the_value_key() {
        let value = to_ingot_value(
            "fs.list_dir",
            "string[]",
            &structured_result(json!({"value": ["a", "b"]})),
        )
        .unwrap();
        assert_eq!(value, json!(["a", "b"]));
    }

    #[test]
    fn a_scalar_without_the_value_key_is_a_clear_error_rather_than_a_guess() {
        let error = to_ingot_value(
            "counter.count",
            "int",
            &structured_result(json!({"count": 3})),
        )
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("under `value`"), "{message}");
        assert!(message.contains("count"), "{message}");
    }

    #[test]
    fn a_file_handle_is_object_shaped_and_not_unwrapped() {
        let value = to_ingot_value(
            "fs.write_file",
            "file",
            &structured_result(json!({"path": "out.md", "bytes": 12})),
        )
        .unwrap();
        assert_eq!(value["path"], json!("out.md"));
    }

    #[test]
    fn a_tool_error_becomes_a_tool_failure_carrying_the_message() {
        let outcome = CallOutcome {
            content: vec![ContentBlock::Text("no such file: a.txt".into())],
            structured: None,
            is_error: true,
        };
        let error = to_ingot_value("fs.read_file", "text", &outcome).unwrap_err();
        assert!(matches!(error, ToolError::Failed(_)), "{error}");
        assert!(error.to_string().contains("no such file"), "{error}");
    }

    #[test]
    fn a_silent_tool_error_still_names_the_tool() {
        let outcome = CallOutcome {
            content: vec![],
            structured: None,
            is_error: true,
        };
        let error = to_ingot_value("fs.read_file", "text", &outcome).unwrap_err();
        assert!(error.to_string().contains("fs.read_file"), "{error}");
    }

    #[test]
    fn content_ingot_cannot_carry_is_named_in_the_error() {
        let outcome = CallOutcome {
            content: vec![ContentBlock::Other("image".into())],
            structured: None,
            is_error: false,
        };
        let error = to_ingot_value("screenshot.take", "text", &outcome).unwrap_err();
        assert!(error.to_string().contains("image"), "{error}");
    }

    #[test]
    fn an_empty_prose_result_is_a_value_but_an_empty_record_is_not() {
        let empty = CallOutcome {
            content: vec![],
            structured: None,
            is_error: false,
        };
        assert_eq!(
            to_ingot_value("fs.read_file", "text", &empty).unwrap(),
            json!("")
        );
        assert!(to_ingot_value("web.search", "search_result", &empty).is_err());
    }
}
