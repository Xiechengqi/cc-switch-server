//! Bounded JSON Schema validation for client-declared Cursor tools.
//!
//! This intentionally implements the assertion subset used by code-agent tool
//! schemas. Remote references and unbounded recursion are rejected: a schema is
//! client input and must never turn tool-call resolution into network I/O or an
//! unbounded CPU/memory operation.

use regex::Regex;
use serde_json::{Map, Value};
use std::collections::{BTreeSet, HashSet};

const MAX_SCHEMA_NODES: usize = 4_096;
const MAX_SCHEMA_DEPTH: usize = 64;
const MAX_INSTANCE_DEPTH: usize = 64;
const MAX_REF_HOPS: usize = 64;
const MAX_PATTERNS: usize = 256;
const MAX_PATTERN_BYTES: usize = 4 * 1024;
const MAX_ARGUMENT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolSchemaErrorKind {
    InvalidSchema,
    ComplexityLimit,
    Validation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSchemaError {
    pub kind: ToolSchemaErrorKind,
    pub path: String,
    pub message: String,
}

impl ToolSchemaError {
    fn schema(message: impl Into<String>) -> Self {
        Self {
            kind: ToolSchemaErrorKind::InvalidSchema,
            path: "$".to_string(),
            message: message.into(),
        }
    }

    fn complexity(message: impl Into<String>) -> Self {
        Self {
            kind: ToolSchemaErrorKind::ComplexityLimit,
            path: "$".to_string(),
            message: message.into(),
        }
    }

    fn validation(path: &str, message: impl Into<String>) -> Self {
        Self {
            kind: ToolSchemaErrorKind::Validation,
            path: path.to_string(),
            message: message.into(),
        }
    }
}

pub fn validate_tool_arguments(schema: &Value, instance: &Value) -> Result<(), ToolSchemaError> {
    if serde_json::to_vec(instance)
        .map(|encoded| encoded.len() > MAX_ARGUMENT_BYTES)
        .unwrap_or(true)
    {
        return Err(ToolSchemaError::complexity(
            "Cursor tool arguments exceed the 1 MiB validation limit",
        ));
    }
    let mut context = ValidationContext {
        root: schema,
        schema_nodes: 0,
        patterns: 0,
        active_refs: HashSet::new(),
    };
    context.validate(schema, instance, "$", 0, 0).map(|_| ())
}

struct ValidationContext<'a> {
    root: &'a Value,
    schema_nodes: usize,
    patterns: usize,
    active_refs: HashSet<String>,
}

