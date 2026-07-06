import { useState, useCallback, useEffect, useRef } from "react";

interface UseModelStateProps {
  settingsConfig: string;
  onConfigChange: (config: string) => void;
}

export type ClaudeModelEnvField =
  | "ANTHROPIC_MODEL"
  | "ANTHROPIC_DEFAULT_HAIKU_MODEL"
  | "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME"
  | "ANTHROPIC_DEFAULT_SONNET_MODEL"
  | "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME"
  | "ANTHROPIC_DEFAULT_OPUS_MODEL"
  | "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME"
  | "ANTHROPIC_DEFAULT_FABLE_MODEL"
  | "ANTHROPIC_DEFAULT_FABLE_MODEL_NAME"
  | "MODEL_MAPPING_SINGLE_UPSTREAM";

export const CLAUDE_ONE_M_MARKER = "[1M]";

export function hasClaudeOneMMarker(model: string): boolean {
  return model.trimEnd().toLowerCase().endsWith("[1m]");
}

export function stripClaudeOneMMarker(model: string): string {
  const trimmedEnd = model.trimEnd();
  if (!trimmedEnd.toLowerCase().endsWith("[1m]")) return model;
  return trimmedEnd.slice(0, -CLAUDE_ONE_M_MARKER.length).trimEnd();
}

export function setClaudeOneMMarker(model: string, enabled: boolean): string {
  const base = stripClaudeOneMMarker(model).trim();
  if (!base) return "";
  return enabled ? `${base}${CLAUDE_ONE_M_MARKER}` : base;
}

/**
 * Parse model values from settings config JSON
 */
function parseModelsFromConfig(settingsConfig: string) {
  try {
    const cfg = settingsConfig ? JSON.parse(settingsConfig) : {};
    const env = cfg?.env || {};
    const mapping = cfg?.modelMapping || {};
    const singleUpstreamModel =
      mapping?.mode === "single" && typeof mapping?.upstreamModel === "string"
        ? mapping.upstreamModel
        : "";
    const model =
      typeof env.ANTHROPIC_MODEL === "string" ? env.ANTHROPIC_MODEL : "";
    const small =
      typeof env.ANTHROPIC_SMALL_FAST_MODEL === "string"
        ? env.ANTHROPIC_SMALL_FAST_MODEL
        : "";
    const haiku =
      typeof env.ANTHROPIC_DEFAULT_HAIKU_MODEL === "string"
        ? env.ANTHROPIC_DEFAULT_HAIKU_MODEL
        : small || model;
    const haikuName =
      typeof env.ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME === "string"
        ? env.ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME
        : stripClaudeOneMMarker(haiku);
    const sonnet =
      typeof env.ANTHROPIC_DEFAULT_SONNET_MODEL === "string"
        ? env.ANTHROPIC_DEFAULT_SONNET_MODEL
        : model || small;
    const sonnetName =
      typeof env.ANTHROPIC_DEFAULT_SONNET_MODEL_NAME === "string"
        ? env.ANTHROPIC_DEFAULT_SONNET_MODEL_NAME
        : stripClaudeOneMMarker(sonnet);
    const opus =
      typeof env.ANTHROPIC_DEFAULT_OPUS_MODEL === "string"
        ? env.ANTHROPIC_DEFAULT_OPUS_MODEL
        : model || small;
    const opusName =
      typeof env.ANTHROPIC_DEFAULT_OPUS_MODEL_NAME === "string"
        ? env.ANTHROPIC_DEFAULT_OPUS_MODEL_NAME
        : stripClaudeOneMMarker(opus);
    // 回填链镜像运行时映射链（fable → opus → default），保证 UI 展示
    // 与代理实际转发的模型一致。
    const fable =
      typeof env.ANTHROPIC_DEFAULT_FABLE_MODEL === "string"
        ? env.ANTHROPIC_DEFAULT_FABLE_MODEL
        : opus;
    const fableName =
      typeof env.ANTHROPIC_DEFAULT_FABLE_MODEL_NAME === "string"
        ? env.ANTHROPIC_DEFAULT_FABLE_MODEL_NAME
        : stripClaudeOneMMarker(fable);

    return {
      model,
      singleUpstreamModel,
      haiku,
      haikuName,
      sonnet,
      sonnetName,
      opus,
      opusName,
      fable,
      fableName,
    };
  } catch {
    return {
      model: "",
      singleUpstreamModel: "",
      haiku: "",
      haikuName: "",
      sonnet: "",
      sonnetName: "",
      opus: "",
      opusName: "",
      fable: "",
      fableName: "",
    };
  }
}

