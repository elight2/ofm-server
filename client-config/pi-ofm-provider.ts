/**
 * ofm-server provider 扩展（生产版）。
 *
 * 设计目标：
 * 1. 模型清单由 ofm-server 的 `GET /v1/models` 动态提供，不写死。
 * 2. 走 pi 官方模型发现机制：`refreshModels` + 持久化，而不是在扩展工厂里
 *    做一次性拉取——后者只覆盖启动那一刻，且不落盘、离线不可用。
 * 3. 离线/未联网时用上次持久化的快照，保证 `pi --list-models` 可用。
 *
 * pi 的刷新时机（读源码确认）：
 * - 交互式启动后异步刷新一次（`interactive-mode.js` 的 `refreshModelCatalogs`，
 *   15s 超时，不阻塞 UI）。
 * - `/model` 打开选择器时再刷新（`model-selector.js`）。
 * - `--print` / `--list-models` 等非交互路径以 `allowNetwork: false` 初始化，
 *   不会联网；此时读到的是 `models-store.json` 里的持久化快照。
 *
 * 因此：**首次必须用交互式模式跑一次**，把清单落盘。之后 print/list 也能用。
 *
 * 配置：服务地址可用环境变量 `OFM_SERVER_BASE_URL` 覆盖，默认
 * `http://127.0.0.1:3060/v1`。
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { ProviderModelConfig } from "@earendil-works/pi-ai";

const PROVIDER_ID = "ofm-server";
const DEFAULT_BASE_URL = "http://127.0.0.1:3060/v1";
/** 快照新鲜度窗口：窗口内不重复请求。 */
const REFRESH_TTL_MS = 5 * 60 * 1000;
const FETCH_TIMEOUT_MS = 10_000;

function baseUrl(): string {
	return (process.env.OFM_SERVER_BASE_URL ?? DEFAULT_BASE_URL).replace(/\/+$/, "");
}

interface OpenAiModelCard {
	id: string;
	owned_by?: string;
}

function toConfig(id: string): ProviderModelConfig {
	return {
		id,
		name: id,
		api: "openai-completions",
		reasoning: true,
		input: ["text"],
		cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		contextWindow: 128_000,
		maxTokens: 16_384,
	};
}

/** 拉取 `GET /v1/models`。失败抛错，由调用方决定降级策略。 */
async function fetchModels(signal: AbortSignal): Promise<ProviderModelConfig[]> {
	const url = `${baseUrl()}/models`;
	const resp = await fetch(url, {
		headers: { authorization: "Bearer dummy" },
		signal: AbortSignal.any([signal, AbortSignal.timeout(FETCH_TIMEOUT_MS)]),
	});
	if (!resp.ok) throw new Error(`GET ${url} 返回 HTTP ${resp.status}`);

	const body = (await resp.json()) as { data?: OpenAiModelCard[] };
	const cards = body.data ?? [];
	// 去重并保留稳定顺序，避免上游顺序抖动导致每次都判定「已变化」。
	const ids = [...new Set(cards.map((c) => c.id).filter(Boolean))].sort();
	return ids.map(toConfig);
}

export default function (pi: ExtensionAPI) {
	/** 内存快照；持久化由 pi 通过 context.publish({ persist }) 负责。 */
	let current: ProviderModelConfig[] = [];

	pi.registerProvider(PROVIDER_ID, {
		name: "ofm-server",
		baseUrl: baseUrl(),
		api: "openai-completions",
		apiKey: "dummy",
		models: [],

		// 注意：扩展的 refreshModels 必须 **返回模型数组**——
		// pi 在 provider-composer 里用的是返回值（`refreshed = await ...`），
		// 而不是 context.publish 的副作用。返回空数组等于「无模型」。
		refreshModels: async (context) => {
			// 1) 恢复持久化快照。非交互路径（--print/--list-models）只走这一步。
			const stored = context.stored?.models as ProviderModelConfig[] | undefined;
			if (stored?.length) current = stored;

			// 2) 不联网的初始化阶段到此为止。
			if (!context.allowNetwork || context.signal.aborted) return current;

			// 3) 新鲜度窗口内跳过网络请求（force 可绕过）。
			const checkedAt = context.stored?.checkedAt;
			if (
				!context.force &&
				checkedAt !== undefined &&
				Date.now() - checkedAt < REFRESH_TTL_MS &&
				current.length > 0
			) {
				return current;
			}

			// 4) 联网拉取；失败时保留既有快照，不把清单清空。
			try {
				const fresh = await fetchModels(context.signal);
				if (context.signal.aborted || fresh.length === 0) return current;

				current = fresh;
				await context.publish({
					update: () => {},
					persist: { models: fresh, checkedAt: Date.now() },
				});
				return current;
			} catch (error) {
				const reason = error instanceof Error ? error.message : String(error);
				if (current.length > 0) {
					console.error(`[ofm-server] 刷新模型失败，沿用缓存: ${reason}`);
				} else {
					console.error(`[ofm-server] 刷新模型失败: ${reason}`);
				}
				return current;
			}
		},
	});
}
