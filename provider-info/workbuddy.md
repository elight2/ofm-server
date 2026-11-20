# WorkBuddy 客户端协议（腾讯 WorkBuddy 中国版）

> 分析来源：`~/tools/docs/agent-clients/WorkBuddy-win32-x64-user-5.11.3.43806934-ecace0f7.exe`
> 解包至 `tmp/workbuddy-extract/`（Electron，`app.asar` 自行解包到 `tmp/wb-asar/`）。
> 客户端版本：5.11.3（`product.json` 内 `genieVersion` 同版本）
> 分析方式：静态阅读解包后的 JS（`main/`、`renderer/assets/`、`app.asar.unpacked/cli/`、
> `app.asar.unpacked/node_modules/@genie/agent-harness/cli/dist-server/*.cjs`），**未做二进制逆向，未抓包**。
> 标注：`[打包JS]` = 解包 JS 直接证据；`[未验证]` = 推断。

## 0. 总结
* 客户端版本：5.11.3（`product.json` 内 `genieVersion` 同版本）

* WorkBuddy 中国版是 Electron 客户端，核心业务在 `@genie/agent-harness`（CodeBuddy 系 harness）里，
  桌面壳（`main/`）另外实现了账户/计费/签到（`main/account.js`）。
* 对话走**标准 OpenAI Chat Completions**：`POST {endpoint}/v2/chat/completions`，SSE 为纯 `data:` 帧。
* 登录走 **account-scoped-external-link**（浏览器授权 + 客户端轮询换 token），
  凭据为 `accessToken` / `refreshToken`（Bearer）。
* 客户端形态与官方的主要差异见 §0.3。

### 本协议不支持, 但 Chat Completions 有的功能

| 功能 | 结论 |
|-|-|
| `response_format` / `json_schema` | 官方客户端未使用，服务端是否支持 `[未验证]` |
| `logprobs` / `top_logprobs` | 官方客户端未使用 `[未验证]` |
| `n > 1` | 未见处理 `[未验证]` |
| `seed` | 未见处理 `[未验证]` |
| `audio` / `video` 输入 | 官方仅文本 + 图片（`image_url`） |
| `prompt_cache_key` | 上游支持（见 §3.4），但官方客户端**未发送**该字段 `[打包JS]`（仅旧插件实测过有效） |
| Responses API | 客户端对部分模型可走 `/v1/responses`；本协议只覆盖 Chat Completions |

### 本协议与官方客户端的出入

1. **域名差异**：官方 harness 的对话基址是 `{product endpoint}/v2`；桌面 `main/server.js`
   另有一处 VLM 代理硬编码 `https://copilot.tencent.com/v2/chat/completions` `[打包JS]`。
   对话基址取 `https://www.workbuddy.cn/v2`。
2. **登录入口不同**：官方桌面端额外支持 `/v2/plugin/device/auth/code` 拿一次性 device code，
   拼 `https://www.workbuddy.cn/console/client-login?code=...&target=...` 给用户点 `[打包JS]`；
   harness 本体走 `auth/state` 返回 `authUrl` `[打包JS]`。两者等价，本协议按 `auth/state` 描述。
3. **模型目录双源**：harness 用 `GET /v3/config`（`data.models[]`，权威，含 `maxInputTokens`）；
   另存在 `GET /v2/enterprises/personal/models`（cloud agent 用的**别名**目录，非 chat 目录）`[打包JS]`。
   本协议以 `/v3/config` 为准。
4. **官方客户端要求带一堆归属头**（`X-IDE-Type`、`X-Private-Data`、`X-Product`、
   `X-Agent-Intent`、`X-User-Id`、`X-Enterprise-Id`…）`[打包JS]`。除 `Authorization` /
   `X-User-Id` 外其余为**归因/风控**用，缺失不影响功能，但建议尽量照抄（§3.2）。
