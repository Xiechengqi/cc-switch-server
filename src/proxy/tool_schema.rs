use std::collections::{BTreeSet, HashSet};

use serde_json::{json, Map, Value};

pub(crate) fn normalize_function_parameters(parameters: Option<&Value>) -> Value {
    let mut parameters = match parameters {
        Some(Value::Object(object)) => Value::Object(object.clone()),
        _ => json!({"type": "object", "properties": {}}),
    };
    if let Some(object) = parameters.as_object_mut() {
        if object.get("type").and_then(Value::as_str) != Some("object") {
            object.insert("type".to_string(), json!("object"));
        }
    }
    parameters
}

/// Repairs the JSON Schema subset rejected by the official Codex Responses
/// transport without weakening otherwise valid constraints.
pub(crate) fn normalize_codex_function_parameters(parameters: Option<&Value>) -> Value {
    let mut parameters = normalize_function_parameters(parameters);
    normalize_codex_schema_node(&mut parameters);
    if let Some(object) = parameters.as_object_mut() {
        object.insert("type".to_string(), Value::String("object".to_string()));
    }
    parameters
}

pub(crate) fn normalize_codex_tool_schemas(request: &mut Value) {
    if let Some(tools) = request.get_mut("tools").and_then(Value::as_array_mut) {
        for tool in tools {
            normalize_codex_tool_schema(tool);
        }
    }
    if let Some(input) = request.get_mut("input").and_then(Value::as_array_mut) {
        for item in input {
            if !item
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("additional_tools"))
            {
                continue;
            }
            if let Some(tools) = item.get_mut("tools").and_then(Value::as_array_mut) {
                for tool in tools {
                    normalize_codex_tool_schema(tool);
                }
            }
        }
    }
}

fn normalize_codex_tool_schema(tool: &mut Value) {
    let Some(object) = tool.as_object_mut() else {
        return;
    };
    if is_reserved_codex_tool_object(object) {
        return;
    }
    let is_function = object.get("type").and_then(Value::as_str) == Some("function");
    let is_namespace = object.get("type").and_then(Value::as_str) == Some("namespace");
    if is_function {
        if let Some(nested) = object.get_mut("function").and_then(Value::as_object_mut) {
            let normalized = normalize_codex_function_parameters(nested.get("parameters"));
            nested.insert("parameters".to_string(), normalized);
        } else {
            let normalized = normalize_codex_function_parameters(object.get("parameters"));
            object.insert("parameters".to_string(), normalized);
        }
    } else if let Some(parameters) = object.get_mut("parameters") {
        normalize_codex_schema_node(parameters);
    }
    if is_namespace {
        if let Some(children) = object.get_mut("tools").and_then(Value::as_array_mut) {
            for child in children {
                normalize_codex_tool_schema(child);
            }
        }
    }
}

pub(crate) fn is_reserved_codex_tool(tool: &Value) -> bool {
    tool.as_object().is_some_and(is_reserved_codex_tool_object)
}

fn is_reserved_codex_tool_object(object: &Map<String, Value>) -> bool {
    object
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| {
            object
                .get("function")
                .and_then(Value::as_object)
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .is_some_and(|name| name.to_ascii_lowercase().starts_with("collaboration."))
}

const CODEX_SCHEMA_MAX_DEPTH: usize = 64;
const CODEX_SCHEMA_MAX_NODES: usize = 8_192;
const CODEX_SCHEMA_MAX_BYTES: usize = 1_048_576;
const CODEX_CONST_UNION_MIN_BRANCHES: usize = 8;

fn normalize_codex_schema_node(value: &mut Value) {
    if !codex_schema_within_limits(value) {
        return;
    }
    normalize_codex_schema_node_inner(value);
}

fn normalize_codex_schema_node_inner(value: &mut Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    if object.get("type").is_some_and(Value::is_null) {
        object.remove("type");
    }
    if object
        .get("pattern")
        .and_then(Value::as_str)
        .is_some_and(has_incompatible_codex_unicode_escape)
    {
        object.remove("pattern");
    }
    simplify_codex_const_union(object);

    if let Some(children) = object
        .get_mut("patternProperties")
        .and_then(Value::as_object_mut)
    {
        children.retain(|pattern, _| !has_incompatible_codex_unicode_escape(pattern));
        for child in children.values_mut() {
            normalize_codex_schema_node_inner(child);
        }
    }
    for key in [
        "properties",
        "$defs",
        "definitions",
        "dependentSchemas",
        "dependencies",
    ] {
        if let Some(children) = object.get_mut(key).and_then(Value::as_object_mut) {
            for child in children.values_mut() {
                normalize_codex_schema_node_inner(child);
            }
        }
    }
    for key in [
        "items",
        "additionalProperties",
        "not",
        "if",
        "then",
        "else",
        "propertyNames",
        "contains",
        "unevaluatedProperties",
        "unevaluatedItems",
        "additionalItems",
        "contentSchema",
    ] {
        if let Some(child) = object.get_mut(key) {
            if let Some(children) = child.as_array_mut() {
                for child in children {
                    normalize_codex_schema_node_inner(child);
                }
            } else {
                normalize_codex_schema_node_inner(child);
            }
        }
    }
    for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(children) = object.get_mut(key).and_then(Value::as_array_mut) {
            for child in children {
                normalize_codex_schema_node_inner(child);
            }
        }
    }
}