impl ValidationContext<'_> {
    fn validate(
        &mut self,
        schema: &Value,
        instance: &Value,
        path: &str,
        schema_depth: usize,
        instance_depth: usize,
    ) -> Result<BTreeSet<String>, ToolSchemaError> {
        self.schema_nodes = self.schema_nodes.saturating_add(1);
        if self.schema_nodes > MAX_SCHEMA_NODES
            || schema_depth > MAX_SCHEMA_DEPTH
            || instance_depth > MAX_INSTANCE_DEPTH
        {
            return Err(ToolSchemaError::complexity(
                "Cursor tool schema exceeds validation complexity limits",
            ));
        }
        match schema {
            Value::Bool(true) => Ok(BTreeSet::new()),
            Value::Bool(false) => Err(ToolSchemaError::validation(
                path,
                "false schema rejects the value",
            )),
            Value::Object(object) => {
                self.validate_object(object, instance, path, schema_depth, instance_depth)
            }
            _ => Err(ToolSchemaError::schema(
                "tool schema nodes must be objects or booleans",
            )),
        }
    }

    fn validate_object(
        &mut self,
        schema: &Map<String, Value>,
        instance: &Value,
        path: &str,
        schema_depth: usize,
        instance_depth: usize,
    ) -> Result<BTreeSet<String>, ToolSchemaError> {
        let mut evaluated = BTreeSet::new();
        if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
            let target = self.resolve_ref(reference)?.clone();
            if self.active_refs.len() >= MAX_REF_HOPS || !self.active_refs.insert(reference.into())
            {
                return Err(ToolSchemaError::complexity(
                    "Cursor tool schema contains a recursive or excessive $ref chain",
                ));
            }
            let result = self.validate(&target, instance, path, schema_depth + 1, instance_depth);
            self.active_refs.remove(reference);
            evaluated.extend(result?);
        }

        if let Some(value) = schema.get("const") {
            if instance != value {
                return Err(ToolSchemaError::validation(
                    path,
                    "value does not match const",
                ));
            }
        }
        if let Some(values) = schema.get("enum") {
            let values = values
                .as_array()
                .ok_or_else(|| ToolSchemaError::schema("enum must be an array"))?;
            if values.is_empty() {
                return Err(ToolSchemaError::schema("enum must not be empty"));
            }
            if !values.iter().any(|value| value == instance) {
                return Err(ToolSchemaError::validation(path, "value is not in enum"));
            }
        }
        self.validate_type(schema.get("type"), instance, path)?;

        if let Some(branches) = schema.get("allOf") {
            for branch in schema_array(branches, "allOf")? {
                evaluated.extend(self.validate(
                    branch,
                    instance,
                    path,
                    schema_depth + 1,
                    instance_depth,
                )?);
            }
        }
        if let Some(branches) = schema.get("anyOf") {
            let branches = schema_array(branches, "anyOf")?;
            if branches.is_empty() {
                return Err(ToolSchemaError::schema("anyOf must not be empty"));
            }
            let mut matched = false;
            let mut branch_evaluated = BTreeSet::new();
            for branch in branches {
                match self.validate(branch, instance, path, schema_depth + 1, instance_depth) {
                    Ok(keys) => {
                        matched = true;
                        branch_evaluated.extend(keys);
                    }
                    Err(error) if error.kind == ToolSchemaErrorKind::Validation => {}
                    Err(error) => return Err(error),
                }
            }
            if !matched {
                return Err(ToolSchemaError::validation(path, "no anyOf branch matched"));
            }
            evaluated.extend(branch_evaluated);
        }
        if let Some(branches) = schema.get("oneOf") {
            let branches = schema_array(branches, "oneOf")?;
            if branches.is_empty() {
                return Err(ToolSchemaError::schema("oneOf must not be empty"));
            }
            let mut matches = 0usize;
            let mut branch_evaluated = BTreeSet::new();
            for branch in branches {
                match self.validate(branch, instance, path, schema_depth + 1, instance_depth) {
                    Ok(keys) => {
                        matches += 1;
                        branch_evaluated.extend(keys);
                    }
                    Err(error) if error.kind == ToolSchemaErrorKind::Validation => {}
                    Err(error) => return Err(error),
                }
            }
            if matches != 1 {
                return Err(ToolSchemaError::validation(
                    path,
                    format!("oneOf requires exactly one match, observed {matches}"),
                ));
            }
            evaluated.extend(branch_evaluated);
        }
        if let Some(negated) = schema.get("not") {
            match self.validate(negated, instance, path, schema_depth + 1, instance_depth) {
                Ok(_) => return Err(ToolSchemaError::validation(path, "not schema matched")),
                Err(error) if error.kind == ToolSchemaErrorKind::Validation => {}
                Err(error) => return Err(error),
            }
        }
        if let Some(condition) = schema.get("if") {
            let condition_matches =
                match self.validate(condition, instance, path, schema_depth + 1, instance_depth) {
                    Ok(_) => true,
                    Err(error) if error.kind == ToolSchemaErrorKind::Validation => false,
                    Err(error) => return Err(error),
                };
            let selected = if condition_matches {
                schema.get("then")
            } else {
                schema.get("else")
            };
            if let Some(selected) = selected {
                evaluated.extend(self.validate(
                    selected,
                    instance,
                    path,
                    schema_depth + 1,
                    instance_depth,
                )?);
            }
        }

        match instance {
            Value::Object(object) => evaluated.extend(self.validate_instance_object(
                schema,
                object,
                path,
                schema_depth,
                instance_depth,
                &evaluated,
            )?),
            Value::Array(array) => {
                self.validate_array(schema, array, path, schema_depth, instance_depth)?
            }
            Value::String(string) => self.validate_string(schema, string, path)?,
            Value::Number(number) => self.validate_number(schema, number, path)?,
            _ => {}
        }
        Ok(evaluated)
    }

    fn validate_instance_object(
        &mut self,
        schema: &Map<String, Value>,
        instance: &Map<String, Value>,
        path: &str,
        schema_depth: usize,
        instance_depth: usize,
        inherited_evaluated: &BTreeSet<String>,
    ) -> Result<BTreeSet<String>, ToolSchemaError> {
        check_count(schema, "minProperties", instance.len(), true, path)?;
        check_count(schema, "maxProperties", instance.len(), false, path)?;
        if let Some(required) = schema.get("required") {
            for name in string_array(required, "required")? {
                if !instance.contains_key(name) {
                    return Err(ToolSchemaError::validation(
                        path,
                        format!("required property `{name}` is missing"),
                    ));
                }
            }
        }

        let properties = optional_object(schema.get("properties"), "properties")?;
        let pattern_properties =
            optional_object(schema.get("patternProperties"), "patternProperties")?;
        let mut evaluated = inherited_evaluated.clone();
        let mut compiled_patterns = Vec::new();
        for (pattern, child) in pattern_properties {
            if pattern.len() > MAX_PATTERN_BYTES {
                return Err(ToolSchemaError::complexity(
                    "tool schema pattern exceeds 4096 bytes",
                ));
            }
            self.patterns = self.patterns.saturating_add(1);
            if self.patterns > MAX_PATTERNS {
                return Err(ToolSchemaError::complexity("too many tool schema patterns"));
            }
            compiled_patterns.push((
                Regex::new(pattern)
                    .map_err(|_| ToolSchemaError::schema("invalid patternProperties regex"))?,
                child,
            ));
        }

        for (name, value) in instance {
            let child_path = property_path(path, name);
            let mut matched = false;
            if let Some(child) = properties.get(name) {
                self.validate(
                    child,
                    value,
                    &child_path,
                    schema_depth + 1,
                    instance_depth + 1,
                )?;
                matched = true;
            }
            for (pattern, child) in &compiled_patterns {
                if pattern.is_match(name) {
                    self.validate(
                        child,
                        value,
                        &child_path,
                        schema_depth + 1,
                        instance_depth + 1,
                    )?;
                    matched = true;
                }
            }
            if matched {
                evaluated.insert(name.clone());
            }
        }

        if let Some(property_names) = schema.get("propertyNames") {
            for name in instance.keys() {
                self.validate(
                    property_names,
                    &Value::String(name.clone()),
                    &property_path(path, name),
                    schema_depth + 1,
                    instance_depth + 1,
                )?;
            }
        }
        if let Some(dependencies) = schema.get("dependentRequired") {
            for (name, required) in required_object(dependencies, "dependentRequired")? {
                if instance.contains_key(name) {
                    for dependent in string_array(required, "dependentRequired value")? {
                        if !instance.contains_key(dependent) {
                            return Err(ToolSchemaError::validation(
                                path,
                                format!("property `{name}` requires `{dependent}`"),
                            ));
                        }
                    }
                }
            }
        }
        if let Some(dependencies) = schema.get("dependentSchemas") {
            for (name, dependent) in required_object(dependencies, "dependentSchemas")? {
                if instance.contains_key(name) {
                    evaluated.extend(self.validate(
                        dependent,
                        &Value::Object(instance.clone()),
                        path,
                        schema_depth + 1,
                        instance_depth,
                    )?);
                }
            }
        }

        self.validate_unevaluated_properties(
            schema,
            instance,
            path,
            schema_depth,
            instance_depth,
            &mut evaluated,
        )?;
        Ok(evaluated)
    }

    fn validate_unevaluated_properties(
        &mut self,
        schema: &Map<String, Value>,
        instance: &Map<String, Value>,
        path: &str,
        schema_depth: usize,
        instance_depth: usize,
        evaluated: &mut BTreeSet<String>,
    ) -> Result<(), ToolSchemaError> {
        if let Some(additional) = schema.get("additionalProperties") {
            let pending = instance
                .iter()
                .filter(|(name, _)| !evaluated.contains(*name))
                .map(|(name, value)| (name.clone(), value))
                .collect::<Vec<_>>();
            for (name, value) in pending {
                self.validate(
                    additional,
                    value,
                    &property_path(path, &name),
                    schema_depth + 1,
                    instance_depth + 1,
                )?;
                evaluated.insert(name);
            }
        }
        if let Some(unevaluated) = schema.get("unevaluatedProperties") {
            for (name, value) in instance
                .iter()
                .filter(|(name, _)| !evaluated.contains(*name))
            {
                self.validate(
                    unevaluated,
                    value,
                    &property_path(path, name),
                    schema_depth + 1,
                    instance_depth + 1,
                )?;
            }
        }
        Ok(())
    }

    fn validate_array(
        &mut self,
        schema: &Map<String, Value>,
        instance: &[Value],
        path: &str,
        schema_depth: usize,
        instance_depth: usize,
    ) -> Result<(), ToolSchemaError> {
        check_count(schema, "minItems", instance.len(), true, path)?;
        check_count(schema, "maxItems", instance.len(), false, path)?;
        if schema.get("uniqueItems").and_then(Value::as_bool) == Some(true) {
            let mut seen = HashSet::with_capacity(instance.len());
            for item in instance {
                let key = canonical_json_bytes(item)?;
                if !seen.insert(key) {
                    return Err(ToolSchemaError::validation(
                        path,
                        "array items are not unique",
                    ));
                }
            }
        }

        let prefix = match schema.get("prefixItems") {
            Some(value) => schema_array(value, "prefixItems")?,
            None => &[],
        };
        for (index, child) in prefix.iter().enumerate().take(instance.len()) {
            self.validate(
                child,
                &instance[index],
                &format!("{path}[{index}]"),
                schema_depth + 1,
                instance_depth + 1,
            )?;
        }
        if let Some(items) = schema.get("items") {
            match items {
                Value::Array(tuple) => {
                    for (index, child) in tuple.iter().enumerate().take(instance.len()) {
                        self.validate(
                            child,
                            &instance[index],
                            &format!("{path}[{index}]"),
                            schema_depth + 1,
                            instance_depth + 1,
                        )?;
                    }
                }
                _ => {
                    for (index, value) in instance.iter().enumerate().skip(prefix.len()) {
                        self.validate(
                            items,
                            value,
                            &format!("{path}[{index}]"),
                            schema_depth + 1,
                            instance_depth + 1,
                        )?;
                    }
                }
            }
        }
        if let Some(contains) = schema.get("contains") {
            let mut matches = 0usize;
            for (index, value) in instance.iter().enumerate() {
                match self.validate(
                    contains,
                    value,
                    &format!("{path}[{index}]"),
                    schema_depth + 1,
                    instance_depth + 1,
                ) {
                    Ok(_) => matches += 1,
                    Err(error) if error.kind == ToolSchemaErrorKind::Validation => {}
                    Err(error) => return Err(error),
                }
            }
            let minimum = schema
                .get("minContains")
                .and_then(Value::as_u64)
                .unwrap_or(1) as usize;
            let maximum = schema
                .get("maxContains")
                .and_then(Value::as_u64)
                .map(|value| value as usize);
            if matches < minimum || maximum.is_some_and(|maximum| matches > maximum) {
                return Err(ToolSchemaError::validation(
                    path,
                    "contains count is outside bounds",
                ));
            }
        }
        Ok(())
    }

    fn validate_string(
        &mut self,
        schema: &Map<String, Value>,
        instance: &str,
        path: &str,
    ) -> Result<(), ToolSchemaError> {
        let length = instance.chars().count();
        check_count(schema, "minLength", length, true, path)?;
        check_count(schema, "maxLength", length, false, path)?;
        if let Some(pattern) = schema.get("pattern") {
            let pattern = pattern
                .as_str()
                .ok_or_else(|| ToolSchemaError::schema("pattern must be a string"))?;
            if pattern.len() > MAX_PATTERN_BYTES {
                return Err(ToolSchemaError::complexity(
                    "tool schema pattern exceeds 4096 bytes",
                ));
            }
            self.patterns = self.patterns.saturating_add(1);
            if self.patterns > MAX_PATTERNS {
                return Err(ToolSchemaError::complexity("too many tool schema patterns"));
            }
            let regex = Regex::new(pattern)
                .map_err(|_| ToolSchemaError::schema("invalid tool schema regex"))?;
            if !regex.is_match(instance) {
                return Err(ToolSchemaError::validation(
                    path,
                    "string does not match pattern",
                ));
            }
        }
        Ok(())
    }

    fn validate_number(
        &self,
        schema: &Map<String, Value>,
        instance: &serde_json::Number,
        path: &str,
    ) -> Result<(), ToolSchemaError> {
        let Some(number) = instance.as_f64().filter(|value| value.is_finite()) else {
            return Err(ToolSchemaError::validation(path, "number is not finite"));
        };
        for (key, inclusive, lower) in [
            ("minimum", true, true),
            ("exclusiveMinimum", false, true),
            ("maximum", true, false),
            ("exclusiveMaximum", false, false),
        ] {
            let Some(bound) = schema.get(key) else {
                continue;
            };
            let bound = bound
                .as_f64()
                .ok_or_else(|| ToolSchemaError::schema(format!("{key} must be numeric")))?;
            let valid = match (lower, inclusive) {
                (true, true) => number >= bound,
                (true, false) => number > bound,
                (false, true) => number <= bound,
                (false, false) => number < bound,
            };
            if !valid {
                return Err(ToolSchemaError::validation(
                    path,
                    format!("number violates {key}"),
                ));
            }
        }
        if let Some(multiple) = schema.get("multipleOf") {
            let multiple = multiple
                .as_f64()
                .filter(|value| value.is_finite() && *value > 0.0)
                .ok_or_else(|| ToolSchemaError::schema("multipleOf must be positive"))?;
            let quotient = number / multiple;
            if (quotient - quotient.round()).abs() > 1e-9 * quotient.abs().max(1.0) {
                return Err(ToolSchemaError::validation(
                    path,
                    "number violates multipleOf",
                ));
            }
        }
        Ok(())
    }

    fn validate_type(
        &self,
        schema_type: Option<&Value>,
        instance: &Value,
        path: &str,
    ) -> Result<(), ToolSchemaError> {
        let Some(schema_type) = schema_type else {
            return Ok(());
        };
        let matches = match schema_type {
            Value::String(value) => {
                ensure_known_type(value)?;
                type_matches(instance, value)
            }
            Value::Array(values) => {
                if values.iter().any(|value| !value.is_string()) {
                    return Err(ToolSchemaError::schema("type array must contain strings"));
                }
                let values = values.iter().filter_map(Value::as_str).collect::<Vec<_>>();
                if values.is_empty() {
                    return Err(ToolSchemaError::schema("type array must not be empty"));
                }
                for value in &values {
                    ensure_known_type(value)?;
                }
                values.iter().any(|value| type_matches(instance, value))
            }
            _ => {
                return Err(ToolSchemaError::schema(
                    "type must be a string or string array",
                ))
            }
        };
        if matches {
            Ok(())
        } else {
            Err(ToolSchemaError::validation(
                path,
                "value has the wrong JSON type",
            ))
        }
    }

    fn resolve_ref(&self, reference: &str) -> Result<&Value, ToolSchemaError> {
        if reference == "#" {
            return Ok(self.root);
        }
        let pointer = reference
            .strip_prefix('#')
            .filter(|pointer| pointer.starts_with('/'))
            .ok_or_else(|| ToolSchemaError::schema("only local JSON Pointer $ref is supported"))?;
        self.root
            .pointer(pointer)
            .ok_or_else(|| ToolSchemaError::schema("local $ref target does not exist"))
    }
}

