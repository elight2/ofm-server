# 客户端配置

## pi：自动发现模型（生产方案）

`pi-ofm-provider.ts` 让 pi 从 ofm-server 的 `GET /v1/models` 动态获取模型清单，
走 pi 官方模型发现机制（`refreshModels` + 持久化），不写死模型名。

### 安装

```sh
mkdir -p ~/.pi/agent/extensions
cp client-config/pi-ofm-provider.ts ~/.pi/agent/extensions/ofm-provider.ts
```

服务地址默认 `http://127.0.0.1:3060/v1`，可用环境变量覆盖：

```sh
export OFM_SERVER_BASE_URL=http://127.0.0.1:3060/v1
```

### 首次必须先跑一次交互式（重要）

pi 的刷新时机（读 `dist/` 源码确认）：

| 场景 | 是否联网刷新 |
|-|-|
| 交互式启动后异步刷新 | **是**（`refreshModelCatalogs`，15s 超时，不阻塞 UI） |
| `/model` 打开选择器 | **是** |
| `--print`、`--list-models`、print 模式 | **否**（`allowNetwork: false`），只读持久化快照 |

因此**首次使用必须先启动一次交互式 `pi`**，等它拉取并写入
`~/.pi/agent/models-store.json`；此后非交互模式也能正常列出模型，且服务未启动时
`--list-models` 仍可用（只是对话会 `ECONNREFUSED`）。

### 行为说明

- 快照新鲜度窗口 5 分钟，窗口内不重复请求（`force` 可绕过）。
- 上游拉取失败时**保留旧快照**，不会把模型列表清空。
- 模型 id 已去重并排序，避免上游顺序抖动导致重复刷新。
- 清空缓存重来：删除 `~/.pi/agent/models-store.json`。

### 不能用于内置 provider 的自动发现

**pi 内置的 provider（openai/anthropic/deepseek 等）不支持从 `/v1/models` 自动发现**：
它们全部是静态清单（`packages/ai/dist/providers/*.models.js` 里写死），
全部 pi 源码里**没有任何一处**调用 `/v1/models`；内置 provider 中只有 `radius`
实现了 `refreshModels`。

想让某个内置 provider 也自动发现，只能用同样的扩展手法**覆盖**它：

```ts
pi.registerProvider("deepseek", { models: [], baseUrl: "https://api.deepseek.com/v1",
  api: "openai-completions", refreshModels: async (ctx) => {/* 拉 /models 并返回 */} });
```

代价是要自己维护该 provider 的认证、api 类型、header 等，**不属于开箱即用**，
且与 pi 内置实现走不同代码路径，版本升级后可能失配。本仓库只提供 ofm-server 的扩展。