/// Performs a conservative, iterative preflight before the recursive schema
/// walker. Counting the complete value also bounds large defaults and enum
/// payloads even though the mutating pass deliberately does not enter them.
fn codex_schema_within_limits(root: &Value) -> bool {
    let mut stack = vec![(root, 0_usize)];
    let mut nodes = 0_usize;
    let mut bytes = 0_usize;
    while let Some((value, depth)) = stack.pop() {
        if depth > CODEX_SCHEMA_MAX_DEPTH {
            return false;
        }
        nodes = nodes.saturating_add(1);
        if nodes > CODEX_SCHEMA_MAX_NODES {
            return false;
        }
        bytes = bytes.saturating_add(1);
        match value {
            Value::Null | Value::Bool(_) => {}
            Value::Number(number) => bytes = bytes.saturating_add(number.to_string().len()),
            Value::String(value) => {
                // Six is the largest JSON expansion of one input byte (for a
                // control character encoded as `\u00XX`).
                bytes = bytes.saturating_add(value.len().saturating_mul(6));
            }
            Value::Array(values) => {
                if nodes
                    .saturating_add(stack.len())
                    .saturating_add(values.len())
                    > CODEX_SCHEMA_MAX_NODES
                {
                    return false;
                }
                stack.extend(values.iter().map(|value| (value, depth + 1)));
            }
            Value::Object(values) => {
                if nodes
                    .saturating_add(stack.len())
                    .saturating_add(values.len())
                    > CODEX_SCHEMA_MAX_NODES
                {
                    return false;
                }
                for (key, value) in values {
                    bytes = bytes.saturating_add(key.len().saturating_mul(6));
                    stack.push((value, depth + 1));
                }
            }
        }
        if bytes > CODEX_SCHEMA_MAX_BYTES {
            return false;
        }
    }
    true
}

/// Detects active `\p{` / `\P{` escapes after JSON decoding, including a
/// textual `\u005c` spelling of the introducing backslash. Even backslash runs
/// are literals and remain compatible.
fn has_incompatible_codex_unicode_escape(pattern: &str) -> bool {
    let bytes = pattern.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'\\' {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && bytes[index] == b'\\' {
            index += 1;
        }
        if (index - start) % 2 == 0 {
            continue;
        }
        if index + 1 < bytes.len()
            && matches!(bytes[index], b'p' | b'P')
            && bytes[index + 1] == b'{'
        {
            return true;
        }
        if index + 6 < bytes.len()
            && matches!(bytes[index], b'u' | b'U')
            && bytes[index + 1..index + 5].eq_ignore_ascii_case(b"005c")
            && matches!(bytes[index + 5], b'p' | b'P')
            && bytes[index + 6] == b'{'
        {
            return true;
        }
    }
    false
}

