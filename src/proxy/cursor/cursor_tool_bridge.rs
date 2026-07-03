//! Builtin Cursor exec → declared client MCP tool bridging.
//!
//! Centralizes alias tables (composer-api / OmniRoute aligned) so
//! `cursor_agent_service.rs` stays orchestration-only.

use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy)]
pub enum BuiltinBridgeKind {
    Read,
    Delete,
    Ls,
    Fetch,
}

impl BuiltinBridgeKind {
    fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::Read => &["read", "read_file", "readfile", "readtool"],
            Self::Delete => &["delete", "remove"],
            Self::Ls => &["ls", "list_dir", "listdir", "glob"],
            Self::Fetch => &["fetch", "web_fetch", "http", "curl"],
        }
    }
}

pub fn normalize_tool_name(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

pub fn is_declared_tool(declared: &[String], tool_name: &str) -> bool {
    let norm = normalize_tool_name(tool_name);
    declared.iter().any(|d| normalize_tool_name(d) == norm)
}

pub fn resolve_mcp_tool_by_aliases(declared: &[String], aliases: &[&str]) -> Option<String> {
    for name in declared {
        let lower = normalize_tool_name(name);
        if aliases
            .iter()
            .any(|alias| lower == *alias || lower.contains(alias))
        {
            return Some(name.clone());
        }
    }
    None
}

pub fn resolve_shell_mcp_tool_name(declared: &[String]) -> Option<String> {
    const SHELL_ALIASES: &[&str] = &[
        "bash",
        "shell",
        "run_terminal_cmd",
        "run_terminal_command",
        "terminal",
    ];
    resolve_mcp_tool_by_aliases(declared, SHELL_ALIASES).or_else(|| declared.first().cloned())
}

/// Remap Cursor-side MCP tool names/args before surfacing to the client.
pub fn bridge_mcp_exec_tool(
    declared: &[String],
    tool_name: &str,
    args: Value,
) -> Option<(String, Value)> {
    let norm = normalize_tool_name(tool_name);
    if matches!(
        norm.as_str(),
        "semsearch"
            | "semanticsearch"
            | "searchcode"
            | "codesearch"
            | "semantic_code_search"
            | "sem_search"
            | "semantic_search"
    ) {
        return bridge_sem_search_tool(declared, args);
    }
    None
}

pub fn bridge_sem_search_tool(declared: &[String], args: Value) -> Option<(String, Value)> {
    const SEM_ALIASES: &[&str] = &[
        "codebase_search",
        "semantic_search",
        "semsearch",
        "search_code",
    ];
    let name = resolve_mcp_tool_by_aliases(declared, SEM_ALIASES)
        .or_else(|| resolve_mcp_tool_by_aliases(declared, &["grep", "search", "rg"]))?;

    let query = first_string_arg(
        &args,
        &[
            "query",
            "pattern",
            "search",
            "searchQuery",
            "search_query",
            "semanticQuery",
            "semantic_query",
            "prompt",
        ],
    );
    let directories = first_string_array_arg(
        &args,
        &[
            "targetDirectories",
            "target_directories",
            "directories",
            "paths",
            "path",
        ],
    );

    let mut args_map = Map::new();
    if let Some(q) = query {
        args_map.insert("query".into(), Value::String(q.clone()));
        args_map.insert("pattern".into(), Value::String(q));
    }
    if let Some(dirs) = directories {
        if dirs.len() == 1 {
            args_map.insert("path".into(), Value::String(dirs[0].clone()));
        }
        args_map.insert(
            "targetDirectories".into(),
            Value::Array(dirs.into_iter().map(Value::String).collect()),
        );
    }
    if args_map.is_empty() {
        return None;
    }
    Some((name, Value::Object(args_map)))
}

pub fn bridge_builtin_tool(
    kind: BuiltinBridgeKind,
    declared: &[String],
    path: &str,
    command_or_url: &str,
    working_dir: &str,
) -> Option<(String, Value)> {
    let name = resolve_mcp_tool_by_aliases(declared, kind.aliases())?;
    let mut args_map = Map::new();
    match kind {
        BuiltinBridgeKind::Read | BuiltinBridgeKind::Delete => {
            if !path.is_empty() {
                args_map.insert("path".into(), Value::String(path.to_string()));
            }
        }
        BuiltinBridgeKind::Ls => {
            args_map.insert(
                "path".into(),
                Value::String(if path.is_empty() {
                    ".".into()
                } else {
                    path.to_string()
                }),
            );
        }
        BuiltinBridgeKind::Fetch => {
            args_map.insert("url".into(), Value::String(command_or_url.to_string()));
        }
    }
    if !working_dir.is_empty() {
        args_map.insert("workdir".into(), Value::String(working_dir.to_string()));
    }
    Some((name, Value::Object(args_map)))
}

pub fn bridge_read_tool(
    declared: &[String],
    path: &str,
    offset: Option<u64>,
    limit: Option<u64>,
) -> Option<(String, Value)> {
    const READ_ALIASES: &[&str] = &["read", "read_file", "readfile", "readtool"];
    let name = resolve_mcp_tool_by_aliases(declared, READ_ALIASES)?;
    let mut args_map = Map::new();
    if !path.is_empty() {
        args_map.insert("path".into(), Value::String(path.to_string()));
    }
    if let Some(o) = offset {
        args_map.insert("offset".into(), Value::Number(o.into()));
    }
    if let Some(l) = limit {
        args_map.insert("limit".into(), Value::Number(l.into()));
    }
    Some((name, Value::Object(args_map)))
}

pub fn bridge_write_or_edit_tool(
    declared: &[String],
    path: &str,
    file_text: &str,
    stream_content: &str,
) -> Option<(String, Value)> {
    let mut args_map = Map::new();
    if !path.is_empty() {
        args_map.insert("path".into(), Value::String(path.to_string()));
    }
    if !stream_content.is_empty() {
        const EDIT_ALIASES: &[&str] = &["edit", "str_replace", "apply_patch", "write"];
        let name = resolve_mcp_tool_by_aliases(declared, EDIT_ALIASES)?;
        args_map.insert(
            "streamContent".into(),
            Value::String(stream_content.to_string()),
        );
        args_map.insert(
            "stream_content".into(),
            Value::String(stream_content.to_string()),
        );
        return Some((name, Value::Object(args_map)));
    }
    if file_text.is_empty() {
        return None;
    }
    const WRITE_ALIASES: &[&str] = &["write", "write_file", "edit", "apply_patch"];
    let name = resolve_mcp_tool_by_aliases(declared, WRITE_ALIASES)?;
    args_map.insert("fileText".into(), Value::String(file_text.to_string()));
    args_map.insert("contents".into(), Value::String(file_text.to_string()));
    Some((name, Value::Object(args_map)))
}

pub fn bridge_ls_or_glob_tool(declared: &[String], path: &str) -> Option<(String, Value)> {
    let looks_like_glob = path.contains('*') || path.contains('?');
    if looks_like_glob {
        return bridge_glob_tool(declared, path, ".");
    }
    bridge_builtin_tool(BuiltinBridgeKind::Ls, declared, path, "", "")
}

pub fn bridge_glob_tool(
    declared: &[String],
    glob_pattern: &str,
    target_directory: &str,
) -> Option<(String, Value)> {
    const GLOB_ALIASES: &[&str] = &["glob", "glob_file_search", "file_search"];
    let name = resolve_mcp_tool_by_aliases(declared, GLOB_ALIASES)?;
    let mut args_map = Map::new();
    if !glob_pattern.is_empty() {
        args_map.insert(
            "globPattern".into(),
            Value::String(glob_pattern.to_string()),
        );
        args_map.insert("pattern".into(), Value::String(glob_pattern.to_string()));
    }
    let dir = if target_directory.is_empty() {
        "."
    } else {
        target_directory
    };
    args_map.insert("targetDirectory".into(), Value::String(dir.to_string()));
    args_map.insert("path".into(), Value::String(dir.to_string()));
    Some((name, Value::Object(args_map)))
}

pub fn bridge_read_lints_tool(declared: &[String], paths: &[String]) -> Option<(String, Value)> {
    const LINT_ALIASES: &[&str] = &["read_lints", "readlints", "diagnostics", "linter"];
    let name = resolve_mcp_tool_by_aliases(declared, LINT_ALIASES)?;
    let mut args_map = Map::new();
    if !paths.is_empty() {
        args_map.insert(
            "paths".into(),
            Value::Array(paths.iter().cloned().map(Value::String).collect()),
        );
        if paths.len() == 1 {
            args_map.insert("path".into(), Value::String(paths[0].clone()));
        }
    }
    Some((name, Value::Object(args_map)))
}

pub fn bridge_grep_tool(
    declared: &[String],
    pattern: &str,
    path: &str,
    glob: &str,
    output_mode: &str,
    case_insensitive: bool,
    head_limit: Option<u64>,
) -> Option<(String, Value)> {
    const GREP_ALIASES: &[&str] = &["grep", "search", "rg", "ripgrep"];
    let name = resolve_mcp_tool_by_aliases(declared, GREP_ALIASES)?;
    let mut args_map = Map::new();
    if !pattern.is_empty() {
        args_map.insert("pattern".into(), Value::String(pattern.to_string()));
    }
    if !path.is_empty() {
        args_map.insert("path".into(), Value::String(path.to_string()));
    }
    if !glob.is_empty() {
        args_map.insert("glob".into(), Value::String(glob.to_string()));
    }
    if !output_mode.is_empty() {
        args_map.insert("output_mode".into(), Value::String(output_mode.to_string()));
    }
    if case_insensitive {
        args_map.insert("case_insensitive".into(), Value::Bool(true));
    }
    if let Some(limit) = head_limit.filter(|n| *n > 0) {
        args_map.insert("head_limit".into(), Value::Number(limit.into()));
    }
    Some((name, Value::Object(args_map)))
}

fn first_string_arg(args: &Value, keys: &[&str]) -> Option<String> {
    let obj = args.as_object()?;
    for key in keys {
        if let Some(s) = obj.get(*key).and_then(Value::as_str) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn first_string_array_arg(args: &Value, keys: &[&str]) -> Option<Vec<String>> {
    let obj = args.as_object()?;
    for key in keys {
        let Some(v) = obj.get(*key) else { continue };
        if let Some(s) = v.as_str() {
            if !s.is_empty() {
                return Some(vec![s.to_string()]);
            }
        }
        if let Some(arr) = v.as_array() {
            let strings: Vec<String> = arr
                .iter()
                .filter_map(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            if !strings.is_empty() {
                return Some(strings);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn declared(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn sem_search_maps_to_codebase_search() {
        let d = declared(&["codebase_search", "grep"]);
        let args = json!({ "query": "main fn", "targetDirectories": ["src"] });
        let (name, out) = bridge_sem_search_tool(&d, args).expect("bridge");
        assert_eq!(name, "codebase_search");
        assert_eq!(out["query"], "main fn");
    }

    #[test]
    fn sem_search_falls_back_to_grep() {
        let d = declared(&["grep"]);
        let args = json!({ "query": "TODO" });
        let (name, _) = bridge_sem_search_tool(&d, args).expect("bridge");
        assert_eq!(name, "grep");
    }

    #[test]
    fn mcp_exec_remaps_semsearch_name() {
        let d = declared(&["codebase_search"]);
        let args = json!({ "query": "auth" });
        let (name, _) = bridge_mcp_exec_tool(&d, "semSearch", args).expect("remap");
        assert_eq!(name, "codebase_search");
    }

    #[test]
    fn is_declared_tool_case_insensitive() {
        let d = declared(&["Read"]);
        assert!(is_declared_tool(&d, "read"));
        assert!(!is_declared_tool(&d, "write"));
    }
}
