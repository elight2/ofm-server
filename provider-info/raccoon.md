# 商汤小浣熊(Raccoon Work)客户端协议

**源码定位**（`tmp/raccoon-extract`）：`app.asar` 里有两层可读源码，**协议逻辑全在主进程
`build/electron/main/`**。

|路径|内容|
|-|-|
|`.env.electron`|全部服务地址、各 API 前缀|
|`build/electron/main/api/config.js`|`BASE_URL`(内网测试地址，勿用)|
|`build/electron/main/account/desktopLogin.js`|桌面深链登录 + code 换 token|
|`build/electron/main/client/account/webLogin.js`|登录状态机(深链 / loopback 兼容登录)|
|`build/electron/main/client/account/webLoginLoopback.js`|本地回连登录服务(127.0.0.1 一次性)|
|`build/electron/main/schedules/scheduleAuth.js`|token 刷新(300s 提前窗口)|
|`build/electron/main/agent/boxAgentConfig.js`|把 token 写进 `auth.json`、生成 LLM 配置|
|`build/electron/main/client/models/hostedModels.js`|模型目录、内置模型预设|
|`build/electron/main/client/models/hostedModelPresentation.js`|倍率/折扣字段|
|`build/electron/main/client/account/accountRequests.js`|`/api/web/auth/v1/*` 全部端点|
|`build/src/lib/agent/localAgentModelRoutingCore.js`|模型目录响应结构校验|
|`dist/electron/main.cjs`|本地 relay(`/v1/chat/completions` → 上游)|
|`out/_next/static/chunks/*`|渲染层：积分、验证码、手机号加密|

## 0. 总结
* 客户端版本: 桌面 `1.2.0`(`@raccoon/office-desktop`，包名`商汤小浣熊-1.2.0-amd64.deb`)；
  移动 `1.1.0`(`raccoon-1.1.0.apk`)
* 分析方式: 解包 deb(`tmp/raccoon-extract`) + 解析 `resources/app.asar` 内 `build/`(未混淆的编译 JS) + 渲染层 webpack chunk
  (`out/_next/static/chunks/`)；APK 为 Capacitor 套壳(`tmp/raccoon-apk/assets/public/_next/`)。
  **未抓包**，未反编译 `box-agent-acp`(PyInstaller)、未反编译 `classes.dex`
* UA: 无固定 UA。身份靠请求头(见下)。`GET /api/web/llm/v2/model_catalog` 等端点**不校验 UA**

### 本协议不支持, 但 Chat Completions 有的功能
* `n` / `logprobs` / `logit_bias` / `seed` / `stop` 之外的采样细节取决于上游网关，客户端不显式传
* 思考档位可用标准 `reasoning_effort` 表达：`low`/`medium`/`high` 是**真实枚举**，
  其中 `low` 能稳定抑制思考（0 思考字符）。非标准值（`xhigh`/`minimal`/`none`）会被
  当默认处理，思考量与不传相当。实测见 §3.7
* （**`tool_choice` 不在此列**：四种取值均实测生效，已实现透传，见 §3 `tool_choice` 实测）

### 本协议与官方客户端的出入
* 官方桌面端登录有两条可用路径(深链 / loopback)。深链需 OS 注册协议，服务器收不到；
  **loopback 兼容登录可服务器复用**(见 §1)。旧插件“回调地址写死改不成 localhost”的结论只对旧版网页成立
* 官方对话由 `box-agent-acp`(Python)发起，主进程只负责下发 `api_base`/`api_key`/`auth.json`；
  服务器直接按 OpenAI 格式打 `/api/web/llm/v2/chat/completions` 即可
* `dist/electron/main.cjs` 的本地 relay 是受限版本(只允许单一 model、`tools` 必须为空)，**不能照抄**
* 官方**没有传统签到**，但有登录领取端点：`POST /api/web/mobile/v1/login/points/grant`。
  它同时发每日 300 分并把有效期延到 7 天，兼发移动端首登 3000 分(幂等,每号一次)。
  实测**只有真实客户端登录触发**该发放，刷 token / 拉余额 / 发对话均无效(见 §7)

