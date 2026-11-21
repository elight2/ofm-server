# minimax 客户端协议（MiniMax Code）

> 客户端版本: `MiniMax/3.1.16`(Electron 42.11.16) ｜ 分析方式: 读客户端 TS 源码 + 抓包 + 明文实测

## 0. 总结

* 上游是 **anthropic-messages**, 不是 Chat Completions: 对话走 `/messages`, SSE 流,
  认证**双发** `x-api-key` + `authorization: Bearer`(同值)。
* 登录是 **OAuth 2.0 Device Code Grant + PKCE**, `client_id = mcode-public`。
* `access_token` / `refresh_token` 都是**不透明串**(`mmoat_…` / `mmort_…`, 各 60 字符,
  无 `.` 分隔), **无法从令牌读到期时间**, 只能用响应 `expires_in` 自算。

### 本协议不支持, 但Chat Completions有的功能

* `temperature` / `top_p` / `top_k` / `stop_sequences` / `metadata` / `service_tier` /
  `logprobs` / `seed` / `user` / `n` / `prompt_cache_*`: 官方客户端从不发送, 上游是否接受未知。
  本实现不发送。
* `response_format`: Messages 侧对应 `output_config.format = {type:"json_schema",schema}`(源码有),
  本实现不转发。
* `tool_choice`: 上游**支持**(`auto`/`any`/`tool`/`none` 四形态实测 200), 本实现按 CC 四值映射。
  ⚠️ 无工具时**不要发** `tool_choice`(`any` 会因无工具被拒)。
* 音频输入: 无(`modalities.input` 只有 text/image/video)。
* 视频输入: 官方在 payload 边界把 `video/*` 块改成 `{type:"video"}`, CC 无对应字段, 不实现。

### 本协议与官方客户端的出入

* 域名写死预设, 忽略目录下发的 `baseURL`: 目录回 `options.baseURL = agent.minimaxi.com/…`,
  源码把该域列入 `LEGACY_MANAGED_PRESET_BASE_URLS`; 真实对话全部打 `agent.minimax.cn`。
* **思考强度不转发**: 上游有两个**互不通用**的落点, 按模型分(见 §3.7), 本实现请求体
  既不带 `thinking` 也不带 `output_config`, 保持上游默认思考行为。
* 登录无法纯命令行完成: 设备码只是第一步, 账号确认页需微信扫码 + 手机号短信验证
  (`1200044 未绑定手机号`), 属浏览器侧。本实现只「打印 user_code + 轮询」。
* `cache_control` 断点不发送: 官方固定 3 处(`system[0]` / 末个 tool / 末条消息末块),
  依赖其消息布局; 我方布局不同, 乱放可能降低命中率。
* 历史 thinking 块丢弃: CC 无 thinking 块概念, 无状态转发只能丢。
* `count_tokens` 端点不实现(CC 无对应端点)。
* 余额不用 Token Plan 窗口额度: 那是订阅制 5 小时/周额度, 与积分余额两回事。

### 域名

| 域名 / 端点 | 用途 |
|-|-|
| `account.minimax.cn`(海外 `account.minimax.io`) | OAuth 设备码、令牌、撤销、登录页 |
| `agent.minimax.cn/mavis/api/v1/models?region=cn&buildEnv=prod` | 模型目录 |
| `agent.minimax.cn/mavis/api/v1/llm/v1/messages` | 对话(+`/count_tokens`) |
| `agent.minimax.cn/matrix/api/v1/commerce/*` | 余额 |
| `agent.minimax.cn/minimax-cloud/api/v1/signin/*` | 签到 |

⚠️ **目录路径不能从对话 baseURL 拼接**: 对话是 `…/mavis/api/v1/llm/v1/messages`,
但目录是 `…/mavis/api/v1/models`(少一级 `llm/v1`)。写死抓包路径。
非 prod 环境源码里是 `.example.invalid` 占位, **只需实现 prod**。

## 1. 登录方式