5. **风控**：桌面端签到需 `X-Device-Token`（图灵 SDK 设备令牌）`[打包JS]`；
   对话侧有 `code=11128`（渠道校验，疑似校验 UA/来源）`[打包JS]`。
6. **深思考**：deepseek 系模型必须显式带 `reasoning_effort` 才出思考内容，
   `thinking:{type:"enabled"}` 单独**无效**（此结论来自旧插件实测 `[未验证]` 对本次中国版）。

### 域名 `[打包JS：product.json / harness 常量]`

| 域名和端点 | 功能 |
|-|-|
| `https://www.workbuddy.cn` | 主 endpoint（`product.json.endpoint`）。`/v3/config`、`/v2/plugin/*`、`/v2/chat/completions`、`/v2/billing/meter/*` |
| `https://copilot.tencent.com` | 备用 origin（`CN_SAAS_CORE_API_ALTERNATE_ORIGIN`），网络故障时 failover |
| `https://wb.tencentbuddy.com` | 灾备 origin（`CN_SAAS_CORE_API_BACKUP_ORIGIN`） |
| `https://staging.workbuddy.cn` | 预发，不使用 |
| `https://www.workbuddy.cn/console/client-login` | 一次性登录码换授权的页面（桌面端 device code 流程） |
| `*.sso.copilot.tencent.com` / `tencent.sso.copilot.tencent.com` | SSO 域（登录页可能跳转过去） |

## 1. 登录方式

类型 `account-scoped-external-link`，`prefixPath = "/plugin"`，平台 `workbuddy`
（`product.json.authentication.attributes`）`[打包JS]`。

### 1.1 步骤

```text
① POST https://www.workbuddy.cn/v2/plugin/auth/state?platform=workbuddy
   Body: {}
   Headers:
     X-No-Authorization: true
     X-No-User-Id: true
     X-No-Enterprise-Id: true
     X-No-Department-Info: true
   → {"code":0,"data":{"state":"<state>","authUrl":"https://.../console/login?..."}}

② 打开 authUrl（浏览器 / 让用户点），用户在网页完成登录

③ GET https://www.workbuddy.cn/v2/plugin/auth/token?state=<state>
   Headers: 同 ① 的四个 X-No-* 头
   未就绪时返回 code=11217（LOGIN_TOKEN_PENDING_CODE）
   → {"code":0,"data":{
        "accessToken":"<jwt/bearer>",
        "refreshToken":"<refresh>",
        "tokenType":"Bearer",
        "expiresIn":<秒>,
        "expiresAt":<ms>,          # 可选，服务端给
        "refreshExpiresIn":<秒>,
        "refreshExpiresAt":<ms>,
        "domain":"<租户域名，可选>",
        "enterpriseId":"<企业 id，个人账号为空>"
     }}

④ GET https://www.workbuddy.cn/v2/plugin/account
   Headers: Authorization: Bearer <accessToken>
   → {"code":0,"data":{ uid, uin, nickname, email, type, enterpriseId,
                        enterpriseName, enterpriseRole, oneidAccountId,
                        avatarUrl, editionType, departmentFullName, ... }}
```

* 轮询间隔 **1 秒**，总超时 **5 分钟**（`SIGN_IN_FETCH_INTERVAL` / `SIGN_IN_PENDING_TIMEOUT`）`[打包JS]`。
* token 未就绪 → `code=11217`；account 未就绪 → `code=12151`（LOGIN_ACCOUNT_PENDING_CODE）`[打包JS]`。
* 另一套（EXTERNAL_LINK_V2）用 `GET /v2/plugin/login/account?state=<state>` 取账号，
  本协议采用 ④ `/v2/plugin/account` `[打包JS]`。
* 桌面端另有 device code 流程：`POST /v2/plugin/device/auth/code`
  → `{"data":{"deviceCode":"..."}}`，再拼
  `https://www.workbuddy.cn/console/client-login?code=<deviceCode>&target=<target>` `[打包JS]`。

### 1.2 账号信息