fn simplify_codex_const_union(object: &mut Map<String, Value>) {
    let one_of = object.get("oneOf");
    let any_of = object.get("anyOf");
    let (union_key, branches) = match (one_of, any_of) {
        (Some(_), Some(_)) | (None, None) => return,
        (Some(Value::Array(branches)), None) => ("oneOf", branches),
        (None, Some(Value::Array(branches))) => ("anyOf", branches),
        _ => return,
    };
    if branches.len() < CODEX_CONST_UNION_MIN_BRANCHES || object.contains_key("enum") {
        return;
    }

    let mut values = Vec::with_capacity(branches.len());
    let mut seen = HashSet::with_capacity(branches.len());
    let mut kind = None;
    let mut all_numbers_integral = true;
    for branch in branches {
        let Some(branch) = branch.as_object() else {
            return;
        };
        if branch
            .keys()
            .any(|key| !matches!(key.as_str(), "const" | "description" | "title"))
        {
            return;
        }
        let Some(constant) = branch.get("const") else {
            return;
        };
        let Some((constant_kind, canonical, integral)) = codex_const_key(constant) else {
            return;
        };
        if kind.is_some_and(|kind| kind != constant_kind) {
            return;
        }
        kind = Some(constant_kind);
        all_numbers_integral &= integral;
        if !seen.insert(canonical) {
            return;
        }
        values.push(constant.clone());
    }
    let Some(kind) = kind else {
        return;
    };
    if !codex_parent_type_accepts(object.get("type"), kind, all_numbers_integral) {
        return;
    }

    object.remove(union_key);
    object.insert("enum".to_string(), Value::Array(values));
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CodexConstKind {
    Null,
    Boolean,
    Number,
    String,
}

fn codex_const_key(value: &Value) -> Option<(CodexConstKind, String, bool)> {
    match value {
        Value::Null => Some((CodexConstKind::Null, "null".to_string(), false)),
        Value::Bool(value) => Some((CodexConstKind::Boolean, format!("b:{value}"), false)),
        Value::String(value) => Some((CodexConstKind::String, format!("s:{value}"), false)),
        Value::Number(value) => {
            let (canonical, integral) = canonical_codex_number(&value.to_string())?;
            Some((CodexConstKind::Number, format!("n:{canonical}"), integral))
        }
        Value::Array(_) | Value::Object(_) => None,
    }
}

fn canonical_codex_number(raw: &str) -> Option<(String, bool)> {
    let (negative, unsigned) = raw
        .strip_prefix('-')
        .map_or((false, raw), |value| (true, value));
    let (mantissa, exponent) = unsigned
        .split_once(['e', 'E'])
        .map_or(Some((unsigned, 0_i64)), |(mantissa, exponent)| {
            exponent.parse::<i64>().ok().map(|value| (mantissa, value))
        })?;
    let fractional = mantissa
        .split_once('.')
        .map_or(0_usize, |(_, fraction)| fraction.len());
    let mut digits = mantissa.replace('.', "");
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let first_non_zero = digits.bytes().position(|byte| byte != b'0');
    let Some(first_non_zero) = first_non_zero else {
        return Some(("0e0".to_string(), true));
    };
    digits.drain(..first_non_zero);
    let mut power = exponent.checked_sub(i64::try_from(fractional).ok()?)?;
    while digits.ends_with('0') {
        digits.pop();
        power = power.checked_add(1)?;
    }
    let integral = power >= 0;
    let sign = if negative { "-" } else { "" };
    Some((format!("{sign}{digits}e{power}"), integral))
}

fn codex_parent_type_accepts(
    parent_type: Option<&Value>,
    kind: CodexConstKind,
    all_numbers_integral: bool,
) -> bool {
    let accepts = |value: &str| match kind {
        CodexConstKind::Null => value == "null",
        CodexConstKind::Boolean => value == "boolean",
        CodexConstKind::Number => value == "number" || (value == "integer" && all_numbers_integral),
        CodexConstKind::String => value == "string",
    };
    match parent_type {
        None => true,
        Some(Value::String(value)) => accepts(value),
        Some(Value::Array(values)) => {
            values.iter().all(|value| {
                value
                    .as_str()
                    .is_some_and(|value| value == "null" || accepts(value))
            }) && values
                .iter()
                .any(|value| value.as_str().is_some_and(accepts))
        }
        _ => false,
    }
}

/// Normalizes the subset of JSON Schema accepted by Gemini/Code Assist tools.
///
/// Keep this separate from `normalize_function_parameters`: other upstreams
/// accept a wider schema dialect and should not lose constraints just because
/// Gemini rejects them.
pub(crate) fn normalize_gemini_function_parameters(parameters: Option<&Value>) -> Value {
    let mut parameters = normalize_function_parameters(parameters);
    normalize_gemini_schema_node(&mut parameters);
    parameters
}

pub(crate) fn normalize_gemini_tool_schemas(request: &mut Value) {
    let Some(tools) = request.get_mut("tools").and_then(Value::as_array_mut) else {
        return;
    };
    for tool in tools {
        let Some(tool) = tool.as_object_mut() else {
            continue;
        };
        let declarations = if tool.contains_key("functionDeclarations") {
            tool.get_mut("functionDeclarations")
        } else {
            tool.get_mut("function_declarations")
        };
        let Some(declarations) = declarations.and_then(Value::as_array_mut) else {
            continue;
        };
        for declaration in declarations {
            let Some(declaration) = declaration.as_object_mut() else {
                continue;
            };
            for key in [
                "parameters",
                "parametersJsonSchema",
                "parameters_json_schema",
            ] {
                if let Some(parameters) = declaration.get_mut(key) {
                    *parameters = normalize_gemini_function_parameters(Some(parameters));
                    break;
                }
            }
        }
    }
}

/// Restricts Antigravity schemas to the private Code Assist protobuf subset.
///
/// This deliberately walks only declaration and generation schema locations.
/// Applying a schema cleaner to the whole request would corrupt ordinary keys
/// such as `title`, `format`, or `default` inside historical function-call args.
pub(crate) fn normalize_antigravity_request_schemas(request: &mut Value) {
    let Some(request) = request.as_object_mut() else {
        return;
    };
    if let Some(tools) = request.get_mut("tools").and_then(Value::as_array_mut) {
        for tool in tools {
            let Some(tool) = tool.as_object_mut() else {
                continue;
            };
            let declarations = if tool.contains_key("functionDeclarations") {
                tool.get_mut("functionDeclarations")
            } else {
                tool.get_mut("function_declarations")
            }
            .and_then(Value::as_array_mut);
            let Some(declarations) = declarations else {
                continue;
            };
            for declaration in declarations {
                let Some(declaration) = declaration.as_object_mut() else {
                    continue;
                };
                if !declaration.contains_key("parameters") {
                    let legacy = declaration
                        .remove("parametersJsonSchema")
                        .or_else(|| declaration.remove("parameters_json_schema"));
                    if let Some(parameters) = legacy {
                        declaration.insert("parameters".to_string(), parameters);
                    }
                } else {
                    declaration.remove("parametersJsonSchema");
                    declaration.remove("parameters_json_schema");
                }
                for key in [
                    "parameters",
                    "response",
                    "responseJsonSchema",
                    "response_json_schema",
                ] {
                    if let Some(schema) = declaration.get_mut(key) {
                        clean_antigravity_schema(schema, AntigravitySchemaMode::Tool);
                    }
                }
            }
        }
    }
    for container in ["generationConfig", "generation_config"] {
        let Some(config) = request.get_mut(container).and_then(Value::as_object_mut) else {
            continue;
        };
        for key in [
            "responseSchema",
            "responseJsonSchema",
            "response_schema",
            "response_json_schema",
        ] {
            if let Some(schema) = config.get_mut(key) {
                clean_antigravity_schema(schema, AntigravitySchemaMode::Response);
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AntigravitySchemaMode {
    Tool,
    Response,
}

fn clean_antigravity_schema(schema: &mut Value, mode: AntigravitySchemaMode) {
    if !schema.is_object() {
        *schema = json!({"type": "object", "properties": {}});
    }
    let root = schema.clone();
    let mut resolving = HashSet::new();
    resolve_antigravity_local_refs(schema, &root, &mut resolving, 0);
    clean_antigravity_schema_node(schema, mode);
}

fn resolve_antigravity_local_refs(
    value: &mut Value,
    root: &Value,
    resolving: &mut HashSet<String>,
    depth: usize,
) {
    if depth > 16 {
        return;
    }
    let Some(object) = value.as_object_mut() else {
        return;
    };
    if let Some(reference) = object
        .get("$ref")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|reference| reference.starts_with("#/"))
    {
        if resolving.insert(reference.clone()) {
            if let Some(target) = root.pointer(reference.trim_start_matches('#')).cloned() {
                let mut target = target;
                resolve_antigravity_local_refs(&mut target, root, resolving, depth + 1);
                if let Some(target) = target.as_object_mut() {
                    object.remove("$ref");
                    for (key, child) in std::mem::take(target) {
                        object.entry(key).or_insert(child);
                    }
                }
            }
            resolving.remove(&reference);
        }
    }
    for key in [
        "properties",
        "patternProperties",
        "$defs",
        "definitions",
        "dependentSchemas",
    ] {
        if let Some(children) = object.get_mut(key).and_then(Value::as_object_mut) {
            for child in children.values_mut() {
                resolve_antigravity_local_refs(child, root, resolving, depth + 1);
            }
        }
    }
    for key in [
        "items",
        "additionalProperties",
        "not",
        "if",
        "then",
        "else",
        "propertyNames",
        "contains",
    ] {
        if let Some(child) = object.get_mut(key) {
            resolve_antigravity_local_refs(child, root, resolving, depth + 1);
        }
    }
    for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(children) = object.get_mut(key).and_then(Value::as_array_mut) {
            for child in children {
                resolve_antigravity_local_refs(child, root, resolving, depth + 1);
            }
        }
    }
}

fn clean_antigravity_schema_node(value: &mut Value, mode: AntigravitySchemaMode) {
    if !value.is_object() {
        *value = json!({"type": "string"});
    }
    let Some(object) = value.as_object_mut() else {
        unreachable!("Antigravity schema nodes are normalized to objects");
    };
    normalize_antigravity_type_array(object);
    merge_antigravity_all_of(object);
    project_antigravity_union(object, "anyOf");
    project_antigravity_union(object, "oneOf");
    promote_boolean_required_properties(object);

    if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
        for schema in properties.values_mut() {
            clean_antigravity_schema_node(schema, mode);
        }
    }
    if let Some(items) = object.get_mut("items") {
        clean_antigravity_schema_node(items, mode);
    } else if schema_type_is(object, "array") {
        object.insert("items".to_string(), json!({"type": "string"}));
    }

    if let Some(constant) = object.remove("const") {
        object
            .entry("enum".to_string())
            .or_insert_with(|| json!([constant]));
    }
    if let Some(values) = object.get_mut("enum").and_then(Value::as_array_mut) {
        if mode == AntigravitySchemaMode::Tool
            || values.iter().any(|value| matches!(value, Value::Bool(_)))
        {
            object.remove("enum");
        } else {
            for value in values {
                if !value.is_string() {
                    *value = Value::String(match value {
                        Value::Null => "null".to_string(),
                        _ => value.to_string(),
                    });
                }
            }
        }
    }

    let allowed = [
        "type",
        "properties",
        "items",
        "required",
        "description",
        "enum",
        "nullable",
        "additionalProperties",
    ];
    object.retain(|key, value| {
        allowed.contains(&key.as_str())
            && !key.starts_with("x-")
            && (key != "additionalProperties"
                || (mode == AntigravitySchemaMode::Response && value == &Value::Bool(false)))
    });

    let property_names = object
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| properties.keys().cloned().collect::<HashSet<_>>())
        .unwrap_or_default();
    if let Some(required) = object.get_mut("required").and_then(Value::as_array_mut) {
        required.retain(|name| {
            name.as_str()
                .is_some_and(|name| property_names.contains(name))
        });
        if required.is_empty() {
            object.remove("required");
        }
    } else {
        object.remove("required");
    }

    if mode == AntigravitySchemaMode::Tool
        && schema_type_is(object, "object")
        && object
            .get("properties")
            .and_then(Value::as_object)
            .is_none_or(serde_json::Map::is_empty)
    {
        object.insert(
            "properties".to_string(),
            json!({"reason": {
                "type": "string",
                "description": "Brief explanation of why you are calling this tool"
            }}),
        );
        object.insert("required".to_string(), json!(["reason"]));
    }
}

fn normalize_antigravity_type_array(object: &mut Map<String, Value>) {
    let Some(types) = object.get("type").and_then(Value::as_array) else {
        return;
    };
    let nullable = types.iter().any(|value| value.as_str() == Some("null"));
    let selected = types
        .iter()
        .filter_map(Value::as_str)
        .find(|value| *value != "null")
        .unwrap_or("string")
        .to_string();
    object.insert("type".to_string(), Value::String(selected));
    if nullable {
        object.insert("nullable".to_string(), Value::Bool(true));
    }
}

fn merge_antigravity_all_of(object: &mut Map<String, Value>) {
    let Some(branches) = object
        .remove("allOf")
        .and_then(|value| value.as_array().cloned())
    else {
        return;
    };
    for branch in branches {
        let Some(branch) = branch.as_object() else {
            continue;
        };
        for (key, value) in branch {
            if key == "required" {
                let mut required = object
                    .get("required")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for item in value.as_array().into_iter().flatten() {
                    if !required.contains(item) {
                        required.push(item.clone());
                    }
                }
                object.insert("required".to_string(), Value::Array(required));
            } else if key == "properties" {
                let properties = object
                    .entry("properties".to_string())
                    .or_insert_with(|| json!({}));
                if let (Some(properties), Some(incoming)) =
                    (properties.as_object_mut(), value.as_object())
                {
                    for (name, schema) in incoming {
                        properties
                            .entry(name.clone())
                            .or_insert_with(|| schema.clone());
                    }
                }
            } else {
                object.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
    }
}

fn project_antigravity_union(object: &mut Map<String, Value>, key: &str) {
    let Some(branches) = object
        .remove(key)
        .and_then(|value| value.as_array().cloned())
    else {
        return;
    };
    if branches.is_empty() {
        return;
    }
    let nullable = branches
        .iter()
        .any(|branch| branch.get("type").and_then(Value::as_str) == Some("null"));
    let selected = branches.into_iter().max_by_key(|branch| {
        if branch.get("type").and_then(Value::as_str) == Some("object")
            || branch.get("properties").is_some()
        {
            3
        } else if branch.get("type").and_then(Value::as_str) == Some("array")
            || branch.get("items").is_some()
        {
            2
        } else if branch.get("type").and_then(Value::as_str) == Some("null") {
            0
        } else {
            1
        }
    });
    if let Some(selected) = selected.and_then(|value| value.as_object().cloned()) {
        for (field, value) in selected {
            object.entry(field).or_insert(value);
        }
    }
    if nullable {
        object.insert("nullable".to_string(), Value::Bool(true));
    }
}

fn normalize_gemini_schema_node(value: &mut Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };

    promote_boolean_required_properties(object);

    if schema_type_is(object, "integer") {
        normalize_integral_exclusive_bound(object, "exclusiveMinimum", "minimum", 1);
        normalize_integral_exclusive_bound(object, "exclusiveMaximum", "maximum", -1);
    } else {
        object.remove("exclusiveMinimum");
        object.remove("exclusiveMaximum");
    }

    // Code Assist currently rejects this keyword even though it is valid JSON
    // Schema. Dropping it weakens validation but keeps the tool callable.
    object.remove("uniqueItems");

    for key in ["properties", "patternProperties", "$defs", "definitions"] {
        if let Some(children) = object.get_mut(key).and_then(Value::as_object_mut) {
            for child in children.values_mut() {
                normalize_gemini_schema_node(child);
            }
        }
    }
    for key in [
        "items",
        "additionalProperties",
        "contains",
        "if",
        "then",
        "else",
        "not",
        "propertyNames",
    ] {
        if let Some(child) = object.get_mut(key) {
            normalize_gemini_schema_node(child);
        }
    }
    for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(children) = object.get_mut(key).and_then(Value::as_array_mut) {
            for child in children {
                normalize_gemini_schema_node(child);
            }
        }
    }
}

fn promote_boolean_required_properties(object: &mut Map<String, Value>) {
    let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) else {
        return;
    };
    let mut promoted = Vec::new();
    for (name, schema) in properties.iter_mut() {
        let Some(schema) = schema.as_object_mut() else {
            continue;
        };
        if let Some(required) = schema.get("required").and_then(Value::as_bool) {
            if required {
                promoted.push(name.clone());
            }
            schema.remove("required");
        }
    }
    if promoted.is_empty() {
        return;
    }

    let mut required = object
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    required.extend(promoted);
    object.insert(
        "required".to_string(),
        Value::Array(required.into_iter().map(Value::String).collect()),
    );
}

