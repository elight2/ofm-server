# WorkBuddy 客户端协议（腾讯 WorkBuddy 中国版）

> 客户端版本: 5.11.3 ｜ 分析方式: 静态读解包 JS(`main/` + `@genie/agent-harness`) + 实测

## 0. 总结

* Electron 客户端, 对话走**标准 OpenAI Chat Completions**: `POST {endpoint}/v2/chat/completions`,
  SSE 为纯 `data:` 帧, **上游只支持流式**。
* 登录走 account-scoped-external-link(浏览器授权 + 轮询换 token), 凭据为 `accessToken`/`refreshToken`。
* 信封类接口一律 HTTP 200 + `{"code":0,"data":...}`, `code != 0` 即失败。

### 本协议不支持, 但Chat Completions有的功能

* `response_format` / `json_schema` / `logprobs` / `n > 1` / `seed`: 官方客户端未使用, 上游是否支持未验证。
* `audio` / `video` 输入: 上游仅文本 + 图片(`image_url`)。
* `prompt_cache_key`: 上游支持前缀缓存(见 §3), 本项目不发送该字段。
* `reasoning_effort` / `thinking`: 上游支持, **本项目两者都不发送**, 保持上游默认思考行为。
* Responses API: 官方部分模型可走 `/v1/responses`, 本协议只覆盖 Chat Completions。

### 本协议与官方客户端的出入

* 对话基址 → `https://www.workbuddy.cn/v2`。
* 模型目录 → 官方有两源, 本项目以**线上 `cli` agent 的 `models[]`** 为准(见 §4)。
* 归属/风控头(`X-IDE-Type`、`X-Private-Data`、`X-Agent-Intent` 等)→ 除 `Authorization` /
  `X-User-Id` 外均非必需, 本项目尽量照抄。
* 思考强度 → 官方按目录 `reasoning.supportedEfforts` 算档位并发送, 本项目不实现该层。
* 签到 → 官方桌面端需 `X-Device-Token`(图灵 SDK), **实测中国版不需要**, 空请求体即可领取。
* 非流式 → 上游不支持, 本项目统一用 `stream:true` 再本地聚合。

### 域名

|域名和端点|功能|
|-|-|
|`https://www.workbuddy.cn`|主 endpoint(`/v3/config`、`/v2/plugin/*`、`/v2/chat/completions`、`/v2/billing/meter/*`)|
|`https://copilot.tencent.com`|备用 origin, 网络故障时 failover|
|`https://wb.tencentbuddy.com`|灾备 origin|
|`https://www.workbuddy.cn/console/client-login`|一次性登录码换授权页|

## 1. 登录方式

```text
① POST https://www.workbuddy.cn/v2/plugin/auth/state?platform=workbuddy
   Body: {}
   Headers: X-No-Authorization: true
            X-No-User-Id: true
            X-No-Enterprise-Id: true
            X-No-Department-Info: true
   → {"code":0,"data":{"state":"<state>","authUrl":"https://..."}}

② 打开 authUrl, 用户网页登录

③ GET https://www.workbuddy.cn/v2/plugin/auth/token?state=<state>
   Headers: 同 ① 的四个 X-No-* 头
   未就绪 → code=11217
   → {"code":0,"data":{accessToken, refreshToken, tokenType:"Bearer",
                       expiresIn, expiresAt, refreshExpiresIn, refreshExpiresAt,
                       domain, enterpriseId}}

④ GET https://www.workbuddy.cn/v2/plugin/account
   Headers: Authorization: Bearer <accessToken>
   → {"code":0,"data":{uid, uin, nickname, email, type, enterpriseId,
                       enterpriseName, enterpriseRole, avatarUrl, editionType,
                       departmentFullName, ...}}
```

* 轮询间隔 **1 秒**, 总超时 **5 分钟**。
* account 未就绪 → `code=12151`。
* 需持久化: `accessToken` / `refreshToken` / `expiresAt` / `refreshExpiresAt` /
  `uid`(→ `X-User-Id`) / `enterpriseId`(→ `X-Enterprise-Id`) / `domain`(→ `X-Domain`)。