| 项 | 值 |
|-|-|
| 设备码 | `POST https://account.minimax.cn/oauth2/device/code`(`x-www-form-urlencoded`) |
| 令牌 | `POST https://account.minimax.cn/oauth2/token`(同上) |
| 撤销 | `POST https://account.minimax.cn/oauth2/revoke` |
| 确认页 | `https://account.minimax.cn/oauth-authorize?user_code=<UC>&client_surface=desktop&download_source=default` |
| `client_id` / `scope` / `audience` | `mcode-public` / `agent.default` / `agent-backend` |
| PKCE | 32 字节随机 → `code_verifier`(base64url 43 字符) → `code_challenge = base64url(sha256(verifier))` |

```text
POST /oauth2/device/code
body: client_id=mcode-public&scope=agent.default&audience=agent-backend
      &code_challenge=<43>&code_challenge_method=S256
→ {"device_code":"<43>","expires_in":300,"interval":3,"user_code":"WVC4-3DE3",
   "verification_uri":"https://account.minimax.cn/oauth-authorize",
   "verification_uri_complete":"…?user_code=WVC4-3DE3"}

POST /oauth2/token   每 3 秒一次
body: grant_type=urn:ietf:params:oauth:grant-type:device_code
      &device_code=<device_code>&client_id=mcode-public&code_verifier=<verifier>
→ 400 {"error":"authorization_pending",…}  |  200 {…见 §1.2}
```

* `user_code` 格式 `XXXX-XXXX`(4+4 大写字母数字)。轮询截止 `now + expires_in*1000`(300s)。
* 客户端侧 `/device/code`、`/token` **不带 cookie、不带 origin**。

轮询分支:

| 上游 | 处理 |
|-|-|
| `400 authorization_pending` / `200 {"status":"pending"}` | 睡 `interval` 继续 |
| `400 slow_down` / `200 {"status":"slow_down"}` | `interval += 5s` 继续 |
| `access_denied` / `{"status":"denied"}` | 登录失败 |
| `expired_token` / `{"status":"expired"}` | 超时 |
| 200 其他 | 解析令牌 |

**凭证存储**: `access_token` / `refresh_token` / `expires_at`(收到响应时刻 + `expires_in`*1000)。
`expires_at` 是判断是否需刷新的**唯一**依据(无 JWT 可解), 缺失一律视为需刷新。
RT 失效 → 必须重新登录, 无其他恢复路径。

## 2. 令牌续期方式

```text
POST https://account.minimax.cn/oauth2/token
body: grant_type=refresh_token&refresh_token=<RT>&client_id=mcode-public
      &scope=agent.default&audience=agent-backend
→ {"access_token":"mmoat_…","expires_in":43200,"refresh_token":"mmort_…",
   "scope":"agent.default","token_type":"Bearer"}
```

* 响应不带新 RT 时沿用旧 RT。
* ⚠️ **RT 单次使用且轮换**: 成功一次后旧 RT 立刻
  `400 {"error":"invalid_grant","error_description":"this refresh token can no longer be used…"}`。
  **必须立即落盘新 RT**, 否则只能重新登录。续期须**持锁串行**, 并发各刷一次会把对方 RT 打废。
* ⚠️ **AT 失效与 `expires_at` 无关**: 观测到磁盘 `expires_at` 仍指向未来但 AT 已 401。
  故 `chat` 对 401 必须刷一次再重试, 不能只靠时间窗。
* 凭证文件见 `config.example.toml`。

## 3. Chat Completions 映射

### 3.1 请求头

```text
POST https://agent.minimax.cn/mavis/api/v1/llm/v1/messages
content-type: application/json
x-api-key: <access_token>
authorization: Bearer <access_token>
anthropic-version: 2023-06-01
user-agent: MiniMaxAgent
```

最少可用集: `content-type` + `anthropic-version` + 双认证头。实测只发
`authorization: Bearer` 也能成功(`x-api-key` 非必需), 但双发同值兼容性最好。
`x-mavis-*` / `x-stainless-*` / `priority` 等均可省略。

### 3.2 请求映射