fn schema_array<'a>(value: &'a Value, name: &str) -> Result<&'a [Value], ToolSchemaError> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| ToolSchemaError::schema(format!("{name} must be an array")))
}

fn optional_object<'a>(
    value: Option<&'a Value>,
    name: &str,
) -> Result<&'a Map<String, Value>, ToolSchemaError> {
    match value {
        Some(value) => value
            .as_object()
            .ok_or_else(|| ToolSchemaError::schema(format!("{name} must be an object"))),
        None => Ok(empty_object()),
    }
}

fn required_object<'a>(
    value: &'a Value,
    name: &str,
) -> Result<&'a Map<String, Value>, ToolSchemaError> {
    value
        .as_object()
        .ok_or_else(|| ToolSchemaError::schema(format!("{name} must be an object")))
}

fn empty_object() -> &'static Map<String, Value> {
    static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(Map::new)
}

fn string_array<'a>(value: &'a Value, name: &str) -> Result<Vec<&'a str>, ToolSchemaError> {
    let values = value
        .as_array()
        .ok_or_else(|| ToolSchemaError::schema(format!("{name} must be an array")))?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| ToolSchemaError::schema(format!("{name} must contain strings")))
        })
        .collect()
}

fn check_count(
    schema: &Map<String, Value>,
    key: &str,
    observed: usize,
    minimum: bool,
    path: &str,
) -> Result<(), ToolSchemaError> {
    let Some(limit) = schema.get(key) else {
        return Ok(());
    };
    let limit = limit
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| ToolSchemaError::schema(format!("{key} must be a non-negative integer")))?;
    if (minimum && observed < limit) || (!minimum && observed > limit) {
        return Err(ToolSchemaError::validation(
            path,
            format!("value violates {key}"),
        ));
    }
    Ok(())
}

