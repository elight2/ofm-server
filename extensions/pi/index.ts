/**
 * ofm-server 模型列表注入插件
 *
 * 连接 ofm-server（默认 127.0.0.1:3060），每 1 分钟拉取一次模型列表，
 * 并通过动态 provider 注入 pi 的模型列表。
 *
 * 本插件不在启动阶段请求网络：模型列表只由 1 分钟定时刷新（以及
 * `/ofm-refresh` 手动刷新）填充。
 *
 * 环境变量：
 * - `OFM_BASE_URL`   OpenAI 兼容 base url，默认 `http://127.0.0.1:3060/v1`
 * - `OFM_API_KEY`    api key，默认 `ofm`（ofm-server 默认不校验 key）
 * - `OFM_REFRESH_MS` 刷新间隔，默认 60000
 */

import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";

const PROVIDER_ID = "ofm-server";
const DEFAULT_BASE_URL = "http://127.0.0.1:3060/v1";
const DEFAULT_REFRESH_MS = 60_000;
const FETCH_TIMEOUT_MS = 10_000;
/** ofm-server 不提供上下文/输出限制信息，这里给保守默认值。 */
const DEFAULT_CONTEXT_WINDOW = 128_000;
const DEFAULT_MAX_TOKENS = 16_384;

interface ModelConfig {
  id: string;
  name: string;
  reasoning: boolean;
  /** 所有模型一律声明支持文本与图片输入。 */
  input: ("text" | "image")[];
  cost: { input: number; output: number; cacheRead: number; cacheWrite: number };
  contextWindow: number;
  maxTokens: number;
}

function baseUrl(): string {
  return (process.env.OFM_BASE_URL?.trim() || DEFAULT_BASE_URL).replace(/\/+$/, "");
}

function apiKey(): string {
  return process.env.OFM_API_KEY?.trim() || "ofm";
}

function refreshInterval(): number {
  const value = Number(process.env.OFM_REFRESH_MS);
  return Number.isFinite(value) && value >= 5_000 ? value : DEFAULT_REFRESH_MS;
}

function toModelConfig(id: string): ModelConfig {
  return {
    id,
    name: id,
    reasoning: false,
    input: ["text", "image"],
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    contextWindow: DEFAULT_CONTEXT_WINDOW,
    maxTokens: DEFAULT_MAX_TOKENS,
  };
}

/** 短时缓存：pi 启动阶段会连续触发多次刷新，避免重复请求 ofm-server。 */
const CACHE_TTL_MS = 5_000;
let cachedIds: string[] | undefined;
let cachedAt = 0;

/** 拉取 `/v1/models`，返回去重后的模型 id 列表（带短时缓存）。 */
async function fetchModelIds(signal: AbortSignal): Promise<string[]> {
  if (cachedIds && Date.now() - cachedAt < CACHE_TTL_MS) return cachedIds;
  const ids = await fetchModelIdsUncached(signal);
  cachedIds = ids;
  cachedAt = Date.now();
  return ids;
}

async function fetchModelIdsUncached(signal: AbortSignal): Promise<string[]> {
  const controller = new AbortController();
  const onAbort = () => controller.abort();
  const timer = setTimeout(() => controller.abort(), FETCH_TIMEOUT_MS);
  signal.addEventListener("abort", onAbort, { once: true });
  try {
    const response = await fetch(`${baseUrl()}/models`, {
      headers: { Authorization: `Bearer ${apiKey()}` },
      signal: controller.signal,
    });
    if (!response.ok) throw new Error(`HTTP ${response.status} ${response.statusText}`.trim());
    const payload = (await response.json()) as { data?: Array<{ id?: unknown }> };
    if (!Array.isArray(payload.data)) throw new Error("响应缺少 data 数组");
    const ids = new Set<string>();
    for (const entry of payload.data) {
      const id = typeof entry?.id === "string" ? entry.id.trim() : "";
      if (id) ids.add(id);
    }
    return [...ids];
  } finally {
    clearTimeout(timer);
    signal.removeEventListener("abort", onAbort);
  }
}

export default async function (pi: ExtensionAPI) {
  // 启动阶段不拉取模型列表：`models` 省略，列表完全由 `refreshModels`
  // （1 分钟定时刷新 / `/ofm-refresh`）填充。
  pi.registerProvider(PROVIDER_ID, {
    name: "OFM Server",
    baseUrl: baseUrl(),
    apiKey: apiKey(),
    api: "openai-completions",
    // pi 的模型刷新（含本插件的定时刷新）会走这里，返回值替换当前模型列表。
    async refreshModels({ signal }) {
      const ids = await fetchModelIds(signal);
      return ids.map(toModelConfig);
    },
  });

  let timer: ReturnType<typeof setInterval> | undefined;
  let refreshing = false;
  let lastError: string | undefined;

  async function refresh(ctx: ExtensionContext, notify: boolean): Promise<void> {
    if (refreshing) return;
    refreshing = true;
    try {
      const result = await ctx.modelRegistry.refresh({
        allowNetwork: true,
        providers: [PROVIDER_ID],
        force: true,
      });
      const error = result.errors.get(PROVIDER_ID);
      if (error) throw error;
      lastError = undefined;
    } catch (error) {
      const message = errorMessage(error);
      if (message !== lastError) {
        lastError = message;
        if (notify && ctx.hasUI) ctx.ui.notify(`ofm 模型刷新失败: ${message}`, "warning");
        else console.warn(`[ofm] 模型刷新失败: ${message}`);
      }
    } finally {
      refreshing = false;
    }
  }

  function stopTimer(): void {
    if (timer !== undefined) {
      clearInterval(timer);
      timer = undefined;
    }
  }

  pi.on("session_start", async (_event, ctx) => {
    // 不在会话启动时立即同步，避免启动阶段请求网络；只按间隔刷新。
    stopTimer();
    timer = setInterval(() => void refresh(ctx, false), refreshInterval());
  });

  pi.on("session_shutdown", async () => {
    stopTimer();
  });

  pi.registerCommand("ofm-refresh", {
    description: "立即刷新 ofm-server 模型列表",
    handler: async (_args, ctx) => {
      await refresh(ctx, true);
      ctx.ui.notify(`ofm 模型列表已刷新（${ctx.modelRegistry.getAvailable().filter((m) => m.provider === PROVIDER_ID).length} 个）`, "info");
    },
  });
}

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error);
}
