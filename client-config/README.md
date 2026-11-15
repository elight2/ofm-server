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

### 两条发现路径（互为兜底）

| 路径 | 触发时机 | 说明 |
|-|-|-|
| 扩展加载时拉取 | 每次启动 | 保证**首次即可用**，无需先跑交互式 |
| `refreshModels` | 交互式启动后 / `/model` 选择器 / 后台刷新 | 走官方机制，负责**持久化**与增量更新 |

pi 的非交互路径（`--print`、`--list-models`）以 `allowNetwork: false` 初始化，
`refreshModels` 只恢复快照、不联网。若只靠它，**首次会显示 0 个模型**，
因此扩展在加载时（`async` 工厂）先同步拉一次作为兜底。

实测对照：

| 场景 | 结果 |
|-|-|
| 无快照 + 服务在线，`--list-models` | ✅ 56 个（0.4s） |
| 无快照 + 服务离线，`--list-models` | 0 个，**0.37s 返回**（回环立即 ECONNREFUSED，不卡启动） |
| 有快照 + 服务离线，`--list-models` | ✅ 56 个 |

启动兜底有 3 秒上限（`STARTUP_TIMEOUT_MS`），避免远端地址不可达时拖慢 pi 启动。

### 避免拖慢启动

首次兜底直接串在 pi 启动路径上。本机回环一般立即失败（<0.5s），
但如果 `OFM_SERVER_BASE_URL` 指向不可达的远端，最坏会等满 3 秒。可调小：

```sh
export OFM_SERVER_TIMEOUT_MS=1500
```

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