fn type_matches(instance: &Value, expected: &str) -> bool {
    match expected {
        "null" => instance.is_null(),
        "boolean" => instance.is_boolean(),
        "object" => instance.is_object(),
        "array" => instance.is_array(),
        "number" => instance.is_number(),
        "integer" => {
            instance.as_i64().is_some()
                || instance.as_u64().is_some()
                || instance
                    .as_f64()
                    .is_some_and(|number| number.is_finite() && number.fract() == 0.0)
        }
        "string" => instance.is_string(),
        _ => false,
    }
}

fn ensure_known_type(value: &str) -> Result<(), ToolSchemaError> {
    matches!(
        value,
        "null" | "boolean" | "object" | "array" | "number" | "integer" | "string"
    )
    .then_some(())
    .ok_or_else(|| ToolSchemaError::schema(format!("unsupported JSON Schema type `{value}`")))
}

fn canonical_json_bytes(value: &Value) -> Result<Vec<u8>, ToolSchemaError> {
    fn canonicalize(value: &Value) -> Value {
        match value {
            Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
            Value::Object(object) => {
                let mut keys = object.keys().collect::<Vec<_>>();
                keys.sort_unstable();
                let mut canonical = Map::with_capacity(object.len());
                for key in keys {
                    canonical.insert(key.clone(), canonicalize(&object[key]));
                }
                Value::Object(canonical)
            }
            value => value.clone(),
        }
    }

    serde_json::to_vec(&canonicalize(value))
        .map_err(|_| ToolSchemaError::schema("tool argument canonicalization failed"))
}