### 域名
|域名和端点|功能|
|-|-|
|`https://xiaohuanxiong.com`|主站与全部 API 基址|
|`/api/web/auth/v1/*`|登录、刷新、用户信息|
|`/api/web/llm/v2/chat/completions`|对话(OpenAI 兼容)|
|`/api/web/llm/v2/model_catalog`|模型目录|
|`/api/web/llm/v2/images/gen`|文生图|
|`/api/web/points/v1/balance`|积分余额|
|`/api/web/points/v1/tasks`|任务/邀请积分|
|`/api/web/points/v2/bills`|积分流水|
|`/api/web/desktop/v1/login/points/grant`|桌面登录赠送积分(需 `X-Client-Platform`)|
|`/api/web/desktop/v1/sessions/{id}/points-consumed`|会话消耗积分|
|`/api/web/office/v3/*`|Office 业务(文档/表格等，与本协议无关)|
|`/api/web/org/*`|企业版(私有化时启用)|
|`wss://collab-server.xiaohuanxiong.com`|协作(无关)|
|`https://sta-volc.xiaohuanxiong.com`|更新与 runtime 下载 CDN|

## 1. 登录方式
共四种，服务器侧可用「loopback 兼容登录」或自行取「授权码」：

1. **桌面深链**(官方主路径)
   * 浏览器打开 `https://xiaohuanxiong.com/authorize?login_source=desktop&appname=办公小浣熊客户端`
     (`NEXT_PUBLIC_LOGIN_URL=/login`，`DESKTOP_AUTH_PATH=/authorize`)
   * 登录成功后回调 `office-raccoon://auth/callback?code=xxx`，客户端取 `code`
   * `POST https://xiaohuanxiong.com/api/web/auth/v1/login_with_authorization_code`
     body `{"authorization_code": "<code>"}`
   * 响应 `{code:0, data:{access_token, refresh_token, office_identity, office_org_name, office_org_role}}`
   * 错误码 `200035` = 授权码不存在/过期/已消费
2. **短信**：`send_sms` / `login_with_sms`，手机号必须 AES-128-CFB 加密
   (key=UTF8(`senseraccoon2023`)，iv=随机16B，输出 `base64(iv‖ct)`)，并需阿里云滑块验证码
3. **IAM**(SSO)：`access_token` 为 JWT 且 `iss` 以 `/iam` 结尾时走 `iam/confirm`、`iam/logout`，
   无 `refresh_token`，只能重新登录

### loopback 兼容登录(1.2.0 新增，服务器可复用)
实现：`client/account/webLoginLoopback.js` + `webLogin.js`
1. 本地起一次性 HTTP 服务：`listen(0, '127.0.0.1')`，回调路径 `/auth/callback`
   (OAuth 模式则为 `/oauth/callback/<state>`)，`state = randomBytes(32).hex`(64 位小写 hex)
2. 浏览器打开授权页，在 `desktopLoginUrl` 基础上追加三项后再打开：
   ```
   https://xiaohuanxiong.com/authorize?login_source=desktop&appname=<appName>
     &desktop_callback=loopback
     &redirect=http://127.0.0.1:<port>/auth/callback
     &state=<64位小写hex>
   ```
   (`appname` 取自 `webLoginResources[lang].appName`；`desktop_callback=loopback` **必需**，否则网页走深链)
3. 浏览器回连 `127.0.0.1:<port>/auth/callback?code=xxx&state=xxx`
4. 校验后调 `login_with_authorization_code` 换 token

服务端严格校验(以下任一不符即拒)：
* `Host` 必须等于 `127.0.0.1:<port>`；`remoteAddress` 必须为 `127.0.0.1`
* 仅 `GET`；`Origin` 若存在必须等于授权页 origin；`Sec-Fetch-Dest` 若存在必须为 `document`
* 路径必须为 `/auth/callback`，query 只允许 `code` 和 `state` 且各仅一个，`state` 必须 `^[a-f0-9]{64}$` 且与本地值 timing-safe 相等
* `code` 非空、长度 ≤ 4096、不含控制字符；单次消费(重复→409)
* 超时 300s，`maxConnections=8`

⚠️ **实测结论(2025-11，已真实登录成功)**：loopback 登录**完全可用**。
* 授权页 URL 返回 HTTP 200，`desktop_callback` / `redirect` / `state` 均被受理
* 浏览器完成授权后**确实回连 `127.0.0.1:<port>/auth/callback?code=...&state=...`**
* 用该 code 调 `login_with_authorization_code` 成功换到 token

