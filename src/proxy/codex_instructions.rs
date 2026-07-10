const INSTRUCTIONS_CODEX: &str = include_str!("codex_instructions/instructions.txt");
const INSTRUCTIONS_GPT5_1: &str = include_str!("codex_instructions/instructions_gpt5_1.txt");
const INSTRUCTIONS_GPT5_2: &str = include_str!("codex_instructions/instructions_gpt5_2.txt");
const INSTRUCTIONS_GPT5_5: &str = include_str!("codex_instructions/instructions_gpt5_5.txt");

pub(crate) fn base_instructions_for_model(model: Option<&str>) -> &'static str {
    let model = model
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if model.contains("codex") {
        return INSTRUCTIONS_CODEX;
    }
    if model.starts_with("gpt-5.2") {
        return INSTRUCTIONS_GPT5_2;
    }
    if model.starts_with("gpt-5.1") {
        return INSTRUCTIONS_GPT5_1;
    }
    INSTRUCTIONS_GPT5_5
}

pub(crate) fn merged_instructions(model: Option<&str>, existing: Option<&str>) -> String {
    let base = base_instructions_for_model(model).trim();
    let existing = existing.map(str::trim).unwrap_or_default();
    if existing.is_empty() {
        base.to_string()
    } else {
        format!("{base}\n\n{existing}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_line(value: &str) -> &str {
        value.trim().lines().next().unwrap_or_default()
    }

    #[test]
    fn selects_codex_instruction_versions_by_model() {
        assert_eq!(
            first_line(base_instructions_for_model(Some("gpt-5-codex"))),
            "You are Codex, based on GPT-5. You are running as a coding agent in the Codex CLI on a user's computer."
        );
        assert_eq!(
            first_line(base_instructions_for_model(Some("gpt-5.2"))),
            "You are GPT-5.2 running in the Codex CLI, a terminal-based coding assistant. Codex CLI is an open source project led by OpenAI. You are expected to be precise, safe, and helpful."
        );
        assert_eq!(
            first_line(base_instructions_for_model(Some("gpt-5.5"))),
            "You are Codex, a coding agent based on GPT-5. You and the user share one workspace, and your job is to collaborate with them until their goal is genuinely handled."
        );
    }

    #[test]
    fn merged_instructions_appends_existing_text() {
        let merged = merged_instructions(Some("gpt-5.5"), Some("Project rule"));
        assert!(merged.contains("Project rule"));
        assert!(merged.contains("\n\nProject rule"));
    }
}
