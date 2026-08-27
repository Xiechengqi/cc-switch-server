use serde_json::Value;

fn fixture(relative: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn cursor_neutral_fixtures_are_versioned_and_non_empty() {
    for path in [
        "tests/fixtures/cursor/responses/continuation_matrix.json",
        "tests/fixtures/cursor/responses/codex_0144_additional_tools.json",
        "tests/fixtures/cursor/retry/semantic_matrix.json",
        "tests/fixtures/cursor/identity/rotation_matrix.json",
    ] {
        let value = fixture(path);
        assert_eq!(value["schemaVersion"], 1, "{path}");
        assert!(!value["cases"].as_array().unwrap().is_empty(), "{path}");
    }
}