⇒ 服务器可直接复用此方式，**无需**手动粘 code。

存储(官方)：`<BOX_AGENT_CONFIG_DIR>/config/auth.json`，权限 0600
```json
{"access_token":"...","refresh_token":"...","office_identity":"personal","office_org_name":"","office_org_role":""}
```
`office_identity` 为 `personal` 或组织码；**非 personal 时请求须带 `X-Org-Code`**。

## 2. 令牌续期方式
* `POST https://xiaohuanxiong.com/api/web/auth/v1/refresh`，body `{"refresh_token":"<rt>"}`
* 响应 `{code:0, data:{access_token, refresh_token}}`
* 提前量 `TOKEN_REFRESH_WINDOW_SECONDS = 300`(官方源码)
* `refresh_token` 自身也需检查 `exp`，过期只能重新登录

### 实测(2025-11，个人身份，`login_with_authorization_code` 登入)
登入得到的 token 形态与官方源码假设**不同**：
* `iss` = `xiaohuanxiong.com/iam`，按官方 `isIamToken()` 判定**属 IAM token**
* `access_token` 与 `refresh_token` **字符串完全相同**(同一 `jti`、同一 `exp`)
* 有效期 **30 天**(非文档所述的“约 3h”)：`exp - iat = 2592000`
* **IAM token 可正常调 `/refresh`**：回 `code:0` 且带两个 token，
  故“IAM token 不刷新”的说法**不成立**，但响应是**原样返还同一 token**
  (未临近过期时 `exp`/`jti` 均不变)

⇒ 采用任意一个 token 作 `refresh_token` 均可续期；只需按 `exp` 判断
(提前 300s 窗口)。由于服务端可能原样返还，不要用“token 是否变化”
判断续期是否成功。

## 3. Chat Completions 映射

实测版本：客户端 `1.2.0`，依据 `hostedModels.js`、`boxAgentConfig.js`、`dist/electron/main.cjs`

* 上游地址：`POST https://xiaohuanxiong.com/api/web/llm/v2/chat/completions`
* 请求头(官方 `createHeaders()`，见渲染层模块 35105 + 50398)：
  ```
  Content-Type: application/json
  Authorization: Bearer <access_token>
  X-Raccoon-Language: zh
  X-Org-Code: <office_identity>        // 仅当 office_identity != "personal" 时
  X-Client-Platform: desktop-linux|desktop-macos|desktop-windows   // 仅 desktop/v1 必需
  X-Client-Version: v1.2.0
  X-Client-Device-ID: <32位hex>
  ```
* 请求体：**OpenAI Chat Completions 原样**。客户端把 `remoteBaseUrl` 设为
  `https://xiaohuanxiong.com/api/web/llm/v2`，`remote_provider=openai`，
  box-agent 自行拼 `/chat/completions`。`remote_api_key` 为**空**或占位
  `box-agent-no-auth`，真实凭据走 `auth.json` 的 `access_token`
* 模型名：直接用目录里的 `name`(如 `sn-glm-5-3`)
* 输出上限：官方默认 `XIAOHUANXIONG_MAX_OUTPUT_TOKENS`(仅 xiaohuanxiong 域名的 base 用此值，
  自定义 base 用 `USER_CONFIGURED_MAX_OUTPUT_TOKENS`)；每模型上限取目录 `params.max_tokens`
* 图片输入：模型 `tags` 含 `vision`(目录里可能是 `image`/`image-understanding`，
  客户端统一归一为 `vision`)时允许；注意按**请求体字节**限制 10MB，超限 `HTTP 413`
* 思考：`extra_body.thinking = {"type":"enabled"|"disabled"}`
* 流式：`stream:true` 走 SSE，与 OpenAI 帧一致(客户端直通转发)

### `tool_choice` 实测：四种取值全部生效 `[curl，2026-11-16]`

上游是标准 OpenAI 语义，`LLMUtilsChatRequest` 那类私有结构不存在，
`tool_choice` **原样透传即可生效**。模型 `sn-deepseek-v4-1-flash`，工具 `get_weather`：

| prompt | 不传 | `"none"` | `"required"` | 指定 `get_weather` |
|-|-|-|-|-|
| 「帮我查北京天气」（天然想用）| 调用 | **不调** ✅ | 调用 | 调用 |
| 「不要查询任何天气…」（天然不想用）| 不调 | 不调 | **调用** ✅ | **调用** ✅ |