| Chat Completions | 上游 body |
|-|-|
| `model` | `model`: 完整 ID(`MiniMax-M3.1-Flash`), **不是**展示名 `M3.1-Flash` |
| `max_tokens` | `max_tokens`: 本实现固定 `128000`(目录 `limit.output`; CC 请求无此字段) |
| `stream` | 恒 `true` |
| `role=system` / `developer` | 拼成 `system: [{type:"text",text:"…"}]`(**数组**, 非字符串); 缺失合法 |
| `role=user` | `{role:"user",content:[{type:"text",text:"…"}]}` |
| `role=assistant` + content | `{role:"assistant",content:[{type:"text",text:"…"}]}` |
| `role=assistant` + tool_calls | `{role:"assistant",content:[{type:"tool_use",id,name,input}]}`, 多个可并列 |
| `role=tool` | `{role:"user",content:[{type:"tool_result",tool_use_id,content:"<字符串>",is_error:false}]}` |
| `tools[]` | `tools[{name,description,input_schema}]`, **`input_schema` 是对象**(不是字符串) |

* ⚠️ `tool_result.content` 是**字符串**, 不是 text 块数组; 多个结果并列在同一 user message。
* `tool_use.id` 格式 `call-<uuid v4>`, 与 `tool_result.tool_use_id` 一一对应。
* `tools[].eager_input_streaming: true`: 官方每个 tool 都带, 是否必需未知; 本实现不带。
* 图片块(仅 base64, **无 `url` 来源类型**):

```json
{"type":"image","source":{"type":"base64","media_type":"image/png","data":"<base64>"}}
```

  `media_type` ∈ `image/jpeg|image/png|image/gif|image/webp`。
  下游 `image_url` 为 http(s) URL 时须**先下载再转 base64**。

### 3.3 流式响应

帧序列(实测):

```text
event: message_start        {"type":"message_start","message":{…,"usage":{"input_tokens":0,"output_tokens":0}}}
event: ping                 {"type":"ping"}                                     ← 必须忽略
event: content_block_start  {"index":0,"content_block":{"type":"text","text":""}}
event: content_block_delta  {"index":0,"delta":{"type":"text_delta","text":"…"}}
event: content_block_delta  {"index":0,"delta":{"type":"signature_delta","signature":"<64hex>"}}
event: content_block_stop   {"index":0}
event: message_delta        {"delta":{"stop_reason":"end_turn"},"usage":{…}}
event: message_stop         {"type":"message_stop"}
```

* `message_start` 的 `usage` **全是 0**, 真实用量只在 `message_delta`。
* `content_block_start.content_block.type` ∈ `text` / `thinking` / `tool_use`:
  * `thinking` 块增量用 `thinking_delta`(字段 `thinking`);
  * `tool_use` 块起始帧带 `id`/`name` 且 `input` **恒为 `{}`**; 增量用 `input_json_delta`,
    `partial_json` 逐片拼接, **首片为空串 `""`**, 随后才是 `"{"` / `"\"city\":\"北京\""` / `"}"`。
* `signature_delta` 收尾 thinking 块, 下游无对应概念, 丢弃。
* ⚠️ **`index` 是全局块序号**(thinking=0, text=1, tool_use=2…), 而下游 `tool_calls[].index`
  必须在工具内部从 0 连续编号 → **必须重映射**。
* `thinking` 块 → 下游 `reasoning_content` 扩展字段。

`stop_reason` → `finish_reason`: `end_turn`/`stop_sequence` → `stop`; `max_tokens` → `length`;
`tool_use` → `tool_calls`。

### 3.4 usage 映射

`message_delta.usage` 实测形状:

```json
{"input_tokens":79,"output_tokens":223,"cache_read_input_tokens":128,
 "service_tier":"standard","output_tokens_details":{"thinking_tokens":123}}
```

⚠️ **`input_tokens` 不含缓存命中部分** —— 本项目最易错的一点。同一 payload 走
`/count_tokens` 返回 206, 而流内 `input_tokens` 只有 78~79, 另计 `cache_read_input_tokens=128`;
`78 + 128 = 206` ✅。

| 上游 | 下游 |
|-|-|
| `input_tokens` **+** `cache_read_input_tokens` | `prompt_tokens`(**必须相加**) |
| `output_tokens` | `completion_tokens` |
| 前两者之和 | `total_tokens` |
| `cache_read_input_tokens` | `prompt_tokens_details.cached_tokens` |
| `output_tokens_details.thinking_tokens` | `completion_tokens_details.reasoning_tokens` |
| `cache_creation_input_tokens` | 无对应位, 丢弃 |