`/v2/plugin/account` 返回的账号字段（桌面端 `account.js` 的归一化字段）：
`uid`（对应 `X-User-Id`）、`uin`、`nickname`/`displayName`、`email`、`phoneNumber`、
`type`（`personal`/`pro`/`ultimate`/`exclusive`）、`enterpriseId`、`enterpriseName`、
`enterpriseRole`、`enterpriseUserName`、`oneidAccountId`、`avatarUrl`、
`editionType`（`experience`/`pro`/…）、`departmentFullName`、`isPro`、`expireAt` `[打包JS]`。

### 1.3 令牌存储

服务端侧只需持久化：`accessToken`、`refreshToken`、`expiresAt`、`refreshExpiresAt`、
`uid`（→ `X-User-Id`）、`enterpriseId`（→ `X-Enterprise-Id`/`X-Tenant-Id`）、`domain`（→ `X-Domain`）。
官方客户端存在本地 `~/.codebuddy/` 配置目录 + 账号存储 `[打包JS]`；服务端侧凭证存储格式由实现方自定（见 §1.3）。

## 2. 令牌续期方式

```text
POST https://www.workbuddy.cn/v2/plugin/auth/token/refresh
Headers:
  X-Refresh-Token: <refreshToken>
  X-Auth-Refresh-Source: plugin
  （+ X-Enterprise-Id / X-Tenant-Id 若为企业账号）
Body: {}
→ {"code":0,"data":{ accessToken, refreshToken, expiresIn, expiresAt,
                     refreshExpiresIn, refreshExpiresAt, ... }}
```

* 客户端在 `refreshSession` 里调用，随后 `calculateExpiresAt`：
  `expiresAt = now + expiresIn*1000`，`refreshExpiresAt = now + refreshExpiresIn*1000` `[打包JS]`。
* 刷新失败且上游回 **401/403** → 判定登录失效（`isAuthenticationInvalidError`）`[打包JS]`。
* 另有 `POST /v2/plugin/auth/logout`（带 `X-Refresh-Token`）用于注销 `[打包JS]`。
* 对话侧 401 时 harness 会先 refresh 再重放一次请求 `[打包JS]`（VLM upstream 同款逻辑）。

## 3. Chat Completions 映射

### 3.1 端点与基础头

```text
POST https://www.workbuddy.cn/v2/chat/completions
Authorization: Bearer <accessToken>
Content-Type: application/json
Accept: text/event-stream
```

### 3.2 官方客户端额外请求头 `[打包JS]`

| 头 | 值 | 必需性 |
|-|-|-|
| `X-User-Id` | 账号 `uid` | 建议（风控/归因） |
| `X-Enterprise-Id` / `X-Tenant-Id` | `enterpriseId`（企业账号） | 企业账号建议 |
| `X-Domain` | `auth.domain`（多租户） | 可选 |
| `X-Department-Info` | `departmentFullName` | 可选 |
| `X-Product` | `SaaS`（deploymentType；另一处代码路径写 `WorkBuddy`） | 建议 |
| `X-IDE-Type` / `X-IDE-Name` | `WorkBuddy` | 建议 |
| `X-IDE-Version` | 客户端版本（如 `5.11.3`） | 建议 |
| `X-Private-Data` | `true` | 建议 |
| `X-Agent-Intent` | `craft` | 建议 |
| `X-Agent-Purpose` | `conversation` | 建议 |
| `x-requested-with` | `XMLHttpRequest` | 建议 |
| `x-codebuddy-request` | `1` | 建议 |
| `X-Request-ID` / `X-Conversation-ID` / `X-Conversation-Request-ID` / `X-Conversation-Message-ID` | UUID | 可选 |
| `User-Agent` | `WorkBuddy/<版本> CLI/<cli版本>` | 建议 |

### 3.3 请求体