第二行是决定性证据：`required` 能让**本来不想用工具的模型强制发起工具调用**。

**强制指定函数确实生效**（两个工具 `get_weather`/`get_time`，prompt 问天气）：

| 请求 | 模型实际调用 |
|-|-|
| 不传 `tool_choice` | `get_weather`（模型自选）|
| 强制 `get_time` | **`get_time`**（违背 prompt 语义，说明约束生效）|
| 强制不存在的 `no_such_tool` | HTTP 400 `invalid_request_error`:<br>`Invalid value for 'function_call': no function named 'no_such_tool' was specified in the 'functions' parameter.` |

注：报错文案里的 `function_call`/`functions` 是上游沿用的旧版 OpenAI 字段名，
但请求要发的是 `tool_choice`/`tools`（发旧名报错不认）。

**经 ofm-server 端到端复核**（`stream:false` 聚合后看 `finish_reason`）：

```
场景1 想用工具:   无tc=tool_calls(1)  none=stop(0)  required=tool_calls(1)
场景2 不想用工具: 无tc=stop(0)        none=stop(0)  required=tool_calls(1)  指定函数=tool_calls(1)
```

**实现要点**：`ChatBody.tool_choice` 用 `skip_serializing_if = "Option::is_none"`，
`Auto`（含未指定、显式 `"auto"`、无法识别的形态）序列化为 `None` → 不发送字段，
因为 `auto` 本就是有工具时的默认行为。复现脚本 `tmp/probe_raccoon_tc.py`、`tmp/probe_raccoon_force.py`。

### 实测(2026-11-16，本机，个人身份)

**响应确为标准 OpenAI SSE**：纯 `data:` 帧（无 `event:` 行），末尾 `data: [DONE]`。
单帧样例：
```json
{"id":"85a2..","object":"chat.completion.chunk","created":1794791018,
 "model":"sn-sensenova-6-8-flash","choices":[{"index":0,
 "delta":{"reasoning_content":"好的"}}],"request_id":"85a2.."}
```
* 首帧 `delta:{"role":"assistant"}`；正文 `delta.content`；思考 `delta.reasoning_content`
* 工具调用：`delta.tool_calls[{index,id,type,function:{name,arguments}}]`
  **首帧给 `id`+`name`，后续分片给 `id:""`/`name:""` 与 arguments 片段**
  （空串等同缺失，客户端不应覆盖首帧的 id/name）
* `finish_reason`：`stop` / `tool_calls`，映射正常
* usage 独立一帧：`{prompt_tokens, completion_tokens, total_tokens,
  completion_tokens_details:{reasoning_tokens}, prompt_tokens_details:{cached_tokens}}`

#### 思考开关实测：`extra_body.thinking` **不能可靠关闭思考**
|模型|thinking=disabled|thinking=enabled|不传(基线)|
|-|-|-|-|
|`sn-sensenova-6-8-flash`|19~25 帧|6 帧|21~92 帧|
|`sn-glm-5-3`|125 帧|—|80 帧|
|`sn-glm-5-3-flash`|0 帧|0 帧|—|

结论：
* 对部分模型(如 `sn-sensenova-*`)有**部分抑制**效果，但不彻底且波动大
* 对 `sn-glm-5-3` 完全无效（该模型强制思考）
* 顶层 `thinking`、`thinking_budget` 实测无效

> 早前结论「`reasoning_effort` 也无效」是**误判**：当时用 8 个非标准枚举值测试，
> 均走默认路径，思考量自然与基线相当。换用标准值 `low` 后可稳定抑制，见 §3.7。

#### 3.7 思考强度 `reasoning_effort` 实测 `[curl，2026-11-16]`

**结论：上游是标准 OpenAI 语义，`low` 可稳定关闭思考。**

测试条件：`sn-glm-5-3`，`stream:true`，固定 prompt，统计 `delta.reasoning_content`
累计字符数（比数帧更可靠，帧数与内容长度不成正比）。

|取值|思考字符|判定|
|-|-|-|
|`low`|0, 0, 0, 0, 0|✅ 真实枚举，思考被抑制|
|`medium`|0, 0, 0|✅ 真实枚举|
|`high`|79, 70, 61, 102, 225|✅ 真实枚举，但思考量远低于不传|
|`xhigh`|1009, 638|❌ 非标准值 → 走默认|
|`minimal`|1060, 877, 3592|❌ 非标准值 → 走默认|
|`none`|1890|❌ 非标准值 → 走默认|
|**不传（基线）**|1134, 1606|—|

