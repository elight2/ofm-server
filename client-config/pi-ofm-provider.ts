/**
 * ofm-server provider：通过 OpenAI 兼容端点接入。
 *
 * 模型清单在扩展加载时从 ofm-server 的 /v1/models 拉取——
 * pi 的 --list-models 与 print 模式都以 allowNetwork:false 初始化，
 * 不会主动调 refreshModels，因此不能用纯 refreshModels 方案。
 * 扩展工厂是 async，pi 启动时会等待它完成。
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { ProviderModelConfig } from "@earendil-works/pi-ai";

const BASE_URL = "http://127.0.0.1:3060/v1";
const PROVIDER_ID = "ofm-server";
const REFRESH_TTL_MS = 5 * 60 * 1000;

interface ModelCard {
	id: string;
}

let cache: { at: number; models: ProviderModelConfig[] } | undefined;

/** 从 ofm-server 的 /v1/models 拉取模型清单。 */
async function discover(signal?: AbortSignal): Promise<ProviderModelConfig[]> {
	const resp = await fetch(`${BASE_URL}/models`, {
		headers: { authorization: "Bearer dummy" },
		signal,
	});
	if (!resp.ok) throw new Error(`GET /models 失败: HTTP ${resp.status}`);

	const body = (await resp.json()) as { data?: ModelCard[] };
	return (body.data ?? []).map((card) => ({
		id: card.id,
		name: card.id,
		api: "openai-completions" as const,
		reasoning: true,
		input: ["text" as const],
		cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		contextWindow: 128000,
		maxTokens: 16384,
	}));
}

async function discoverCached(): Promise<ProviderModelConfig[]> {
	const now = Date.now();
	if (cache && now - cache.at < REFRESH_TTL_MS) return cache.models;
	const models = await discover(AbortSignal.timeout(10_000));
	cache = { at: now, models };
	return models;
}

export default async function (pi: ExtensionAPI) {
	let models: ProviderModelConfig[] = [];
	try {
		models = await discoverCached();
	} catch (error) {
		// 服务未启动时不阻断 pi 启动，只留空表。
		console.error(
			`[ofm-server] 模型发现失败: ${error instanceof Error ? error.message : String(error)}`,
		);
	}

	pi.registerProvider(PROVIDER_ID, {
		name: "ofm-server",
		baseUrl: BASE_URL,
		api: "openai-completions",
		apiKey: "dummy",
		models,
		refreshModels: async (context) => {
			if (!context.allowNetwork) return cache?.models ?? models;
			try {
				return await discover(context.signal);
			} catch {
				return cache?.models ?? models;
			}
		},
	});
}