标准 OpenAI 字段：`model`、`messages`、`stream`、`tools`、`tool_choice`、`temperature`、
`stop`、`max_tokens`（对 `o1/o3/o4/gpt-4.1/gpt-5` 前缀模型官方改写为 `max_completion_tokens`，
见 harness `MaxTokensFieldRule`）`[打包JS]`。

额外/需注意：

* `reasoning_effort`：见 §5。
* `thinking`：deepseek 系模型官方会带 `thinking:{type:"enabled"}`；关闭思考时带
  `thinking:{type:"disabled"}` 并删除 `reasoning_effort`（harness `ThinkingFormatTranslatorRule`
  的 `deepseek` 分支）`[打包JS]`。
* `prompt_cache_key`：`{会话 id}`。缺省时前缀缓存不命中；带同 key 可复用前缀缓存 `[未验证（旧插件实测）]`。
* 图片：`{"type":"image_url","image_url":{"url":"data:image/png;base64,..."}}`，
  仅 `supportsImages=true` 的模型可发。
* 官方会删除下游带上的 `verbosity`、`reasoning_summary`（`SdkFieldCleanupRule`）`[打包JS]`。
* 官方在 `reasoning_effort` 不被模型支持时删除该字段（`ReasoningEffortSupportRule`）`[打包JS]`。

### 3.4 响应（SSE）

纯 `data:` 帧（无 `event:` 行），OpenAI 标准 chunk：

```text
data: {"choices":[{"index":0,"delta":{"reasoning_content":"..."}}]}
data: {"choices":[{"index":0,"delta":{"content":"..."}}]}
data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_x",
        "function":{"name":"get_weather","arguments":"{\"city\":"}}]}}]}
data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],
       "usage":{"prompt_tokens":28,"completion_tokens":32,"total_tokens":60,
                "prompt_cache_hit_tokens":0,
                "prompt_tokens_details":{"cached_tokens":0,"cache_write_tokens":0},
                "completion_tokens_details":{"reasoning_tokens":29},
                "credit":0.02}}
data: [DONE]
```

* `reasoning_content`（或 `reasoning`）承载思考链；官方 harness 两者都读 `[打包JS]`。
* `usage` 除标准字段外还有 `prompt_cache_hit_tokens` / `prompt_cache_miss_tokens` /
  `prompt_cache_write_tokens`（以及 Anthropic 风格的 `cache_read_input_tokens` /
  `cache_creation_input_tokens`）和 `credit`（费用权重，非 token 数）`[打包JS]`。
* 思考 token 计数在 `usage.completion_tokens_details.reasoning_tokens`（也兼容
  `reasoningTokens` / `thought_tokens`）`[打包JS]`。
* 工具调用后续分片里 `id`/`name` 可能为空串（等价缺失），解析时不可覆盖已累积值。

## 4. models 端点映射

### 4.1 上游模型目录

```text
GET https://www.workbuddy.cn/v3/config
（headers: Connection: close，超时 5s；失败可重试）
→ {"code":0,"data":{
     "models":[ { id, name, credits, vendor, iconUrl,
                 maxInputTokens, maxOutputTokens, maxAllowedSize,
                 supportsToolCall, supportsImages, supportsReasoning,
                 onlyReasoning, temperature,
                 reasoning:{ effort|defaultEffort, supportedEfforts[],
                             canDisableThinking, summary } , ... } ],
     "agents":[ { name:"cli", models:[...] , ...} ],
     "availableModels":[...] , ...
   }}
```

* `data.models[]` 是权威目录（harness `enrichLanguageModelFromCatalog` 读它的
  `maxInputTokens` / `supportsImages` / `supportsReasoning`）`[打包JS]`。
* `data.agents[].models[]` 给出各 agent（`cli`/`craft`）可选模型子集 `[打包JS]`。
* `product.json` 里另有一份**内置兜底目录**（58 条，含 `default`/`auto`/图像/视频/补全类模型），
  远端不可用时官方回退用它 `[打包JS]`。

### 4.2 映射到 `/v1/models`