## 2. 令牌续期方式

```text
POST https://www.workbuddy.cn/v2/plugin/auth/token/refresh
Headers: X-Refresh-Token: <refreshToken>
         X-Auth-Refresh-Source: plugin
         X-Enterprise-Id / X-Tenant-Id   // 企业账号
Body: {}
→ {"code":0,"data":{accessToken, refreshToken, expiresIn, expiresAt,
                    refreshExpiresIn, refreshExpiresAt}}
```

* 到期时间按 `expiresAt = now + expiresIn*1000` 计算, refresh 侧同理。
* 刷新回 **401/403** → 判定登录失效, 需重新登录。
* 对话 401 时先 refresh 再重放一次请求。

## 3. Chat Completions映射

```text
POST https://www.workbuddy.cn/v2/chat/completions
Authorization: Bearer <accessToken>
Content-Type: application/json
Accept: text/event-stream
X-User-Id: <uid>
X-Enterprise-Id / X-Tenant-Id: <enterpriseId>   // 企业账号
X-Domain: <domain>                              // 多租户时
```

**上游只支持流式**: `stream:false` 回 `code=11101`(`Non-stream chat request is currently not supported`)。

请求体为标准 OpenAI 字段: `model` / `messages` / `stream` / `tools` / `tool_choice` /
`temperature` / `stop` / `max_tokens`。

* `max_tokens` 对 `o1/o3/o4/gpt-4.1/gpt-5` 前缀模型须改写为 `max_completion_tokens`
* 图片: `{"type":"image_url","image_url":{"url":"data:image/png;base64,..."}}`,
  仅 `supportsImages=true` 的模型可发
* 不发送 `reasoning_effort` / `thinking` / `prompt_cache_key`

响应为纯 `data:` 帧(无 `event:` 行), 标准 OpenAI chunk:

```text
data: {"choices":[{"index":0,"delta":{"reasoning_content":"..."}}]}
data: {"choices":[{"index":0,"delta":{"content":"..."}}]}
data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_x","function":{"name":"get_weather","arguments":"{\"city\":"}}]}}]}
data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":28,"completion_tokens":32,"total_tokens":60,"prompt_cache_hit_tokens":0,"prompt_tokens_details":{"cached_tokens":0,"cache_write_tokens":0},"completion_tokens_details":{"reasoning_tokens":29},"credit":0.02}}
data: [DONE]
```

⚠️ 解析要点(不照做就出错):

* **中间帧的 `finish_reason` 是空串 `""` 而非 `null`**, 只有最后一帧才给真实取值。
  须把空串当缺失, 否则第一帧就判流结束, 后续正文全丢。
* `usage` 在中间帧恒为 `null`, 真实 usage 只在结束帧; 须忽略 `null`。
* `delta.function_call` 在结束帧可能是 `{"name":"","arguments":""}`(空串等价缺失),
  不可误判为一次工具调用。
* 工具调用后续分片的 `id`/`name` 可能为空串, 不可覆盖已累积值。
* `usage` 额外字段: `prompt_cache_hit_tokens` / `prompt_cache_miss_tokens` /
  `prompt_cache_write_tokens` / `credit`(费用权重, 非 token 数)。
  思考 token 在 `completion_tokens_details.reasoning_tokens`。

## 4. models端点映射

主源:

```text
GET https://www.workbuddy.cn/v2/enterprises/personal/models
Headers: User-Agent: WorkBuddy/5.11.3      // 缺失回 code=12403
→ {"code":0,"data":{
     "models":[{id, name, credits, vendor, maxInputTokens, maxOutputTokens,
                supportsToolCall, supportsImages, supportsReasoning,
                reasoning:{effort|defaultEffort, supportedEfforts[], canDisableThinking}}],
     "agents":[{name:"cli", models:[...]}]
   }}
```

备源: `GET https://www.workbuddy.cn/v3/config`(超时 5s, 结构同上)。

解析规则:

* **只暴露 `name == "cli"` 的 agent 的 `models[]`**: `data.models[]` 含图像/视频/补全类条目,
  调用对话端点会回 `code=11102`