要点：
* `low`/`medium`/`high` 是真实枚举，能被下游识别；其余取值被当默认。
  用非标准值测试是早前误判「无效」的原因。
* **不构成单调关系**：`high` 反而比不传时思考更少。故无法用 `high` 实现「加深思考」，
  「深度思考」只能靠**不发送该字段**（走基线）。
* ⚠︎ **限流**：连续请求会触发 `HTTP 429 user_model_rate_limited`。429 时思考字符也为 0，
  与 `low` 的真实抑制**无法区分**，测时必须校验 `err` 为空，否则得到假阳性。

服务端实现（`RaccoonProvider::effort`，见 §3 实现要点）：

|配置 `[raccoon] reasoning_effort`|上游请求体|
|-|-|
|`low`|`"reasoning_effort": "low"`|
|`high`|不发送该字段|

#### 其它实测结论
* 多轮工具结果回传正常（`role:tool` + `tool_call_id` 被上游正确理解）
* 图片输入正常：`content` 数组 + `image_url` + `data:` URL，vision 模型能识别
* **不存在的模型名不会报错**：上游会用默认模型兜底作答（返回 200 与正常内容）
  故客户端无法用“模型不存在”做参数校验

### 实现要点(已完成)
`src/providers/raccoon/chat.rs`：请求体直接序列化为标准 OpenAI 形态
* 单段纯文本用字符串 content（最兼容），含图片/多段时用内容数组
* `tools[].function.parameters` 保持**对象**（与 trae 的字符串化不同）
* 工具调用键名是标准 `function`（非 trae 的 `function_call`）
* 无参数工具省略 `parameters` 字段
* 思考强度取 `RaccoonProvider::effort`（来自 `[raccoon] reasoning_effort`）：
  `Low` → 发 `reasoning_effort:"low"`；`High` → 不发该字段（实测见 §3.7）

`src/providers/raccoon/stream.rs`：帧解析
* 空串 `delta.content`/`reasoning_content` 不产事件
* 工具调用空串 `id`/`name` 视为缺失，不覆盖已累积值
* 未发 `finish_reason` 就断流时补 `Finish(Stop)`，避免下游挂住

## 4. models 端点映射

* `GET https://xiaohuanxiong.com/api/web/llm/v2/model_catalog`
* 头：`Authorization: Bearer <token>`，非 personal 加 `X-Org-Code`
* 超时 10s，超时重试 1 次(仅该幂等接口)；401 时刷新 token 重试一次
* 响应结构：
  ```
  {code:0, data:{categories:[
      {type:"chat", default_model:"...", models:[
          {name, description, tags:[...], ability_level, params:{context_window, max_tokens},
           reasoning_efforts,
           points_multiplier, billing_multiplier, billing_effective_multiplier, billing_status:"normal"|"discount"|"limited_free", billing_status_note,
           billing_discounts:[{name,multiplier,active,daily_start,daily_end,start_at,end_at}],
           display_description, visible}
      ]},
      {type:"image"|..., ...}
  ]}}
  ```
* 解析规则(`normalizeLocalAgentRoutingCatalog`)：`code` 必须为 0；只取 `categories[].type=="chat"`；
  按 `name` 去重；`visible!=false` 才算展示；`context_window` 兼容 `params.context_window` 与顶层；
  `default_model` 不在列表内则退化为第一个

### 实测(2025-11，个人身份)
`data.categories` 只有 1 个，`type="chat"`，`models` 共 9 个：
|name|visible|备注|
|-|-|-|
|`raccoon-090dd3`(default) / `raccoon-19b265` / `raccoon-405a1c`|`false`|内部路由模型，不展示|
|`sn-sensenova-6-8-flash` / `sn-sensenova-6-8-flash-lite`|`true`|`tags` 含 `vision`|
|`sn-glm-5-3` / `sn-glm-5-3-flash`|`true`|`billing_category: lite`|
|`sn-kimi-k3` / `sn-deepseek-v4-1-flash-1017`|`true`|——|