/**
 * 管理模型选择状态
 * 支持 ANTHROPIC_MODEL 和各类型默认模型
 */
export function useModelState({
  settingsConfig,
  onConfigChange,
}: UseModelStateProps) {
  const initial = useState(() => parseModelsFromConfig(settingsConfig))[0];
  const [claudeModel, setClaudeModel] = useState(initial.model);
  const [singleUpstreamModel, setSingleUpstreamModel] = useState(
    initial.singleUpstreamModel,
  );
  const [defaultHaikuModel, setDefaultHaikuModel] = useState(initial.haiku);
  const [defaultHaikuModelName, setDefaultHaikuModelName] = useState(
    initial.haikuName,
  );
  const [defaultSonnetModel, setDefaultSonnetModel] = useState(initial.sonnet);
  const [defaultSonnetModelName, setDefaultSonnetModelName] = useState(
    initial.sonnetName,
  );
  const [defaultOpusModel, setDefaultOpusModel] = useState(initial.opus);
  const [defaultOpusModelName, setDefaultOpusModelName] = useState(
    initial.opusName,
  );
  const [defaultFableModel, setDefaultFableModel] = useState(initial.fable);
  const [defaultFableModelName, setDefaultFableModelName] = useState(
    initial.fableName,
  );

  const isUserEditingRef = useRef(false);
  const lastConfigRef = useRef(settingsConfig);
  const latestConfigRef = useRef(settingsConfig);

  latestConfigRef.current = settingsConfig;

  // 仅在 settingsConfig 外部变化时同步（表单加载 / 切换预设）；
  // 用户正在编辑时 (isUserEditingRef) 跳过一次以避免回填覆盖。
  useEffect(() => {
    if (lastConfigRef.current === settingsConfig) {
      return;
    }
    if (isUserEditingRef.current) {
      isUserEditingRef.current = false;
      lastConfigRef.current = settingsConfig;
      return;
    }
    lastConfigRef.current = settingsConfig;

    const parsed = parseModelsFromConfig(settingsConfig);
    setClaudeModel(parsed.model);
    setSingleUpstreamModel(parsed.singleUpstreamModel);
    setDefaultHaikuModel(parsed.haiku);
    setDefaultHaikuModelName(parsed.haikuName);
    setDefaultSonnetModel(parsed.sonnet);
    setDefaultSonnetModelName(parsed.sonnetName);
    setDefaultOpusModel(parsed.opus);
    setDefaultOpusModelName(parsed.opusName);
    setDefaultFableModel(parsed.fable);
    setDefaultFableModelName(parsed.fableName);
  }, [settingsConfig]);

  const handleModelChange = useCallback(
    (field: ClaudeModelEnvField, value: string) => {
      isUserEditingRef.current = true;

      if (field === "ANTHROPIC_MODEL") setClaudeModel(value);
      if (field === "MODEL_MAPPING_SINGLE_UPSTREAM") {
        setSingleUpstreamModel(value);
        setClaudeModel(value);
        setDefaultHaikuModel(value);
        setDefaultSonnetModel(value);
        setDefaultOpusModel(value);
        setDefaultFableModel(value);
        setDefaultHaikuModelName(stripClaudeOneMMarker(value));
        setDefaultSonnetModelName(stripClaudeOneMMarker(value));
        setDefaultOpusModelName(stripClaudeOneMMarker(value));
        setDefaultFableModelName(stripClaudeOneMMarker(value));
      }
      if (field === "ANTHROPIC_DEFAULT_HAIKU_MODEL")
        setDefaultHaikuModel(value);
      if (field === "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME")
        setDefaultHaikuModelName(value);
      if (field === "ANTHROPIC_DEFAULT_SONNET_MODEL")
        setDefaultSonnetModel(value);
      if (field === "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME")
        setDefaultSonnetModelName(value);
      if (field === "ANTHROPIC_DEFAULT_OPUS_MODEL") setDefaultOpusModel(value);
      if (field === "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME")
        setDefaultOpusModelName(value);
      if (field === "ANTHROPIC_DEFAULT_FABLE_MODEL")
        setDefaultFableModel(value);
      if (field === "ANTHROPIC_DEFAULT_FABLE_MODEL_NAME")
        setDefaultFableModelName(value);

      try {
        const currentConfig = latestConfigRef.current
          ? JSON.parse(latestConfigRef.current)
          : { env: {} };
        if (!currentConfig.env) currentConfig.env = {};
        const env = currentConfig.env as Record<string, unknown>;

        const trimmed = value.trim();
        if (field === "MODEL_MAPPING_SINGLE_UPSTREAM") {
          const roleValue = trimmed;
          if (roleValue) {
            currentConfig.modelMapping = {
              mode: "single",
              upstreamModel: roleValue,
            };
            env["ANTHROPIC_MODEL"] = roleValue;
            env["ANTHROPIC_DEFAULT_HAIKU_MODEL"] =
              stripClaudeOneMMarker(roleValue);
            env["ANTHROPIC_DEFAULT_SONNET_MODEL"] = roleValue;
            env["ANTHROPIC_DEFAULT_OPUS_MODEL"] = roleValue;
            env["ANTHROPIC_DEFAULT_FABLE_MODEL"] = roleValue;
            env["ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME"] =
              stripClaudeOneMMarker(roleValue);
            env["ANTHROPIC_DEFAULT_SONNET_MODEL_NAME"] =
              stripClaudeOneMMarker(roleValue);
            env["ANTHROPIC_DEFAULT_OPUS_MODEL_NAME"] =
              stripClaudeOneMMarker(roleValue);
            env["ANTHROPIC_DEFAULT_FABLE_MODEL_NAME"] =
              stripClaudeOneMMarker(roleValue);
          } else {
            delete currentConfig.modelMapping;
          }
        } else if (trimmed) {
          if (!field.endsWith("_NAME")) {
            delete currentConfig.modelMapping;
          }
          env[field] = trimmed;
        } else {
          if (!field.endsWith("_NAME")) {
            delete currentConfig.modelMapping;
          }
          delete env[field];
        }
        // 删除旧键
        delete env["ANTHROPIC_SMALL_FAST_MODEL"];

        const updatedConfig = JSON.stringify(currentConfig, null, 2);
        latestConfigRef.current = updatedConfig;
        onConfigChange(updatedConfig);
      } catch (err) {
        console.error("Failed to update model config:", err);
      }
    },
    [onConfigChange],
  );

  return {
    claudeModel,
    setClaudeModel,
    singleUpstreamModel,
    setSingleUpstreamModel,
    defaultHaikuModel,
    setDefaultHaikuModel,
    defaultHaikuModelName,
    setDefaultHaikuModelName,
    defaultSonnetModel,
    setDefaultSonnetModel,
    defaultSonnetModelName,
    setDefaultSonnetModelName,
    defaultOpusModel,
    setDefaultOpusModel,
    defaultOpusModelName,
    setDefaultOpusModelName,
    defaultFableModel,
    setDefaultFableModel,
    defaultFableModelName,
    setDefaultFableModelName,
    handleModelChange,
  };
}
