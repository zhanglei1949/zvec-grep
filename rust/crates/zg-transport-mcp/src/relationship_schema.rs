//! Model-facing relationship filters. The legacy serde decoder stays unchanged.

use serde_json::{Map, Value, json};

use super::{MAX_PATH_CHARS, MAX_PATH_FILTERS};

pub(super) fn canonical(legacy: &Map<String, Value>) -> Map<String, Value> {
    let mut schema = legacy.clone();
    let properties = schema
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .expect("relationship schema properties");
    let globs = json!({
        "description": properties["globs"]["description"],
        "type": "array",
        "maxItems": MAX_PATH_FILTERS,
        "items": {
            "type": "object",
            "additionalProperties": false,
            "required": ["pattern"],
            "properties": {
                "pattern": {"type": "string", "minLength": 1, "maxLength": MAX_PATH_CHARS},
                "caseInsensitive": {"type": "boolean", "default": false}
            }
        }
    });
    // Optionality belongs to the existing required list, not a nullable union.
    // Keep the field description from the same source as the legacy schema.
    properties.insert("globs".to_owned(), globs);
    properties.insert(
        "insensitiveGlobs".to_owned(),
        json!({
            "description": properties["insensitiveGlobs"]["description"],
            "type": "array",
            "maxItems": MAX_PATH_FILTERS,
            "items": {"type": "string", "maxLength": MAX_PATH_CHARS}
        }),
    );
    // Only the two replaced filter fields referenced these definitions.
    schema.remove("$defs");
    schema
}