倍率取 `billing_effective_multiplier`(已计入折扣)，如 `sn-sensenova-6-8-flash` 的
`billing_multiplier: 0.5` 但 `billing_effective_multiplier: 0`(限免到 11-30，`billing_status: limited_free`)。
模型样例：
```json
{"name":"sn-sensenova-6-8-flash","visible":true,"billing_category":"lite",
 "params":{"context_window":256000,"max_tokens":63999},
 "billing_multiplier":0.5,"billing_effective_multiplier":0,
 "billing_status":"limited_free","billing_status_note":"限免到11月30日",
 "billing_discounts":[{"name":"限免到11月30日","multiplier":0,"active":true,
   "start_at":"2026-11-11T16:00:00Z","end_at":"2026-11-30T15:59:59Z"}]}
```
注：文档早期写 `points_multiplier`，**实测响应里没有该字段**，只有 `billing_*` 系列。
已核对全部 9 个模型的键集合，确认不含 `points_multiplier`。

### 实现要点(已完成)
`src/providers/raccoon/models.rs`：
* 只取 `type=="chat"` 分类；同 `name` 先去重(保留首个)
* `visible == Some(false)` 才过滤；**字段缺失视为可见**(不因上游省略字段丢模型)
* 倍率映射 `ModelInfo.rate = billing_effective_multiplier`
  注意 `Some(0.0)`(限免) 不等于 `None`，需原样保留
* 未过滤前 9 个，过滤后 6 个

## 5. 余额获取
* 积分余额：`GET https://xiaohuanxiong.com/api/web/points/v1/balance`
  (类 `PointsApi.baseUrl = ${origin}/api/web/points`，头带 `Authorization`，
  非 personal 加 `X-Org-Code`)
* 实测响应(个人身份)：
  ```json
  {"code":0,"data":{"available_points":7713,"daily_points":300,"monthly_points":0,
   "reward_points":7413,"topup_points":0,"enterprise_topup_points":0,
   "total_chargeable_points":7713,"enterprise_reserved":false,"topup_frozen":false,
   "owner_type":"users","owner_id":7657055}}
  ```
  卡片主数字取 `available_points`。
* 积分流水：`GET /api/web/points/v2/bills?paging.limit=20&type=all[&cursor=]`
* 会话消耗：`GET /api/web/desktop/v1/sessions/{sessionId}/points-consumed`
  → `{code:0, data:{total_points_consumed: number}}`
## 6. 上游异常响应
通用业务信封：`{code: <int>, message: <str>, data: ...}`，**HTTP 200 也可能 `code!=0`**，必须解析业务码。
客户端已识别的错误码：
|场景|判定|
|-|-|
|授权码失效|`code == 200035` (`AUTHORIZATION_CODE_NOT_FOUND`)|
|身份无效|`code` ∈ `[200022(orgNotExists), 200201(userNotInOrg)]` → 客户端切回 personal 后重试|
|用量超限|`code` ∈ `USAGE_BLOCKED` 列表(渲染层模块 91187 `Vs`)|
|登录失效|HTTP `401` → 刷新 token 后重试一次|
|点数不足|`1000007`(渲染层 `points` 模块)|
|模型目录超时|客户端本地错误 `HOSTED_MODEL_CATALOG_TIMEOUT`|
|请求体过大|`HTTP 413`，客户端文案 `request body exceeds 10MB`|

## 7. 签到协议
**没有传统意义的「签到」端点**，但有一个**登录领取**端点：
`POST /api/web/mobile/v1/login/points/grant`（移动端）。

它既是「每日 300 分」的唯一主动获取入口，也顺带发放移动端首登奖励 3000 分。
服务端幂等，同一天重复调用返回 `granted: false`，已实现为 `claim`。

### 每日积分的有效期与续期(官方文案原文)
客户端积分明细页文案 `setting.resource.points.dailyGrantRule`：

> **每日积分每天 00:00 发放，默认当日 24:00 清零，当天登录后有效期延长至 7 天。**

英/日文同义(`Daily points are granted at 00:00 each day and expire at 24:00 that day by default. Logging in that day extends their validity to 7 days.`)。

**该文案已在 11-16 得到实证**（见下「决定性实测」）：登录确实同时发放当日积分并延长到 7 天。

### 账单字段陷阱：`created_at` 不可信
`bill_id` 有两种形态，含义完全不同：

