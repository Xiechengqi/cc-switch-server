//! Cursor tool-call resolver.
//!
//! Cursor AgentService can occasionally emit a declared tool name with argument
//! keys that belong to a different Claude Code tool. Resolve against the
//! client's declared inventory before surfacing a tool_use/tool_calls block.

use super::agent_proto::McpToolDef;
use bytes::Bytes;
use serde_json::{Map, Number, Value};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedToolCall {
    pub name: String,
    pub args: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InvalidToolCall {
    pub original_name: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
struct ToolSpec {
    name: String,
    schema: ToolSchema,
}

#[derive(Debug, Clone, Default)]
struct ToolSchema {
    properties: HashMap<String, Value>,
    required: HashSet<String>,
    additional_properties: Option<bool>,
    open: bool,
}

pub fn resolve_tool_call(
    tools: &[McpToolDef],
    emitted_name: &str,
    args: Value,
) -> Result<ResolvedToolCall, InvalidToolCall> {
    let args_obj = match args {
        Value::Object(map) => map,
        other => {
            return Err(invalid(
                emitted_name,
                format!("arguments must be a JSON object, got {}", json_type(&other)),
            ))
        }
    };
    let specs: Vec<ToolSpec> = tools.iter().map(tool_spec).collect();
    if specs.is_empty() {
        return Err(invalid(
            emitted_name,
            "no client tool inventory is available",
        ));
    }

    let emitted_canonical = canonical_tool_name(emitted_name);
    if emitted_canonical == "websearch" {
        if let Some(target) = resolve_websearch_misroute(&specs, &args_obj) {
            return target;
        }
    }

    if let Some(spec) = find_tool(&specs, emitted_name) {
        if let Some(args) = normalize_and_validate(&args_obj, spec, &emitted_canonical) {
            return Ok(ResolvedToolCall {
                name: spec.name.clone(),
                args,
            });
        }
    }

    for candidate in candidate_tool_names(&emitted_canonical, &args_obj) {
        if let Some(spec) = find_tool_by_canonical(&specs, candidate) {
            if let Some(args) = normalize_and_validate(&args_obj, spec, candidate) {
                return Ok(ResolvedToolCall {
                    name: spec.name.clone(),
                    args,
                });
            }
        }
    }

    let allowed = specs
        .iter()
        .map(|s| s.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Err(invalid(
        emitted_name,
        format!("arguments do not satisfy any declared tool schema; allowed tools: {allowed}"),
    ))
}

fn resolve_websearch_misroute(
    specs: &[ToolSpec],
    args: &Map<String, Value>,
) -> Option<Result<ResolvedToolCall, InvalidToolCall>> {
    let has_grep_shape = has_any(args, &["pattern", "regex", "search"])
        || (has_any(args, &["query"]) && has_any(args, &["path", "glob", "include"]));
    let has_glob_shape = has_any(
        args,
        &[
            "glob",
            "globPattern",
            "glob_pattern",
            "filePattern",
            "file_pattern",
            "include",
            "targetDirectory",
            "target_directory",
            "targeting",
        ],
    );
    let order: &[&str] = if has_grep_shape {
        &["grep", "glob"]
    } else if has_glob_shape {
        &["glob", "grep"]
    } else {
        return None;
    };
    for target in order {
        if let Some(spec) = find_tool_by_canonical(specs, target) {
            if let Some(args) = normalize_and_validate(args, spec, target) {
                return Some(Ok(ResolvedToolCall {
                    name: spec.name.clone(),
                    args,
                }));
            }
        }
    }
    Some(Err(invalid(
        "WebSearch",
        "WebSearch was called with filesystem-search arguments, but no declared Glob/Grep schema accepted them",
    )))
}

fn normalize_and_validate(
    args: &Map<String, Value>,
    spec: &ToolSpec,
    emitted_canonical: &str,
) -> Option<Value> {
    let mut normalized = normalize_arguments(args, spec, emitted_canonical);
    repair_glob_swapped_values(&mut normalized, spec);
    if schema_accepts(&normalized, &spec.schema) {
        Some(Value::Object(normalized))
    } else {
        None
    }
}

fn normalize_arguments(
    args: &Map<String, Value>,
    spec: &ToolSpec,
    emitted_canonical: &str,
) -> Map<String, Value> {
    if spec.schema.open {
        return args.clone();
    }
    let canonical = canonical_tool_name(&spec.name);
    let effective = if canonical.is_empty() {
        emitted_canonical
    } else {
        canonical.as_str()
    };
    let property_lookup = normalized_property_lookup(&spec.schema);
    let mut out = Map::new();
    for (key, value) in args {
        if let Some(target) = property_lookup
            .get(&normalize_key(key))
            .cloned()
            .or_else(|| alias_property(effective, key, &property_lookup))
        {
            let prop_schema = spec.schema.properties.get(&target);
            out.insert(target, normalize_value_for_schema(value, prop_schema));
        } else if spec.schema.additional_properties.unwrap_or(false) {
            out.insert(key.clone(), value.clone());
        }
    }
    apply_required_defaults(&mut out, args, spec, effective, &property_lookup);
    out
}

fn apply_required_defaults(
    out: &mut Map<String, Value>,
    original: &Map<String, Value>,
    spec: &ToolSpec,
    canonical: &str,
    property_lookup: &HashMap<String, String>,
) {
    if canonical == "glob" {
        if let Some(pattern_key) = first_property(
            property_lookup,
            &["pattern", "globPattern", "glob_pattern", "query"],
        ) {
            if spec.schema.required.contains(&pattern_key) && !out.contains_key(&pattern_key) {
                if let Some(pattern) = first_string(
                    original,
                    &[
                        "glob",
                        "globPattern",
                        "glob_pattern",
                        "filePattern",
                        "file_pattern",
                        "pattern",
                        "query",
                        "include",
                    ],
                ) {
                    out.insert(pattern_key, Value::String(pattern));
                }
            }
        }
        if let Some(path_key) = first_property(
            property_lookup,
            &["path", "targetDirectory", "target_directory", "cwd", "root"],
        ) {
            if spec.schema.required.contains(&path_key) && !out.contains_key(&path_key) {
                out.insert(path_key, Value::String(".".to_string()));
            }
        }
    } else if canonical == "grep" {
        if let Some(pattern_key) = first_property(property_lookup, &["pattern", "query", "regex"]) {
            if spec.schema.required.contains(&pattern_key) && !out.contains_key(&pattern_key) {
                if let Some(pattern) =
                    first_string(original, &["pattern", "query", "regex", "search"])
                {
                    out.insert(pattern_key, Value::String(pattern));
                }
            }
        }
    }
}

fn repair_glob_swapped_values(args: &mut Map<String, Value>, spec: &ToolSpec) {
    if canonical_tool_name(&spec.name) != "glob" {
        return;
    }
    let property_lookup = normalized_property_lookup(&spec.schema);
    let Some(pattern_key) = first_property(
        &property_lookup,
        &["pattern", "globPattern", "glob_pattern", "query"],
    ) else {
        return;
    };
    let Some(path_key) = first_property(
        &property_lookup,
        &["path", "targetDirectory", "target_directory", "cwd", "root"],
    ) else {
        return;
    };
    let pattern = args
        .get(&pattern_key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let path = args
        .get(&path_key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if !looks_like_glob(&pattern) && looks_like_glob(&path) {
        let next_pattern = Value::String(path);
        let next_path = if pattern.is_empty() { "." } else { &pattern };
        args.insert(pattern_key, next_pattern);
        args.insert(path_key, Value::String(next_path.to_string()));
    }
}

fn schema_accepts(args: &Map<String, Value>, schema: &ToolSchema) -> bool {
    for required in &schema.required {
        if !args.contains_key(required) {
            return false;
        }
    }
    if schema.open {
        return true;
    }
    for (key, value) in args {
        if let Some(prop_schema) = schema.properties.get(key) {
            if !value_matches_schema(value, prop_schema) {
                return false;
            }
            continue;
        }
        if schema.additional_properties == Some(false) {
            return false;
        }
    }
    true
}

fn value_matches_schema(value: &Value, schema: &Value) -> bool {
    let Some(obj) = schema.as_object() else {
        return true;
    };
    if let Some(const_value) = obj.get("const") {
        return value == const_value;
    }
    if let Some(enum_values) = obj.get("enum").and_then(Value::as_array) {
        if !enum_values.iter().any(|v| v == value) {
            return false;
        }
    }
    if let Some(any_of) = obj.get("anyOf").and_then(Value::as_array) {
        if !any_of.iter().any(|s| value_matches_schema(value, s)) {
            return false;
        }
    }
    if let Some(one_of) = obj.get("oneOf").and_then(Value::as_array) {
        if !one_of.iter().any(|s| value_matches_schema(value, s)) {
            return false;
        }
    }
    if let Some(all_of) = obj.get("allOf").and_then(Value::as_array) {
        if !all_of.iter().all(|s| value_matches_schema(value, s)) {
            return false;
        }
    }
    let types = schema_types(obj.get("type"));
    if types.is_empty() {
        return true;
    }
    types.iter().any(|ty| json_type_matches(value, ty))
}

fn schema_types(value: Option<&Value>) -> Vec<&str> {
    match value {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(arr)) => arr.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

fn json_type_matches(value: &Value, ty: &str) -> bool {
    match ty {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        _ => true,
    }
}

fn tool_spec(tool: &McpToolDef) -> ToolSpec {
    ToolSpec {
        name: tool.name.clone(),
        schema: parse_tool_schema(&tool.input_schema),
    }
}

fn parse_tool_schema(bytes: &Bytes) -> ToolSchema {
    let root = serde_json::from_slice::<Value>(bytes).unwrap_or(Value::Object(Map::new()));
    let Some(obj) = root.as_object() else {
        return ToolSchema {
            open: true,
            ..ToolSchema::default()
        };
    };
    let properties = obj
        .get("properties")
        .and_then(Value::as_object)
        .map(|props| {
            props
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let required = obj
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    let additional_properties = obj.get("additionalProperties").and_then(Value::as_bool);
    let open = properties.is_empty() && required.is_empty() && additional_properties != Some(false);
    ToolSchema {
        properties,
        required,
        additional_properties,
        open,
    }
}

fn find_tool<'a>(specs: &'a [ToolSpec], name: &str) -> Option<&'a ToolSpec> {
    let norm = normalize_key(name);
    specs.iter().find(|s| normalize_key(&s.name) == norm)
}

fn find_tool_by_canonical<'a>(specs: &'a [ToolSpec], canonical: &str) -> Option<&'a ToolSpec> {
    specs
        .iter()
        .find(|s| canonical_tool_name(&s.name) == canonical)
        .or_else(|| {
            specs.iter().find(|s| {
                let norm = normalize_key(&s.name);
                norm == canonical || norm.contains(canonical)
            })
        })
}

fn candidate_tool_names(canonical: &str, args: &Map<String, Value>) -> Vec<&'static str> {
    let mut out = Vec::new();
    if has_any(
        args,
        &["glob", "globPattern", "glob_pattern", "filePattern"],
    ) {
        out.push("glob");
    }
    if has_any(args, &["pattern", "regex", "search"]) {
        out.push("grep");
    }
    match canonical {
        "websearch" => out.extend(["grep", "glob"]),
        "search" => out.extend(["grep", "glob"]),
        "grep" => out.push("grep"),
        "glob" => out.push("glob"),
        "shell" => out.push("shell"),
        "read" => out.push("read"),
        "write" => out.push("write"),
        "edit" => out.push("edit"),
        "webfetch" => out.push("webfetch"),
        _ => {}
    }
    dedup(out)
}

fn dedup(items: Vec<&'static str>) -> Vec<&'static str> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(*item))
        .collect()
}

fn canonical_tool_name(name: &str) -> String {
    let norm = normalize_key(name);
    if ["bash", "shell", "terminal", "runcommand", "runshellcommand"].contains(&norm.as_str()) {
        return "shell".to_string();
    }
    if ["websearch", "web_search"].contains(&norm.as_str()) {
        return "websearch".to_string();
    }
    if ["webfetch", "web_fetch", "fetch"].contains(&norm.as_str()) {
        return "webfetch".to_string();
    }
    if ["grep", "rg", "searchfiles", "search"].contains(&norm.as_str()) {
        return "grep".to_string();
    }
    if [
        "glob",
        "fileglob",
        "filesearch",
        "find",
        "findfile",
        "findfiles",
    ]
    .contains(&norm.as_str())
    {
        return "glob".to_string();
    }
    if ["read", "readfile", "openfile"].contains(&norm.as_str()) {
        return "read".to_string();
    }
    if ["write", "writefile", "createfile"].contains(&norm.as_str()) {
        return "write".to_string();
    }
    if ["edit", "editfile", "replacefile"].contains(&norm.as_str()) {
        return "edit".to_string();
    }
    norm
}

fn normalized_property_lookup(schema: &ToolSchema) -> HashMap<String, String> {
    schema
        .properties
        .keys()
        .map(|k| (normalize_key(k), k.clone()))
        .collect()
}

fn alias_property(canonical: &str, key: &str, props: &HashMap<String, String>) -> Option<String> {
    let norm = normalize_key(key);
    let candidates: &[&str] = match canonical {
        "glob" => match norm.as_str() {
            "glob" | "globpattern" | "filepattern" | "include" | "query" => {
                &["pattern", "globPattern", "glob_pattern", "query"]
            }
            "targetdirectory" | "targeting" | "root" | "rootdir" | "cwd" | "path" => {
                &["path", "targetDirectory", "target_directory", "cwd"]
            }
            _ => &[],
        },
        "grep" => match norm.as_str() {
            "query" | "search" | "regex" | "pattern" => &["pattern", "query", "regex", "search"],
            "glob" | "globpattern" | "include" => &["glob", "include", "files"],
            "targetdirectory" | "targeting" | "root" | "rootdir" | "cwd" | "path" => {
                &["path", "cwd"]
            }
            "caseinsensitive" | "ignorecase" => &[
                "case_insensitive",
                "caseInsensitive",
                "ignoreCase",
                "ignore_case",
            ],
            "headlimit" | "limit" | "maxresults" => &["limit", "headLimit", "head_limit"],
            _ => &[],
        },
        "websearch" => match norm.as_str() {
            "query" | "search" | "pattern" => &["query"],
            _ => &[],
        },
        "webfetch" => match norm.as_str() {
            "url" | "uri" | "href" => &["url"],
            _ => &[],
        },
        "shell" => match norm.as_str() {
            "cmd" | "command" | "script" => &["command", "cmd"],
            "cwd" | "workdir" | "workingdirectory" => &["workdir", "cwd"],
            _ => &[],
        },
        "read" | "write" | "edit" => match norm.as_str() {
            "path" | "filepath" | "targetfile" | "file" => &["path", "file_path", "filePath"],
            "content" | "contents" | "filetext" | "filecontent" => {
                &["content", "file_text", "fileText", "contents"]
            }
            "oldstring" | "oldstr" | "oldtext" | "search" => {
                &["old_string", "oldString", "old_str"]
            }
            "newstring" | "newstr" | "newtext" | "replacement" => {
                &["new_string", "newString", "new_str"]
            }
            _ => &[],
        },
        _ => &[],
    };
    first_property(props, candidates)
}

fn first_property(props: &HashMap<String, String>, candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .find_map(|candidate| props.get(&normalize_key(candidate)).cloned())
}

fn normalize_key(key: &str) -> String {
    key.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn normalize_value_for_schema(value: &Value, schema: Option<&Value>) -> Value {
    let numeric_types = schema
        .and_then(Value::as_object)
        .map(|obj| {
            let types = schema_types(obj.get("type"));
            (types.contains(&"integer"), types.contains(&"number"))
        })
        .unwrap_or((false, false));
    if !numeric_types.0 && !numeric_types.1 {
        return value.clone();
    }
    match value {
        Value::String(s) if numeric_types.0 => s
            .parse::<i64>()
            .map(Number::from)
            .map(Value::Number)
            .unwrap_or_else(|_| value.clone()),
        Value::String(s) if numeric_types.1 => s
            .parse::<f64>()
            .ok()
            .and_then(Number::from_f64)
            .map(Value::Number)
            .unwrap_or_else(|| value.clone()),
        _ => value.clone(),
    }
}

fn has_any(args: &Map<String, Value>, keys: &[&str]) -> bool {
    keys.iter().any(|key| args.contains_key(*key))
}

fn first_string(args: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        args.get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
    })
}

fn looks_like_glob(value: &str) -> bool {
    value.contains('*') || value.contains('?') || value.contains('[') || value.contains('{')
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn invalid(name: impl Into<String>, reason: impl Into<String>) -> InvalidToolCall {
    InvalidToolCall {
        original_name: name.into(),
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str, schema: Value) -> McpToolDef {
        McpToolDef {
            name: name.to_string(),
            description: String::new(),
            input_schema: Bytes::from(schema.to_string()),
            provider_identifier: "cc-switch".to_string(),
            tool_name: name.to_string(),
        }
    }

    #[test]
    fn websearch_with_glob_args_maps_to_glob() {
        let tools = vec![
            tool(
                "WebSearch",
                json!({"type":"object","additionalProperties":false,"properties":{"query":{"type":"string"}},"required":["query"]}),
            ),
            tool(
                "Glob",
                json!({"type":"object","additionalProperties":false,"properties":{"pattern":{"type":"string"},"path":{"type":"string"}},"required":["pattern"]}),
            ),
        ];
        let resolved = resolve_tool_call(
            &tools,
            "WebSearch",
            json!({"path":"/root","glob":"**/claude-api/**"}),
        )
        .unwrap();
        assert_eq!(resolved.name, "Glob");
        assert_eq!(
            resolved.args,
            json!({"path":"/root","pattern":"**/claude-api/**"})
        );
    }

    #[test]
    fn websearch_with_grep_args_maps_to_grep() {
        let tools = vec![
            tool(
                "WebSearch",
                json!({"type":"object","additionalProperties":false,"properties":{"query":{"type":"string"}},"required":["query"]}),
            ),
            tool(
                "Grep",
                json!({"type":"object","additionalProperties":false,"properties":{"pattern":{"type":"string"},"path":{"type":"string"}},"required":["pattern"]}),
            ),
        ];
        let resolved = resolve_tool_call(
            &tools,
            "WebSearch",
            json!({"pattern":"claude-opus","path":"/root"}),
        )
        .unwrap();
        assert_eq!(resolved.name, "Grep");
        assert_eq!(
            resolved.args,
            json!({"pattern":"claude-opus","path":"/root"})
        );
    }

    #[test]
    fn invalid_wrong_websearch_args_do_not_pass_through() {
        let tools = vec![tool(
            "WebSearch",
            json!({"type":"object","additionalProperties":false,"properties":{"query":{"type":"string"}},"required":["query"]}),
        )];
        let err = resolve_tool_call(&tools, "WebSearch", json!({"path":"/root","glob":"*.rs"}))
            .unwrap_err();
        assert!(err.reason.contains("filesystem-search"));
    }

    #[test]
    fn repairs_swapped_glob_args() {
        let tools = vec![tool(
            "Glob",
            json!({"type":"object","additionalProperties":false,"properties":{"pattern":{"type":"string"},"path":{"type":"string"}},"required":["pattern"]}),
        )];
        let resolved = resolve_tool_call(
            &tools,
            "Glob",
            json!({"targetDirectory":"**/*.tsx","globPattern":"/tmp/project"}),
        )
        .unwrap();
        assert_eq!(
            resolved.args,
            json!({"pattern":"**/*.tsx","path":"/tmp/project"})
        );
    }

    #[test]
    fn preserves_numeric_strings_for_string_schema() {
        let tools = vec![tool(
            "WebSearch",
            json!({"type":"object","additionalProperties":false,"properties":{"query":{"type":"string"}},"required":["query"]}),
        )];
        let resolved = resolve_tool_call(&tools, "WebSearch", json!({"query":"123"})).unwrap();
        assert_eq!(resolved.args, json!({"query":"123"}));
    }

    #[test]
    fn converts_numeric_strings_for_integer_schema() {
        let tools = vec![tool(
            "LimitTool",
            json!({"type":"object","additionalProperties":false,"properties":{"limit":{"type":"integer"}},"required":["limit"]}),
        )];
        let resolved = resolve_tool_call(&tools, "LimitTool", json!({"limit":"5"})).unwrap();
        assert_eq!(resolved.args, json!({"limit":5}));
    }

    #[test]
    fn converts_decimal_strings_for_number_schema() {
        let tools = vec![tool(
            "NumberTool",
            json!({"type":"object","additionalProperties":false,"properties":{"temperature":{"type":"number"}},"required":["temperature"]}),
        )];
        let resolved =
            resolve_tool_call(&tools, "NumberTool", json!({"temperature":"0.5"})).unwrap();
        assert_eq!(resolved.args, json!({"temperature":0.5}));
    }
}