* 只暴露 `data.models[]` 中 `id` 以对话模型为主的条目；图像/视频/补全类
  （`hunyuan-image-*`、`kling-*`、`codewise-*`、`hunyuan-3b` 等 `maxOutputTokens` 仅 256 或无
  `maxInputTokens` 的条目）不映射为 chat 模型。
* `id` → `model.id`；`name` → `model.name`；`owned_by` 固定 `workbuddy`。
* `maxInputTokens` → 上下文窗口；`maxOutputTokens` → 默认输出上限（不发 `max_tokens` 时用它兜底）。
* `supportsImages` → `input_modalities` 含 `image`。
* `reasoning.supportedEfforts` → `reasoning_efforts`；`reasoning.defaultEffort` → `default_reasoning_effort`；
  `reasoning.canDisableThinking=false` 时不暴露 `none`。
* `credits`（如 `x0.51 credits`）可映射为倍率 `rate`。

### 4.3 另一目录（不用于 `/v1/models`）

```text
GET https://www.workbuddy.cn/v2/enterprises/personal/models
→ {"models":[{"id":..., "tags":[...]}]}   # cloud agent 别名目录，会过滤 tags 含 text-to-image 的项
```
`[打包JS]`，本协议不使用该目录。

## 5. reasoning_effort 字段映射

客户端的思考强度字段就是 OpenAI 的 `reasoning_effort`（字符串），另有 `reasoning:{effort:...}`
对象形态，两者官方都读 `[打包JS]`。

全局档位集合（harness 常量 `VALID_REASONING_EFFORTS`）：
请求层合法值 `minimal, low, medium, high, xhigh, max`（**不含 `off`/`none`**）；
内部思考等级集合 `off, minimal, low, medium, high, xhigh, max` `[打包JS]`。

每个模型实际支持的档位来自 `/v3/config` 的 `reasoning.supportedEfforts`；
若模型未声明 `supportedEfforts` 但声明了 `thinkingLevelMap`，则按 map 推导：
取 map 中出现 `max` → 最高 `max`；出现 `xhigh` → 最高 `xhigh`；否则最高 `high`，
再取不高于该档位的全部等级 `[打包JS]`。两者都没有时视为不支持 reasoning。

内置产品目录里的典型声明 `[打包JS]`：

| 模型 | reasoning 声明 |
|-|-|
| `deepseek-v4-pro` / `deepseek-v4-flash` / `deepseek-v4.1-flash` | `effort: high`, `summary: auto`（未列 supportedEfforts → 走默认集合） |
| `glm-5.3-flash` / `glm-5.3-flashx` | `defaultEffort: high`, `supportedEfforts: [low, high, max]`, `canDisableThinking: true` |
| `kimi-k2.8-preview` | 同上 `[low, high, max]` |
| `hy4-preview` / `hy4-preview-x` | `supportedEfforts: [high]`, `canDisableThinking: false` |
| `glm-5.2` / `glm-5.1` / `minimax-*` / `kimi-k2.6/2.7` / `hy3*` | `effort: medium` |
| `default` / `auto` / `hunyuan-chat` / `glm-4.6` | 无 reasoning |

映射规则：

| 下游值 | 上游值 | 说明 |
|-|-|-|
| `none` | `reasoning_effort: "none"` | 仅 `canDisableThinking=true` 的模型允许；harness 在用户显式关闭思考时把 `reasoning_effort` 置为 `"none"`（仅对 chat/completions 网关请求生效）`[打包JS]` |
| `minimal`/`low`/`medium`/`high` | 原样 | 必须在该模型支持的档位集合内 |
| `xhigh`/`max` | `high` | 模型无 `thinkingLevelMap` 时官方把 `xhigh`/`max` 收敛为 `high`（`LegacyXhighFallbackRule` / `CustomModelFieldCleanupPlugin`）；有 map 则按 map 翻译 `[打包JS]` |
| `off` | `none` | 同 `none` |

