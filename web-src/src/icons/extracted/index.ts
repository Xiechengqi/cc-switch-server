import claude from "./claude.svg?raw";
import anthropic from "./anthropic.svg?raw";
import openai from "./openai.svg?raw";
import gemini from "./gemini.svg?raw";
import deepseek from "./deepseek.svg?raw";
import ollama from "./ollama.svg?raw";
import openrouter from "./openrouter.svg?raw";
import zhipu from "./zhipu.svg?raw";
import qwen from "./qwen.svg?raw";
import alibaba from "./alibaba.svg?raw";
import bailian from "./bailian.svg?raw";
import kimi from "./kimi.svg?raw";
import nvidia from "./nvidia.svg?raw";
import aws from "./aws.svg?raw";
import azure from "./azure.svg?raw";
import google from "./google.svg?raw";
import cloudflare from "./cloudflare.svg?raw";
import mistral from "./mistral.svg?raw";
import cohere from "./cohere.svg?raw";
import perplexity from "./perplexity.svg?raw";
import huggingface from "./huggingface.svg?raw";
import novita from "./novita.svg?raw";
import baidu from "./baidu.svg?raw";
import tencent from "./tencent.svg?raw";
import hunyuan from "./hunyuan.svg?raw";
import minimax from "./minimax.svg?raw";
import xai from "./xai.svg?raw";
import grok from "./grok.svg?raw";
import copilot from "./copilot.svg?raw";
import githubcopilot from "./githubcopilot.svg?raw";
import github from "./github.svg?raw";
import googlecloud from "./googlecloud.svg?raw";
import doubao from "./doubao.svg?raw";
import siliconflow from "./siliconflow.svg?raw";
import stepfun from "./stepfun.svg?raw";
import meta from "./meta.svg?raw";
import huawei from "./huawei.svg?raw";
import newapi from "./newapi.svg?raw";
import subrouter from "./subrouter.svg?raw";
import bytedance from "./bytedance.svg?raw";
import cursorUrl from "./cursor.png";
import kiroUrl from "./kiro.png";

import { IconMetadata, iconMetadata } from "./metadata";

export const icons: Record<string, string> = {
  claude,
  anthropic,
  openai,
  gemini,
  deepseek,
  ollama,
  openrouter,
  zhipu,
  qwen,
  alibaba,
  bailian,
  kimi,
  nvidia,
  aws,
  azure,
  google,
  cloudflare,
  mistral,
  cohere,
  perplexity,
  huggingface,
  novita,
  baidu,
  tencent,
  hunyuan,
  minimax,
  xai,
  grok,
  copilot,
  githubcopilot,
  github,
  googlecloud,
  doubao,
  siliconflow,
  stepfun,
  meta,
  huawei,
  newapi,
  subrouter,
  bytedance,
};

export const iconUrls: Record<string, string> = {
  cursor: cursorUrl,
  kiro: kiroUrl,
};

export function normalizeIconName(icon: string): string {
  return icon.trim().toLowerCase().replace(/[\s_]+/g, "-");
}

export function getIcon(icon: string): string {
  return icons[normalizeIconName(icon)] || "";
}

export function getIconUrl(icon: string): string {
  return iconUrls[normalizeIconName(icon)] || "";
}

export function hasIcon(icon: string): boolean {
  const normalized = normalizeIconName(icon);
  return normalized in icons || normalized in iconUrls;
}

export function isUrlIcon(icon: string): boolean {
  return normalizeIconName(icon) in iconUrls;
}

export function getIconMetadata(icon: string): IconMetadata | undefined {
  return iconMetadata[normalizeIconName(icon)];
}

export { iconMetadata };
