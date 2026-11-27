# ofm 模型注入插件

每 1 分钟从 ofm-server 拉取一次模型列表，并把列表动态注入 pi 的模型列表（provider id `ofm-server`）。启动阶段不请求网络。

所有模型统一声明为支持文本 + 图片输入。

## 加载方式

pi 只自动发现 `<agent目录>/extensions/` 与项目内 `.pi/extensions/`，本插件放在项目 `extensions/pi/` 下，需要显式加载：

- 临时：`pi -e ./extensions/pi`
- 长期：在 `~/.pi/agent/settings.json` 的 `extensions` 数组里加上本目录的绝对路径：
  ```json
  { "extensions": ["/home/main/working/ofm-server/extensions/pi"] }
  ```

> 注意：若已加载 `~/.pi/agent/extensions/ofm-provider.ts`（provider id 同为 `ofm-server`），后加载者会覆盖前者，建议删掉旧的只保留本次插件。

## 配置（环境变量）

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `OFM_BASE_URL` | `http://127.0.0.1:3060/v1` | ofm-server 的 OpenAI 兼容 base url |
| `OFM_API_KEY` | `ofm` | 下游 api key（ofm-server 默认不校验） |
| `OFM_REFRESH_MS` | `60000` | 刷新间隔，最小 5000 |

## 行为

- 注册 provider 时不带 `models`：模型列表完全由 1 分钟定时刷新（以及 `/ofm-refresh`）填充，启动阶段不请求 ofm-server。
- `session_start` 时只启动 60s 定时刷新，不立即同步；`session_shutdown` 清理定时器。
- 刷新通过 `ctx.modelRegistry.refresh({ providers: ["ofm-server"], force: true })` 走 pi 官方的 `refreshModels` 通道，结果直接替换 pi 的模型列表。
- 刷新失败（服务未启动）只提示一次告警，保留旧列表。
- `/ofm-refresh` 命令可手动触发一次刷新。
- 模型上下文窗口/输出上限取保守默认值 128K / 16K（ofm-server 的 `/v1/models` 不提供这些字段）。