不支持的档位处理（`clampEffortToSupported`）：若下游给的档位不在模型支持集合内，
向下取最近的受支持档位；若一个都没有，取集合中最低档；仍无支持档位则删除 `reasoning_effort`
（`ReasoningEffortSupportRule`）`[打包JS]`。

**deepseek 系特殊要求**：必须显式带 `reasoning_effort`（或 `thinking:{type:"enabled"}` + 档位），
否则上游按不思考应答；此结论来自旧插件对官方客户端的三站点实测 `[未验证]`。

## 6. 余额获取

### 6.1 个人账号（资源包汇总）

```text
POST https://www.workbuddy.cn/billing/meter/get-user-resource-summary
（桌面端 resourcePrefix 为空；headers: Accept-Language: zh|en）
→ {"code":0,"data":{
     "Packages":[{ "PackageCode":"TCACA_code_002_AkiJS3ZHF5",
                   "CycleTotalCapacity":<数>, "CycleRemainCapacity":<数>,
                   "CycleUsedCapacity":<数>, ... }],
     "SubscriptionPackageCode":"...", "SubscriptionPackageName":"...",
     "IsPaidUser":true }}
```

桌面端解析：`creditsTotal = Σ CycleTotalCapacity`，`creditsLeft = Σ CycleRemainCapacity`，
并把每个包映射为 `{commodityCode: PackageCode, remaining, total}` `[打包JS]`。

配套的分页明细端点（`resourcePrefix` 为空，`POST`，`Accept-Language` 头）：

* `/billing/meter/get-user-resource-free-packages`
* `/billing/meter/get-user-resource-paid-packages`

请求体：`{PageNumber, PageSize, Status:[...], PackageCodes?, OnlyValidPeriod?, NeedRenewInfo?}`，
响应 `data.Accounts[]` + `data.TotalCount`；字段含 `ResourceId`、`PackageName`、`PackageCode`、
`CapacitySizePrecise`/`CapacityUsedPrecise`/`CapacityRemainPrecise`、
`CycleCapacity*`、过期时间 `DeductionEndTime`/`ExpiredTime`/`CycleEndTime` `[打包JS]`。

### 6.2 企业账号

```text
POST https://www.workbuddy.cn/v2/billing/meter/get-enterprise-user-usage
Headers: X-Enterprise-Id: <enterpriseId>
→ {"code":0,"data":{"limit_num":<总额度>,"used_num":<已用>}}
```

`limit_num == -1` 表示无限额度 `[打包JS]`。
另有一套 cloud-agent 侧调用（无 `/v2` 前缀、驼峰字段）：
`POST /billing/meter/get-enterprise-user-usage` → `data.limitNum` `[打包JS]`。

### 6.3 其它

* 套餐/升级：`POST /v2/billing/pay/get-price`、`/v2/billing/pay/create-order`、`/v2/billing/pay/set-renew-flag` `[打包JS]`
* 大使：`GET /v2/ambassador/status` → `{is_ambassador}` `[打包JS]`
* 邀请码：`GET /activity/workbuddy/invitation/v2/my-code`、`.../invite-records` `[打包JS]`

## 7. 签到协议

桌面端 `DesktopAccountRepo` 的 `billingPrefix = "/v2"`，`resourcePrefix = ""`；
`postCheckin` 会附加图灵设备令牌头（见下）`[打包JS]`。

```text
① POST https://www.workbuddy.cn/v2/billing/meter/checkin-activity-status
   Headers:
     X-Device-Token: <图灵设备令牌>      # 或 X-Device-Token-Error: <err>
   → {"code":0,"data":{
        active:true, today_checked_in:false, streak_days:0,
        daily_credit:100, today_credit:0, is_streak_day:true,
        next_streak_day:0, streak_bonus_days, streak_bonus_credit,
        checkin_dates:[...], total_credits, start_time, end_time,
        theme_name, season, activity_name, claim_button_text, action_button }}

② POST https://www.workbuddy.cn/v2/billing/meter/daily-checkin
   Headers: 同 ①
   → {"code":0,"data":{ credit:100, streak_days:1, is_streak_day:true }}
   失败：{"code":1001,"msg":"..."}   # 1001 已领取 / 1002 不符合条件 / 1003 活动已结束
```