| `bill_id` 形态 | 含义 | `created_at` 是否可信 |
|-|-|-|
| `daily_makeup:<uid>:<YYYYMMDD>:free:300:grant` | **补记**（事后批量生成的历史账） | ❌ 恒为业务日 `00:00:00` |
| `bill_uuid-xxxx-...` | **真实单据** | ✅ 真实发生时刻 |

⚠️ 11-08 ~ 11-14 那 7 条 `daily_grant` 全是 `daily_makeup` 补记，`created_at` 是业务日假值。
**不能用 `created_at` 推断发放规律**，必须先看 `bill_id` 前缀。
目前真实单据只有两条：11-15 22:04:18（web）和 11-16 09:43:22（mobile）。

### 每日积分发放/过期的实测(2026-11-15/16，本机真实账号)
数据源：`GET /api/web/points/v2/bills?paging.limit=50&type=all[&paging.cursor=]`

响应结构：`{code:0, data:{items:[...], has_more, next_cursor}}`。
⚠️ 翻页参数是 **`paging.cursor`**，不是 `paging.offset`（误用 offset 会静默返回同一页）。
单条记录字段：
|字段|说明|
|-|-|
|`bill_id` / `group_id`|记录 id / 分组 id；`bill_id` 前缀见上表|
|`bill_count`|同组条数|
|`client_type`|`web` 或 `desktop`(消耗来源)|
|`event_name`|中文事件名，如「每日积分发放」「桌面端 LLM 对话消耗」|
|`biz_type`|`daily_grant` / `daily_expire` / `reward_grant` / `desktop`(消费)|
|`biz_id`|业务 id；过期记录形如 `20261114:free:300`|
|`points`|变动分值，**开销为负**|
|`created_at`|记账时间(补记账单为业务日假值)|
|`grant_at`|发放时间(仅 daily_* 有)|
|`expire_at`|失效时间(仅 daily_* 有)|

#### 决定性实测：移动端登录触发发放 + 7 天延期
2026-11-16 09:43:22，用户在 Android APK 客户端登录，**同刻**产生两条真实单据：

|biz_type|points|grant_at|expire_at|
|-|-|-|-|
|`daily_grant`|300|11-16 00:00|**11-22 23:59**（+7 天）|
|`reward_grant`|3000|—|—（「移动端登录奖励」，每号一次）|

余额 `7698 → 10998`：`daily 285→585`（+300）、`reward 7413→10413`（+3000）。

⇒ **「当天登录后有效期延长至 7 天」成立**，且每日 300 分同时补发。
⇒ 多批积分**叠加不覆盖**：11-15 批剩 285（expire 11-21）+ 11-16 批 300（expire 11-22）并存。

#### 已否证的触发方式
|动作|是否触发每日发放/延期|说明|
|-|-|-|
|跨天无任何请求（10.5 小时）|❌|11-16 08:55 查，无新 `daily_grant`|
|`GET /points/v1/balance`|❌|只读|
|`POST /auth/v1/refresh`（回 `code:0`）|❌|只续 token，不算登录|
|`POST /chat/completions`（真实扣费 15 次）|❌|只扣费不算登录|
|网页 OAuth 授权登录|✅|11-15 22:04，`client_type: web`|
|**移动端 APK 登录**|✅|11-16 09:43，已验证|

⇒ 结论：**只有真实客户端登录才算「登录」**。定时刷 token / 拉余额 / 发对话都不能代替。
⇒ 故不实现「定时刷新代替签到」；`claim` 直接调领取端点，效果等同于一次移动端登录。

### 领取端点（APK v1.1.0 源码，已实测可用）
来源：`assets/public/_next/static/chunks/app/layout-*.js` 模块 93305。

```js
static async grantLoginPoints(e, t) {
  return (await u.nC.fetchWithAuth(
    `${u.hj()}/api/web/mobile/v1/login/points/grant`,
    { method: "POST",
      headers: { "X-Client-Platform": `app-${e}`,
                 "X-Client-Version": `v${t.replace(/^v/i, "")}` } })).data
}
```

* 端点：**`POST /api/web/mobile/v1/login/points/grant`**，**无请求体**
* `e` 来自 `getPlatform()`，只会是 `android` / `ios`
  （模块 84935：`getPlatform() in {ios,android} ? 平台 : null`，非移动端不调用）