fn schema_type_is(object: &Map<String, Value>, expected: &str) -> bool {
    object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case(expected))
}

fn normalize_integral_exclusive_bound(
    object: &mut Map<String, Value>,
    exclusive_key: &str,
    inclusive_key: &str,
    delta: i8,
) {
    let Some(exclusive) = object.remove(exclusive_key) else {
        return;
    };
    let candidate = match exclusive {
        Value::Bool(false) => return,
        Value::Bool(true) => object
            .get(inclusive_key)
            .and_then(|value| increment_integral_bound(value, delta)),
        value => increment_integral_bound(&value, delta),
    };
    let Some(candidate) = candidate else {
        return;
    };
    let replace = object
        .get(inclusive_key)
        .and_then(json_number_as_f64)
        .zip(json_number_as_f64(&candidate))
        .map(|(existing, candidate)| {
            if delta > 0 {
                existing < candidate
            } else {
                existing > candidate
            }
        })
        .unwrap_or(true);
    if replace {
        object.insert(inclusive_key.to_string(), candidate);
    }
}

fn increment_integral_bound(value: &Value, delta: i8) -> Option<Value> {
    if let Some(value) = value.as_i64() {
        return value
            .checked_add(i64::from(delta))
            .map(|value| Value::Number(value.into()));
    }
    let value = value.as_u64()?;
    if delta > 0 {
        value
            .checked_add(delta as u64)
            .map(|value| Value::Number(value.into()))
    } else {
        value
            .checked_sub(delta.unsigned_abs() as u64)
            .map(|value| Value::Number(value.into()))
    }
}