* `mapCheckinStatus`：`1001→AlreadyClaimed`、`1002→NotEligible`、`1003→EventEnded`，
  其余 `UnknownBizError` `[打包JS]`。
* 设备令牌由图灵 SDK（`turingSdk.channelId = 109144`）产生，取不到时客户端发
  `X-Device-Token-Error` 头 `[打包JS]`。**建议先不带该头请求**
  （国际版旧插件实测可直接成功，中国版是否强制 `[未验证]`）。
* Web 版（renderer）另有不带 `/v2` 前缀的两个端点：
  `POST /billing/meter/checkin-status`、`POST /billing/meter/daily-checkin` `[打包JS]`；
  桌面端主链路用带 `/v2` 的 `checkin-activity-status`。
* 大使活动：`GET /v2/ambassador/status`（与签到分离）。

## 8. 上游异常响应

### 8.1 形态

| 场景 | 形态 |
|-|-|
| 信封类接口（`/v3/config`、`/billing/*`、`/v2/plugin/*`） | HTTP 200 + `{"code":0,"data":{...}}`；`code != 0` 即失败，`msg` 为文案 |
| 对话 HTTP 错误 | 非 200，body 为 `{"code":<数字>,"msg":"...","displayMsg":{"zh":"...","en":"..."}}`（腾讯系自有格式，非 OpenAI `error` 对象）`[打包JS]` |
| 对话流内错误 | HTTP 200，SSE 帧里出现 `{"code":11140,"msg":...}`（**无 `choices`**）`[打包JS]` |

`displayMsg` 是腾讯系特有的中英文双语文案，官方客户端优先用 `displayMsg.zh`/`displayMsg.en`
呈现给用户 `[打包JS]`；应解析 `displayMsg`（否则只有 `msg` 原文）。

### 8.2 错误码（客户端内置文案表 `[打包JS]`）

| code | 官方文案（en） | 建议归类 |
|-|-|-|
| `11128` | Request blocked（渠道校验失败，可能非官方客户端发起） | InvalidRequest（可提示用官方客户端） |
| `11140` | Account restricted for unusual activity（账号被风控限制） | InvalidRequest / 账号风控 |
| `11141` | Model retired（模型已下线） | InvalidRequest（提示切换模型） |
| `401` / `403` | 认证失效 | Authentication（先 refresh，失败则需重登） |
| `429` | 限流 | RateLimit |
| `400` | 参数错误 | InvalidRequest |
| `5xx` | 服务端错误 | Upstream |

### 8.3 流内错误帧的判据

SSE 帧**没有 `choices`** 且带 `code`/`msg` 时视为错误帧（`11140`/`11128` 等）`[打包JS]`；
有 `choices` 的帧即使正文出现「审核」等字样也不是错误帧。官方对 `11128` 直接抛错，
对 `11140` 标记账号冷却并抛错 `[打包JS]`。

### 8.4 上下文超限

上游会以 `displayMsg` 文案表达「请求超出模型上下文限制」，官方通过
`CONTEXT_WINDOW_EXCEEDED_CODE` + 文案正则双重判定（正则要求出现
`context` / `for this model` / `maximum context` 等字样）`[打包JS]`。
应结合 `/v3/config` 的 `maxInputTokens` 做本地预检。

### 8.5 认证失效的处理顺序

1. 对话返回 401/403 → 先 `POST /v2/plugin/auth/token/refresh`；
2. 刷新成功 → 重放一次请求；
3. 刷新失败或仍 401 → 判定需重新登录（`ofm-server login workbuddy`）。