* 响应 `{code:0, data:{granted:bool, popup?:{points, display_name, source}}}`
* `popup.source` 枚举：`daily` / `monthly` / `old_user` / `new_user` / `desktop_login` / `mobile_login`

#### 客户端触发逻辑（同一文件 `MobileAppProvider`）
```js
if (!await R9()) return          // R9 = 隐私授权是否已同意（模块 4169）
let e = await Y6()                // 取版本
if (M.current !== R) return       // 用户切走则放弃
let a = await m.grantLoginPoints(platform, e.version)
await UF(a, { show, refresh })    // UF: granted===true 才弹窗+刷新
```
每次 App 回到前台都会调一次，靠服务端 `granted` 幂等去重。

#### 与桌面端端点的区别
| | 移动端 | 桌面端 |
|-|-|-|
|路径|`/api/web/mobile/v1/login/points/grant`|`/api/web/desktop/v1/login/points/grant`|
|`X-Client-Platform`|`app-android` / `app-ios`|`desktop-linux`|
|发放每日 300|✅|❌|
|发放首登 3000|✅（移动端登录奖励）|✅（桌面端登录奖励）|

APK 内**不存在**桌面端路径；反之桌面版未见移动端路径。两者是独立渠道。

#### 凭证可用性（实测）
桌面端 token 可直接调移动端端点：token 的 `aud` 仅 `xiaohuanxiong.com`，
无客户端类型绑定，服务端只校验 header。

**实测（2026-11-16 10:20，本机）**：
```bash
ofm-server claim     # → [raccoon] 签到跳过: 今日已领取（服务端幂等，每次登录只发一次）
```
返回 `granted: false` 而非 401/403，证明凭证与 header 均被接受，仅因当日已领被幂等拦下。

⚠️ **未实测**：跨天首次调用是否真返回 `granted: true` 并发 300（需次日验证，会消耗当日额度）。

### 实现要点(已完成)
`src/providers/raccoon/claim.rs`：
* `POST /api/web/mobile/v1/login/points/grant`，header 带
  `X-Client-Platform: app-android`、`X-Client-Version: v1.1.0`
* ⚠️ 版本号用 APK 的 `v1.1.0`，**不复用**桌面版常量 `api::CLIENT_VERSION`（`v1.2.0`）：
  冒充移动端却报 Electron 版本自相矛盾
* `granted: false` → `skipped: true`（**不是错误**），避免 `claim.log` 每天记假警报
* `granted: true` → 用 `popup.source` 翻译成人话（如「每日积分 300」）
* 顺带读一次余额填入 `ClaimReport.balance`

`api.rs` 新增 `post_envelope_with_headers`（原 `post_envelope` 只支持 bearer，无法带客户端标识头）。

### 每日积分过期与账单字段
余额分池(`GET /api/web/points/v1/balance`)：`available_points`(总)、`daily_points`、`monthly_points`、
`reward_points`、`topup_points`、`topup_frozen`。消耗顺序：每日额度 → 每月额度 → 奖励积分 → 加油包。

**余额接口不带任何到期字段**(实测全字段已核对)，到期信息只能从账单的 `expire_at` 取。
实测样例(11-16)：`available_points: 10998 = daily 585 + reward 10413`。
注意 `reward_points` 无到期字段，与每日积分为不同池子。

账单展示文案(`GET /api/web/points/v2/bills`)：
|字段/类型|含义|
|-|-|
|`biz_type: 'daily_grant'`|每日积分发放；展示 `grant_at · 有效期至 <expire_at> 24:00`|
|`biz_type: 'daily_expire'`|每日积分过期；`当日未登录` 表示当天没登录导致作废|
|`biz_type: 'reward_grant'`|奖励积分（登录奖励 / 新人礼包）|
|`grant_at` / `expire_at`|发放时间 / 失效时间；二者同一天时显示「当日 24:00」|

### 换 token 端点实测(本机，2026-11)
`POST /api/web/auth/v1/login_with_authorization_code` 以假 code 实测，返回：
```
HTTP 400 {"code":200035,"message":"authorization_code_not_found_error","details":"authorization code not found"}
```
* 端点真实存在且受理该请求体格式 `{"authorization_code":"..."}`
* 错误码 `200035` 与源码一致；**假 code 会回 HTTP 400**（非 200 信封）
* 故客户端不能只看 HTTP 状态，需解析 body 的业务码