下游 `stream:false`: 上游恒 `stream:true`, 必须**聚合完整 SSE** 再拼单个 `chat.completion`;
中途 `event: error` 应报错而不是吐半截内容。

### 3.5 思考过程回传

assistant 历史消息里的 thinking 块:

```json
{"type":"thinking","thinking":"…","signature":"<64 hex = 32 字节>"}
```

块序固定: `thinking` 在 `text` **之前**, 含工具时 `tool_use` 在 `text` **之后**。
Anthropic 语义要求 thinking 块连签名原样回传, 否则可能 400。
CC 无对应概念 → 本实现**丢弃**历史 thinking, 属已知能力损失。

### 3.6 count_tokens(不实现)

`POST …/llm/v1/messages/count_tokens` → `{"input_tokens":N}`。CC 无对应端点, 不实现。
仅注意事项: 其 body 是**严格白名单**(`model/messages/system/tools/thinking/tool_choice`),
必须剥掉 `max_tokens`/`output_config`/`tools[].eager_input_streaming`, 否则 400。

### 3.7 思考强度(上游能力参考, 本协议不发送)

两个落点**按模型分, 互不通用**:

| 模型 | 有效手段 | 无效 / 危险 |
|-|-|-|
| `MiniMax-M3`(`mode: switchable`) | `thinking:{"type":"adaptive"\|"disabled"}` | `output_config.effort` 完全无效; `on`/`off`/`enabled` 被**静默忽略**; `budget_tokens` 不识别; 省略全部字段 → 不思考 |
| `MiniMax-M3.1-Flash`(`mode: forced_on`) | `output_config:{"effort":"low"…"max"}`(单调生效) | ⚠️ 发 `thinking:{"type":"disabled"}` 会 **HTTP 400**; `off` 被静默忽略 |

* `default` 与不传等价; Flash 省略全部字段 = 正常思考。
* 档位集合由目录 `effort_options` 决定, **不硬编码**。
* 官方 UI 给 M3 的 `on`/`off` 是**客户端内部字面量**, 发到上游等于没发 —— 「发了字段 ≠ 生效」。
* 本实现请求体**两者都不发**, 保持上游默认。

## 4. models 端点映射

```text
GET https://agent.minimax.cn/mavis/api/v1/models?region=cn&buildEnv=prod
authorization: Bearer <access_token>
accept: application/json
user-agent: <Electron 桌面 UA>
```

目录接口**不带** `x-api-key` / `anthropic-version` / `x-mavis-*`, UA 是完整浏览器 UA
而非 `MiniMaxAgent`。两个 query 参数必须带。

响应结构: `{version, ttlSeconds:300, providers:[{providerId, config:{models:{…}, model_order, whitelist, options}}]}`。

映射:

| 上游 | 下游 |
|-|-|
| `config.models` 的 **key** | `id`(`minimax/MiniMax-M3.1-Flash`), 不用 `name` |
| `config.model_order` | 列表顺序(与 key 字典序相反) |
| `limit.context` / `limit.output` | 上下文 512000 / 最大输出 128000 |
| `reasoning` / `attachment` | 是否支持思考 / 附件 |
| `modalities.input` 含 `image` | 是否支持图片 |
| `thinking_config.mode` | `switchable` / `forced_on`(见 §3.7) |
| `effort_options` / `default_effort` | 可选档位集, 仅 M3.1-Flash 有 |

忽略项: `variants`(与 `thinking_config` 重复)、`capabilities.*`(文件 API、附件上限)、
`options.apiKey`(占位 `sk-xxx`)、`options.baseURL`(旧域名, §0)、`whitelist`(含 `MiniMax-M2.7*`
但 `models` 未下发, 不用于列表)、`ttlSeconds`(本实现不缓存, 每次实时请求)。

* 目录**不提供**任何倍率/折扣字段 → 对外模型名**不带** ` (x…)` 后缀。
* `providers` 空或 `models` 缺失 → 返回**空列表**而非报错(目录可用但暂无模型)。