* `id` → model id, `name` → 显示名, `owned_by` 固定 `workbuddy`
* `maxInputTokens` → 上下文窗口; `maxOutputTokens` → 默认输出上限
* `supportsImages` → `input_modalities` 含 `image`
* `credits` **就是计费倍率**: 形如 `x0.51 credits` / `x0.00`, 去掉前缀 `x` 与 ` credits`
  单位后解析为数值 → `rate`。无 `credits` 字段的(如 `auto`)倍率未知, 不输出倍率后缀

## 5. 余额获取

个人账号:

```text
POST https://www.workbuddy.cn/billing/meter/get-user-resource-summary
Headers: Accept-Language: zh|en
→ {"code":0,"data":{
     "Packages":[{"PackageCode":"...","CycleTotalCapacity":<数>,
                  "CycleRemainCapacity":<数>,"CycleUsedCapacity":<数>}],
     "IsPaidUser":true}}
```

`creditsTotal = Σ CycleTotalCapacity`, `creditsLeft = Σ CycleRemainCapacity`。

⚠️ 额度字段实际可能返回**数字或字符串**(个人账号实测返回 `"2262"`), 两种都要兼容。

企业账号:

```text
POST https://www.workbuddy.cn/v2/billing/meter/get-enterprise-user-usage
Headers: X-Enterprise-Id: <enterpriseId>
→ {"code":0,"data":{"limit_num":<总额度>,"used_num":<已用>}}
```

`limit_num == -1` 表示无限额度。

## 6. 签到协议

```text
① POST https://www.workbuddy.cn/v2/billing/meter/checkin-activity-status
   → {"code":0,"data":{active, today_checked_in, streak_days, daily_credit,
                       today_credit, is_streak_day, next_streak_day,
                       streak_bonus_days, streak_bonus_credit, checkin_dates,
                       total_credits, start_time, end_time, ...}}

② POST https://www.workbuddy.cn/v2/billing/meter/daily-checkin
   Body: {}
   → {"code":0,"data":{credit:100, streak_days:1, is_streak_day:true}}
   失败: {"code":1001|1002|1003,"msg":"..."}
```

* 错误码映射: `1001` → 已领取 / `1002` → 不符合条件 / `1003` → 活动已结束
* ⚠️ 官方桌面端会带 `X-Device-Token`(图灵 SDK 设备令牌), **中国版实测无需该头**:
  个人账号带现有认证头 + 空请求体 `{}` 即可领取(`code=0`、`credit=100`)

## 7. 上游异常响应

| 场景 | 形态 |
|-|-|
| 信封类接口(`/v3/config`、`/billing/*`、`/v2/plugin/*`) | HTTP 200 + `{"code":0,"data":...}`, `code != 0` 即失败, `msg` 为文案 |
| 对话 HTTP 错误 | 非 200, body 为 `{"code":<数字>,"msg":"...","displayMsg":{"zh":"...","en":"..."}}`(**不是** OpenAI `error` 对象) |
| 对话流内错误 | HTTP 200, SSE 帧中出现 `{"code":11140,"msg":...}`(**无 `choices`**) |

* 应有则解析 `displayMsg.zh`/`displayMsg.en`(官方客户端优先用它呈现), 否则只剩 `msg` 原文。
* 流内错误帧判据: **无 `choices`** 且带 `code`/`msg`。有 `choices` 的帧即使正文出现「审核」
  等字样也不是错误帧。

错误码:

| code | 含义 | 归类 |
|-|-|-|
| `11128` | Request blocked(渠道校验失败) | InvalidRequest |
| `11140` | 账号被风控限制 | InvalidRequest |
| `11141` | 模型已下线 | InvalidRequest |
| `401` / `403` | 认证失效 | 先 refresh, 失败则需重登 |
| `429` | 限流 | RateLimit |
| `400` | 参数错误 | InvalidRequest |
| `5xx` | 服务端错误 | Upstream |

上下文超限: 上游以 `displayMsg` 文案表达, 需结合文案(含 `context` / `maximum context` 等字样)
与目录 `maxInputTokens` 双重判定。