fn json_number_as_f64(value: &Value) -> Option<f64> {
    value.as_f64().filter(|value| value.is_finite())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn defaults_missing_null_and_non_object_parameters() {
        for parameters in [None, Some(&Value::Null), Some(&json!(["invalid"]))] {
            assert_eq!(
                normalize_function_parameters(parameters),
                json!({"type": "object", "properties": {}})
            );
        }
    }

    #[test]
    fn forces_object_type_without_dropping_schema_keywords() {
        let normalized = normalize_function_parameters(Some(&json!({
            "type": null,
            "oneOf": [
                {"type": "object", "properties": {"id": {"type": "string"}}},
                {"type": "object", "properties": {"slug": {"type": "string"}}}
            ]
        })));
        assert_eq!(normalized["type"], "object");
        assert_eq!(normalized["oneOf"].as_array().map(Vec::len), Some(2));
    }

    #[test]
    fn preserves_valid_object_schema() {
        let schema = json!({
            "type": "object",
            "properties": {"query": {"type": "string"}},
            "required": ["query"]
        });
        assert_eq!(normalize_function_parameters(Some(&schema)), schema);
    }

    #[test]
    fn proxy_bridge_contract_fixture_normalizes_tool_schema_roots() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/proxy_bridge/tool_schema.json"
        ))
        .unwrap();
        assert_eq!(fixture["id"], "tool-schema-root-object");
        assert_eq!(fixture["category"], "tool_schema");

        for case in fixture["cases"].as_array().unwrap() {
            assert_eq!(
                normalize_function_parameters(case["input"].get("parameters")),
                case["expected"],
                "fixture case {}",
                case["name"]
            );
        }
    }

    #[test]
    fn gemini_normalizes_nested_exclusive_bounds_and_unique_items() {
        let normalized = normalize_gemini_function_parameters(Some(&json!({
            "type": "object",
            "properties": {
                "counts": {
                    "type": "array",
                    "uniqueItems": true,
                    "items": {"type": "integer", "exclusiveMinimum": 0}
                },
                "strict": {"type": "integer", "exclusiveMinimum": 0, "minimum": 5},
                "weak": {"type": "integer", "exclusiveMinimum": 2, "minimum": 1},
                "upper": {"type": "integer", "exclusiveMaximum": 10}
            }
        })));

        assert_eq!(
            normalized.pointer("/properties/counts/items/minimum"),
            Some(&json!(1))
        );
        assert!(normalized
            .pointer("/properties/counts/uniqueItems")
            .is_none());
        assert_eq!(
            normalized.pointer("/properties/strict/minimum"),
            Some(&json!(5))
        );
        assert_eq!(
            normalized.pointer("/properties/weak/minimum"),
            Some(&json!(3))
        );
        assert_eq!(
            normalized.pointer("/properties/upper/maximum"),
            Some(&json!(9))
        );
    }

    #[test]
    fn gemini_drops_ambiguous_exclusive_bounds_and_promotes_required_flags() {
        let normalized = normalize_gemini_function_parameters(Some(&json!({
            "type": "object",
            "properties": {
                "ratio": {"type": "number", "exclusiveMinimum": 0.5},
                "query": {"type": "string", "required": true},
                "optional": {"type": "string", "required": false}
            },
            "required": ["existing"]
        })));

        assert!(normalized
            .pointer("/properties/ratio/exclusiveMinimum")
            .is_none());
        assert!(normalized.pointer("/properties/query/required").is_none());
        assert!(normalized
            .pointer("/properties/optional/required")
            .is_none());
        assert_eq!(normalized["required"], json!(["existing", "query"]));
    }

    #[test]
    fn gemini_native_request_normalizes_each_function_declaration() {
        let mut request = json!({
            "tools": [{
                "functionDeclarations": [{
                    "name": "lookup",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "limit": {"type": "integer", "exclusiveMinimum": 0}
                        }
                    }
                }]
            }]
        });
        normalize_gemini_tool_schemas(&mut request);
        assert_eq!(
            request.pointer("/tools/0/functionDeclarations/0/parameters/properties/limit/minimum"),
            Some(&json!(1))
        );
        assert!(request
            .pointer("/tools/0/functionDeclarations/0/parameters/properties/limit/exclusiveMinimum")
            .is_none());
    }

    #[test]
    fn antigravity_schema_cleaning_is_scoped_and_renames_legacy_parameters() {
        let history = json!([{
            "role": "model",
            "parts": [{"functionCall": {"name": "write", "args": {
                "title": "keep",
                "format": "markdown",
                "default": "keep",
                "additionalProperties": "ordinary argument"
            }}}]
        }]);
        let mut request = json!({
            "contents": history.clone(),
            "tools": [{"functionDeclarations": [{
                "name": "write",
                "parametersJsonSchema": {
                    "type": "object",
                    "$schema": "draft",
                    "properties": {
                        "title": {"type": "string", "title": "drop", "minLength": 3},
                        "items": {"type": "array", "uniqueItems": true}
                    },
                    "required": ["title", "missing"],
                    "additionalProperties": false,
                    "x-private": true
                }
            }]}]
        });
        normalize_antigravity_request_schemas(&mut request);

        assert_eq!(request["contents"], history);
        let declaration = request.pointer("/tools/0/functionDeclarations/0").unwrap();
        assert!(declaration.get("parametersJsonSchema").is_none());
        let schema = declaration.get("parameters").unwrap();
        assert_eq!(
            schema.pointer("/properties/title/type"),
            Some(&json!("string"))
        );
        assert!(schema.pointer("/properties/title/title").is_none());
        assert!(schema.pointer("/properties/title/minLength").is_none());
        assert_eq!(
            schema.pointer("/properties/items/items/type"),
            Some(&json!("string"))
        );
        assert_eq!(schema.get("required"), Some(&json!(["title"])));
        assert!(schema.get("additionalProperties").is_none());
        assert!(schema.get("x-private").is_none());
    }

    #[test]
    fn antigravity_response_schema_resolves_refs_projects_unions_and_is_idempotent() {
        let mut request = json!({
            "generationConfig": {
                "responseSchema": {
                    "type": "object",
                    "$defs": {
                        "result": {
                            "type": "object",
                            "properties": {"score": {"type": "number", "enum": [0.5, 1]}},
                            "required": ["score"],
                            "additionalProperties": false
                        }
                    },
                    "properties": {
                        "result": {"anyOf": [{"$ref": "#/$defs/result"}, {"type": "null"}]},
                        "enabled": {"type": "boolean", "enum": [true, false]}
                    },
                    "required": ["result", "enabled"],
                    "additionalProperties": false
                }
            }
        });
        normalize_antigravity_request_schemas(&mut request);
        let once = request.clone();
        normalize_antigravity_request_schemas(&mut request);
        assert_eq!(request, once);

        let schema = request.pointer("/generationConfig/responseSchema").unwrap();
        assert!(schema.get("$defs").is_none());
        assert_eq!(schema.get("additionalProperties"), Some(&json!(false)));
        assert_eq!(
            schema.pointer("/properties/result/type"),
            Some(&json!("object"))
        );
        assert_eq!(
            schema.pointer("/properties/result/nullable"),
            Some(&json!(true))
        );
        assert_eq!(
            schema.pointer("/properties/result/properties/score/enum"),
            Some(&json!(["0.5", "1"]))
        );
        assert!(schema.pointer("/properties/enabled/enum").is_none());
    }

    #[test]
    fn antigravity_cleans_tool_result_and_both_generation_schema_spellings() {
        let dirty =
            || json!({"type": "object", "$comment": "drop", "propertyNames": {"type": "string"}});
        let mut request = json!({
            "tools": [{"function_declarations": [{
                "name": "lookup",
                "parameters": dirty(),
                "response": dirty(),
                "responseJsonSchema": dirty(),
                "response_json_schema": dirty()
            }]}],
            "generationConfig": {
                "responseSchema": dirty(),
                "responseJsonSchema": dirty()
            },
            "generation_config": {
                "response_schema": dirty(),
                "response_json_schema": dirty()
            }
        });
        normalize_antigravity_request_schemas(&mut request);
        for pointer in [
            "/tools/0/function_declarations/0/parameters",
            "/tools/0/function_declarations/0/response",
            "/tools/0/function_declarations/0/responseJsonSchema",
            "/tools/0/function_declarations/0/response_json_schema",
            "/generationConfig/responseSchema",
            "/generationConfig/responseJsonSchema",
            "/generation_config/response_schema",
            "/generation_config/response_json_schema",
        ] {
            let schema = request.pointer(pointer).unwrap();
            assert!(schema.get("$comment").is_none(), "{pointer}");
            assert!(schema.get("propertyNames").is_none(), "{pointer}");
        }
    }

    #[test]
    fn codex_schema_removes_nested_null_types_without_weakening_constraints() {
        let mut request = json!({
            "tools": [{
                "type": "function",
                "name": "lookup",
                "parameters": {
                    "type": null,
                    "properties": {"q": {"type": null, "pattern": "^[a-z]+$"}},
                    "$defs": {"item": {"type": null}},
                    "items": [{"type": null}],
                    "allOf": [{"type": null}]
                }
            }],
            "input": [{
                "type": "additional_tools",
                "tools": [{"type": "function", "name": "extra", "parameters": {"type": null}}]
            }]
        });
        normalize_codex_tool_schemas(&mut request);
        assert_eq!(
            request.pointer("/tools/0/parameters/type"),
            Some(&json!("object"))
        );
        assert!(request
            .pointer("/tools/0/parameters/properties/q/type")
            .is_none());
        assert_eq!(
            request.pointer("/tools/0/parameters/properties/q/pattern"),
            Some(&json!("^[a-z]+$"))
        );
        assert!(request
            .pointer("/tools/0/parameters/$defs/item/type")
            .is_none());
        assert!(request
            .pointer("/tools/0/parameters/items/0/type")
            .is_none());
        assert!(request
            .pointer("/tools/0/parameters/allOf/0/type")
            .is_none());
        assert_eq!(
            request.pointer("/input/0/tools/0/parameters/type"),
            Some(&json!("object"))
        );
    }

    #[test]
    fn codex_strips_only_incompatible_patterns_from_nested_schema_locations() {
        let mut request = json!({
            "tools": [{
                "type": "function",
                "name": "lookup",
                "parameters": {
                    "type": "object",
                    "description": "keep \\p{L}",
                    "default": {"pattern": "\\p{L}"},
                    "enum": ["\\P{N}"],
                    "properties": {
                        "direct": {"type": "string", "pattern": "^\\p{L}+$"},
                        "ordinary": {"type": "string", "pattern": "^[a-z]+$"},
                        "array": {
                            "type": "array",
                            "items": {"type": "string", "pattern": "^\\P{C}+$"}
                        }
                    },
                    "patternProperties": {
                        "^safe\\d+$": {"type": "string", "pattern": "\\p{Script=Han}"},
                        "^\\u005cp{L}+$": {"type": "number"}
                    },
                    "$defs": {
                        "escaped.key:literal": {"type": "string", "pattern": "\\P{Z}"}
                    }
                }
            }]
        });

        normalize_codex_tool_schemas(&mut request);
        let schema = request.pointer("/tools/0/parameters").unwrap();
        assert_eq!(schema["description"], "keep \\p{L}");
        assert_eq!(schema["default"], json!({"pattern": "\\p{L}"}));
        assert_eq!(schema["enum"], json!(["\\P{N}"]));
        assert!(schema.pointer("/properties/direct/pattern").is_none());
        assert_eq!(
            schema.pointer("/properties/ordinary/pattern"),
            Some(&json!("^[a-z]+$"))
        );
        assert!(schema.pointer("/properties/array/items/pattern").is_none());
        assert!(schema
            .pointer("/patternProperties/^safe\\d+$/pattern")
            .is_none());
        assert!(schema
            .get("patternProperties")
            .and_then(Value::as_object)
            .is_some_and(|patterns| !patterns.contains_key("^\\u005cp{L}+$")));
        assert!(schema
            .pointer("/$defs/escaped.key:literal/pattern")
            .is_none());
    }

    #[test]
    fn codex_unicode_pattern_detector_covers_escape_mutations() {
        for (pattern, incompatible) in [
            (r"\p{L}", true),
            (r"\P{Letter}", true),
            (r"\\p{L}", false),
            (r"\\\p{L}", true),
            (r"\\\\p{L}", false),
            (r"\p", false),
            (r"\p{", true),
            (r"\u005cp{L}", true),
            (r"\U005CP{L}", true),
            (r"\\u005cp{L}", false),
            (r"^[0-9a-f]{32}$", false),
        ] {
            assert_eq!(
                has_incompatible_codex_unicode_escape(pattern),
                incompatible,
                "pattern mutation {pattern:?}"
            );
        }
    }

    #[test]
    fn codex_schema_sanitizer_fails_closed_at_each_budget() {
        let oversized = |tail: Value| {
            json!({
                "type": "object",
                "pattern": "\\p{L}",
                "properties": {"tail": tail}
            })
        };

        let mut deep = json!({"type": "string"});
        for _ in 0..=CODEX_SCHEMA_MAX_DEPTH {
            deep = json!({"properties": {"next": deep}});
        }
        let mut deep = oversized(deep);
        normalize_codex_schema_node(&mut deep);
        assert_eq!(deep.get("pattern"), Some(&json!("\\p{L}")));

        let properties = (0..CODEX_SCHEMA_MAX_NODES)
            .map(|index| (format!("p{index}"), json!({"type": "string"})))
            .collect::<Map<_, _>>();
        let mut many = oversized(Value::Object(properties));
        normalize_codex_schema_node(&mut many);
        assert_eq!(many.get("pattern"), Some(&json!("\\p{L}")));

        let mut large = oversized(json!({
            "type": "string",
            "description": "x".repeat(CODEX_SCHEMA_MAX_BYTES / 6 + 1)
        }));
        normalize_codex_schema_node(&mut large);
        assert_eq!(large.get("pattern"), Some(&json!("\\p{L}")));
    }

    #[test]
    fn codex_simplifies_large_pure_const_unions_without_dropping_parent_constraints() {
        let constants = [
            "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta",
        ];
        let branches = constants
            .iter()
            .map(|value| json!({"const": value, "description": format!("choice {value}")}))
            .collect::<Vec<_>>();
        let mut schema = json!({
            "type": "string",
            "description": "mode",
            "default": "alpha",
            "nullable": true,
            "minLength": 2,
            "oneOf": branches
        });
        let before = serde_json::to_vec(&schema).unwrap().len();

        normalize_codex_schema_node(&mut schema);

        assert!(schema.get("oneOf").is_none());
        assert_eq!(schema["enum"], json!(constants));
        assert_eq!(schema["description"], "mode");
        assert_eq!(schema["default"], "alpha");
        assert_eq!(schema["nullable"], true);
        assert_eq!(schema["minLength"], 2);
        assert!(serde_json::to_vec(&schema).unwrap().len() < before);
    }

    #[test]
    fn codex_simplifies_nested_array_const_union_and_is_idempotent() {
        let branches = (0..8)
            .map(|value| json!({"const": value}))
            .collect::<Vec<_>>();
        let mut schema = json!({
            "type": "object",
            "properties": {
                "codes": {"type": "array", "items": {"type": "integer", "anyOf": branches}}
            }
        });
        normalize_codex_schema_node(&mut schema);
        let once = schema.clone();
        normalize_codex_schema_node(&mut schema);
        assert_eq!(schema, once);
        assert_eq!(
            schema.pointer("/properties/codes/items/enum"),
            Some(&json!([0, 1, 2, 3, 4, 5, 6, 7]))
        );
        assert!(schema.pointer("/properties/codes/items/anyOf").is_none());
    }

    #[test]
    fn codex_leaves_uncertain_const_unions_unchanged() {
        let base = (0..8)
            .map(|value| json!({"const": format!("v{value}")}))
            .collect::<Vec<_>>();
        let mut cases = Vec::new();

        let mut mixed = base.clone();
        mixed[7] = json!({"const": 7});
        cases.push(json!({"oneOf": mixed}));

        let mut constrained = base.clone();
        constrained[7] = json!({"const": "v7", "type": "string"});
        cases.push(json!({"oneOf": constrained}));

        let mut reference = base.clone();
        reference[7] = json!({"$ref": "#/$defs/value"});
        cases.push(json!({"oneOf": reference}));

        let mut duplicate = base.clone();
        duplicate[7] = json!({"const": "v0"});
        cases.push(json!({"oneOf": duplicate}));

        cases.push(json!({"oneOf": base.clone(), "anyOf": base.clone()}));
        cases.push(json!({"type": "number", "oneOf": base.clone()}));
        cases.push(json!({"oneOf": base.clone(), "enum": ["v0"]}));
        cases.push(json!({
            "oneOf": (0..8).map(|value| json!({"const": {"v": value}})).collect::<Vec<_>>()
        }));

        let numeric_duplicate = [
            json!(1),
            json!(1.0),
            json!(2),
            json!(3),
            json!(4),
            json!(5),
            json!(6),
            json!(7),
        ]
        .into_iter()
        .map(|value| json!({"const": value}))
        .collect::<Vec<_>>();
        cases.push(json!({"oneOf": numeric_duplicate}));

        for mut schema in cases {
            let original = schema.clone();
            normalize_codex_schema_node(&mut schema);
            assert_eq!(schema, original);
        }
    }

    #[test]
    fn codex_reserved_tool_is_value_identical() {
        let reserved = json!({
            "type": "function",
            "name": "collaboration.spawn_agent",
            "parameters": {"type": null}
        });
        let mut request = json!({"tools": [reserved.clone()]});
        normalize_codex_tool_schemas(&mut request);
        assert_eq!(request.pointer("/tools/0"), Some(&reserved));
    }
}