## 5. 余额获取

```text
POST https://agent.minimax.cn/matrix/api/v1/commerce/get_membership_info
body: {}
Authorization: Bearer <access_token>
```

取 `op_credit_summary.total_remaining_amount`(⚠️ **字符串**, 如 `"8437.456"`),
回退 `opcredit_balance`; 两者都取不到返回 `None`(账号未开通计费, 不是错误)。

* ⚠️ `opcredit_balance` / `total_remains_credit` 可能都是 `0`, 与真实余额不符 ——
  **只能用 `op_credit_summary`**。
* 签名头 `yy`/`x-timestamp`/`x-signature`、全部 query 参数、`user_id`、
  前置 `get_user_extra_info` 均**不需要**, 一次直连即可。

## 6. 签到协议

```text
GET  https://agent.minimax.cn/minimax-cloud/api/v1/signin/status?timezone_offset=28800
POST https://agent.minimax.cn/minimax-cloud/api/v1/signin/claim?timezone_offset=28800   body: {}
Authorization: Bearer <access_token>
```

* `timezone_offset`(秒)**必需**: 缺失回 HTTP 200 + `{"base_resp":{"status_code":1406010011,
  "status_msg":"invalid timezone_id"}}`。取值不影响返回哪一天, 本实现固定 `28800`。
* `POST` body 必须为 `{}`。签名头、`user_id`、其余 query 均不需要。

响应:

```json
GET status → {"data":{"scene":2,"days":[{"day_no":1,"points":400,"status":3,
               "is_today":true,"bonus_points":0},…]},"base_resp":{"status_code":0}}
POST claim → {"data":{"claim_id":"…","claim_result":2,"day_no":1,"points":400},
              "base_resp":{"status_code":0}}
```

| 字段 | 含义 |
|-|-|
| `days[].status` | `1` 未到 / `2` 可领 / `3` 已领 / `4` 停用 |
| `days[].is_today` | 今日那一格(全表最多一个) |
| `days[].points` | 当日积分, **已含 `bonus_points`**, 不要相加 |
| `panel.scene` | `0` 未知 / `1` 首次 / `2` 进行中 / `3` 已完成 / `4` 异常 |
| `claim_result` | `1` 本次领取成功 / `2` 今日已领(**幂等, 不是错误**) |

实现策略: 先 `GET status` 判今日 `status == 3` → `skipped` 不打 POST; `== 2` 才 `POST claim`;
`claim_result == 2` → `skipped`; `base_resp.status_code != 0` → 协议错误。

## 7. 上游异常响应

### 7.1 OAuth

标准 OAuth 2.0 错误体(HTTP 非 2xx + 表单 POST): `{"error":"…","error_description":"…"}`,
分支见 §1。⚠️ 同时兼容另一种表示: **HTTP 200 + `{"status":"pending"}`**, 两套按同语义分类。

### 7.2 对话 `/messages`

```json
{"type":"error","error":{"type":"invalid_request_error",
 "message":"invalid params, unknown model 'No-Such-Model' (2013)"},
 "request_id":"…"}
```

按 `error.type` 分类(`invalid_request_error` / `authentication_error` / `rate_limit_error` /
`overloaded_error` / `not_found_error`)转下游 OpenAI 错误体。
HTTP 401 → 刷新一次后重试; 刷新也失败(RT 失效)才映射下游 401。

### 7.3 模型目录 / 账号中心

* 目录: 无业务码信封, HTTP 非 200 即失败。
* 令牌失效: `401 {"error":"invalid access token"}`(**无信封、无 `code` 字段**)。
* 账号中心两套形状并存: `/oauth2/*` 用 `{"error",…}`; `/v1/api/user/*` 与 `/oauth2/login`
  用 `{"statusInfo":{code,httpCode,message,…}}`(**可能 HTTP 200 而 `code != 0`**,
  如 `1200044` 未绑定手机号)。`/oauth2/login` 成功又是 `{code:0,msg:"ok",data:{…}}` 无 `statusInfo`
  → 两种形状都要兼容, `code` / `statusInfo.code` 任一非 0 即失败。