fn property_path(path: &str, property: &str) -> String {
    format!("{path}.{}", property.replace('.', "\\."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn one_of_requires_exactly_one_match() {
        let schema = json!({"oneOf":[{"type":"number"},{"minimum":0}]});
        assert!(validate_tool_arguments(&schema, &json!(-1)).is_ok());
        assert!(validate_tool_arguments(&schema, &json!(1)).is_err());
    }

    #[test]
    fn validates_nested_objects_arrays_conditions_and_dependencies() {
        let schema = json!({
            "type":"object",
            "properties":{
                "mode":{"enum":["read","write"]},
                "paths":{"type":"array","minItems":1,"items":{"type":"string","minLength":1}},
                "content":{"type":"string"}
            },
            "required":["mode","paths"],
            "dependentRequired":{"content":["mode"]},
            "if":{"properties":{"mode":{"const":"write"}}},
            "then":{"required":["content"]},
            "additionalProperties":false
        });
        assert!(validate_tool_arguments(&schema, &json!({"mode":"read","paths":["src"]})).is_ok());
        assert!(
            validate_tool_arguments(&schema, &json!({"mode":"write","paths":["src"]})).is_err()
        );
        assert!(validate_tool_arguments(
            &schema,
            &json!({"mode":"read","paths":["src"],"extra":true})
        )
        .is_err());
    }

    #[test]
    fn unique_items_is_order_independent_for_object_properties() {
        let schema = json!({"type":"array", "uniqueItems":true});
        let instance = json!([
            {"first":1, "second":2},
            {"second":2, "first":1}
        ]);
        assert_eq!(
            validate_tool_arguments(&schema, &instance)
                .unwrap_err()
                .kind,
            ToolSchemaErrorKind::Validation
        );
    }

    #[test]
    fn resolves_local_refs_and_rejects_remote_or_recursive_refs() {
        let schema = json!({
            "$defs":{"path":{"type":"string","pattern":"^.+$"}},
            "type":"object",
            "properties":{"path":{"$ref":"#/$defs/path"}},
            "required":["path"]
        });
        assert!(validate_tool_arguments(&schema, &json!({"path":"src/main.rs"})).is_ok());
        assert!(
            validate_tool_arguments(&json!({"$ref":"https://example/schema"}), &json!({})).is_err()
        );
        assert!(validate_tool_arguments(&json!({"$ref":"#"}), &json!({})).is_err());
    }

    #[test]
    fn invalid_branch_schemas_are_not_hidden_as_non_matches() {
        let error = validate_tool_arguments(
            &json!({"anyOf":[{"type":"string"},{"type":"not-a-json-type"}]}),
            &json!("matches-first"),
        )
        .unwrap_err();
        assert_eq!(error.kind, ToolSchemaErrorKind::InvalidSchema);

        assert!(validate_tool_arguments(&json!({"type":"integer"}), &json!(1.0)).is_ok());
        assert!(validate_tool_arguments(&json!({"type":"integer"}), &json!(1.5)).is_err());
    }

    #[test]
    fn supports_pattern_and_unevaluated_properties() {
        let schema = json!({
            "type":"object",
            "patternProperties":{"^x-":{"type":"string"}},
            "unevaluatedProperties":false
        });
        assert!(validate_tool_arguments(&schema, &json!({"x-name":"ok"})).is_ok());
        assert!(validate_tool_arguments(&schema, &json!({"other":"no"})).is_err());
    }
}
