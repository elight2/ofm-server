# minimax 客户端协议（MiniMax Code）

**来源**：客户端源码 `~/tools/docs/agent-clients/minimax-code/packages/`（TypeScript 源，未打包）+
本机 mitmproxy 抓包 `test/capture_output/minimax_requests.jsonl`（2026-11-17，61 条 HTTP）

**标注方式**

| 标注 | 含义 |
|-|-|
| `[抓包]` | 抓包实录，含请求/响应原文 |
| `[源码]` | 直接读客户端 TS 源码（附文件路径） |
| `[未验证]` | 无直接证据，仅为推断 |
| `[实测]` | `ofm-server` 实现落地时的真实请求实测（2026-11-17）|

抓包覆盖：完整登录链（含设备码轮询 17 次）、`/v1/models` ×1、`/v1/messages` ×26、`/messages/count_tokens` ×4。
**未覆盖**：余额（§6 仅源码）、签到（§7 无任何证据）。

**实现进度**分两列：`[实测]` 标注的是本服务器实现落地时真实跑过的结果，
§9 列了已确认项与待验项，§11 列了各步状态。

## 0. 总结

* 客户端版本：`MiniMax/3.1.16`（`Electron/42.11.16`，`Chrome/148.0.7778.280`），抓包日期 2026-11-17
* 分析方式：源码静态阅读 + mitmproxy 全量抓包（Windows 测试机 → 本机代理）
* 上游协议是 **anthropic-messages**，不是 Chat Completions：对话走 `/messages`，
  认证双发 `x-api-key` + `authorization: Bearer`，SSE 流
* 登录是 **OAuth 2.0 Device Code Grant + PKCE**，`client_id=mcode-public`

### 本协议不支持, 但 Chat Completions 有的功能

| 功能 | 结论 |
|-|-|
| `temperature` / `top_p` / `top_k` | 官方客户端 26 次对话**一次都没发**（官方 UI 无此设置）。上游是否接受 `[未验证]` |
| `stop_sequences` / `metadata` / `service_tier` | 同上，从未发送 `[抓包]` |
| `tool_choice` | 官方客户端从未发送 `[抓包]`；但**上游实际支持**——四种形态均实测 200（§3.4），可正常映射 |
| `n>1` | 无 Anthropic 对应字段，`[未验证]` |
| `response_format: json_object` | Messages 侧映射到 `output_config.format`（`{type:"json_schema",schema}`）`[源码 local-turn-payload-transform.ts]`，官方对话未走到该分支 `[未验证]` |
| `logprobs` / `seed` / `user` / `prompt_cache_*` | 无；缓存只用 Anthropic `cache_control`（见 §3.5） |
| 音频输入 | 无（`modalities.input` 只有 text/image/video）`[抓包 models]` |

### 本协议与官方客户端的出入

1. **目录下发的 `baseURL` 是旧域名，不能用**：`/v1/models` 返回
   `options.baseURL = https://agent.minimaxi.com/mavis/api/v1/llm/v1`，
   但 26 次真实对话全部打到 `https://agent.minimax.cn/mavis/api/v1/llm/v1/messages`。
   源码 `packages/config/src/config.ts:1579` 把 `cn-prod` 预设定义为 `agent.minimax.cn`，
   把 `agent.minimaxi.com` 列入 `LEGACY_MANAGED_PRESET_BASE_URLS`。
   → **实现用预设域名，忽略目录里的 `baseURL`**，否则可能吃重定向或旧网关行为差异 `[抓包 + 源码]`
2. **思考强度有两个落点，按模型分，且互不通用**：`MiniMax-M3` 只认 `thinking.type`
   （`adaptive`/`disabled`），发 `output_config.effort` 完全无效；
   `MiniMax-M3.1-Flash` 只认 `output_config.effort`（真实档位，单调生效），
   发 `thinking.type=disabled` 会被 **HTTP 400** 拒绝。另需注意官方 UI 显示的
   M3 档位 `on`/`off` 是客户端内部字面量，直接发上游会被**静默忽略**（见 §5.2）
3. **`count_tokens` 与 `/messages` 的 body 白名单不同**：`count_tokens` 必须在边界剥掉
   `max_tokens`/`output_config`/`tools[].eager_input_streaming`，否则严格端点 400
   `[源码 messages-count-tokens-payload.ts:27]`
4. **登录无法纯命令行完成**：设备码只是第一步，账号确认页需要微信扫码 + **绑定手机号短信验证**
   （`code:1200044 未绑定手机号`），属浏览器侧，我方只实现「打印 user_code + 轮询」
5. **`whitelist` 里有目录里没有的模型**：`whitelist` 含 `MiniMax-M2.7`/`MiniMax-M2.7-highspeed`，
   但 `models` 只下发 M3 与 M3.1-Flash；M2.7 能否被 `/messages` 调用 `[未验证]`
6. **令牌不是 JWT**（2026-11-17 实测更正）：本文件早前写「access_token 是 JWT」，
   实际是不透明串 `mmoat_…` / `mmort_…`（各 60 字符），无 `.` 分隔，
   **无法从令牌读到期时间**，只能用 `expires_in` 自算。详见 §1.4
7. **上游会静默忽略不认识的思考字段**：M3 收到 `thinking.type=off`/`on`/`enabled`
   或 `output_config.effort` 都不报错，照常思考；只有 `disabled` 真正关闭。
   即「发了字段 ≠ 生效」，实现时不能靠不报错来判断是否生效（§5.2）
8. **浏览器必须由用户在图形会话里打开**：`xdg-open` 在没有 `DISPLAY`/`WAYLAND_DISPLAY`
   的环境（ssh、agent 会话等）下仍会 `spawn` 成功但不打开任何窗口，
   会产生「以为已打开、实际在等一个用户看不到的 `user_code`」的假象

### 域名

| 域名 / 端点 | 用途 |
|-|-|
| `https://account.minimax.cn`（海外 `account.minimax.io`） | 账号中心：OAuth 设备码、令牌、撤销、登录页 |
| `https://agent.minimax.cn/mavis/api/v1/llm/v1`（海外 `agent.minimax.io`） | 对话 `/messages`、`/messages/count_tokens` |
| `https://agent.minimax.cn/mavis/api/v1/models` | 模型目录（`region=cn&buildEnv=prod`） |
| `https://agent.minimaxi.com/mavis/api/v1/llm/v1` | 目录里下发的**旧** baseURL，源码列为 legacy |

`buildEnv` ∈ `dev|test|staging|prod`，`region` ∈ `cn|en`。
非 prod 环境的域名为 `.example.invalid` 占位（源码对外发布时替换），**只需实现 prod**。

## 1. 登录方式

### 1.1 端点与固定参数

| 项 | 值 |
|-|-|
| 设备码 | `POST https://account.minimax.cn/oauth2/device/code`（`application/x-www-form-urlencoded`）|
| 令牌 | `POST https://account.minimax.cn/oauth2/token`（同 content-type）|
| 撤销 | `POST https://account.minimax.cn/oauth2/revoke` |
| 用户确认页 | `https://account.minimax.cn/oauth-authorize?user_code=<USER_CODE>&client_surface=desktop&download_source=default` |
| `client_id` | `mcode-public` |
| `scope` | `agent.default` |
| `audience` | `agent-backend` |
| PKCE | `code_challenge_method=S256`；客户端生成 32 字节随机 → `code_verifier`（base64url 43 字符，无 padding）→ `code_challenge = base64url(sha256(verifier))`（同样 43 字符）`[抓包验证 + 源码 oauth-client.ts:80]` |

### 1.2 客户端侧流程（我方实现这三步 + 轮询）`[抓包]`

> **实现已跑通** `[实测]`：整套设备码 + 轮询拿令牌成功，无需额外头、无需 cookie；
> 上述 `user_code` 长度/格式、`interval=3`、`expires_in=300` 与抓包一致。

```text
POST /oauth2/device/code
content-type: application/x-www-form-urlencoded
accept: application/json
body: client_id=mcode-public&scope=agent.default&audience=agent-backend
      &code_challenge=<43 字符 base64url>&code_challenge_method=S256
→ 200 {"device_code":"<43 字符 base64url>","expires_in":300,"interval":3,
       "user_code":"WVC4-3DE3",
       "verification_uri":"https://account.minimax.cn/oauth-authorize",
       "verification_uri_complete":"https://account.minimax.cn/oauth-authorize?user_code=WVC4-3DE3"}
```

`user_code` 格式：`XXXX-XXXX`（4+4 大写字母/数字，抓包样本 `WVC4-3DE3`）。
响应头：`cache-control: no-store`、`trace-id: <32hex>`、`strict-transport-security`。

```text
POST /oauth2/token   每 3 秒一次（实测间隔：16 个区间全为 3.04~3.08s）
content-type: application/x-www-form-urlencoded
accept: application/json
body: grant_type=urn:ietf:params:oauth:grant-type:device_code
      &device_code=<device_code>&client_id=mcode-public&code_verifier=<verifier>
→ 400 {"error":"authorization_pending","error_description":"the user has not finished authorizing yet"}
→ 200 {"access_token":"<redacted>","expires_in":43200,
       "refresh_token":"<redacted>","scope":"agent.default","token_type":"Bearer"}
```

轮询终止条件与错误分支 `[源码 oauth-client.ts:135-186]`：

| 上游 | 处理 |
|-|-|
| `400` + `{"error":"authorization_pending"}` | 睡 `interval` 后继续 |
| `400` + `{"error":"slow_down"}` | `interval += 5s` 后继续 |
| 200 + `{"status":"pending"}` | 同待授权（兼容另一套设备码实现）|
| 200 + `{"status":"slow_down"}` | 同上加 5s |
| `access_denied` / `{"status":"denied"}` | 登录失败 |
| `expired_token` / `{"status":"expired"}` | 超时（`expires_in=300`）|
| 200 其他（无上述标记） | 输出去 `parseTokenGrant` |

* 轮询截止时间 = `now + expires_in*1000`（`300s`），到点未成功即 `expired_token`
* `verification_uri_complete` 可直接打开，用户登录后点「确认授权」
* 实测：从 `device/code` 到 token 200 共 **49s**（17 次轮询，中间夹着浏览器扫码/短信验证）

### 1.3 浏览器侧确认链（**不实现**，仅供参考）`[抓包]`

账号中心统一封套（除 OAuth 端点）：
`{data, statusInfo:{code, httpCode, message, serviceTime, requestID, debugInfo, serverAlert}}`，
**HTTP 200 也可能 `code != 0`**（如 `1200044`），必须解析业务码。
所有 `/v1/api/user/*` 与 `/oauth2/login` 请求都带一大串**公共 query 参数**：

```text
device_platform=web&biz_id=3&app_id=3001&version_code=22201&unix=<毫秒>&timezone_offset=28800
&lang=zh&sys_language=zh&uuid=<uuidv4>&device_id=<0|deviceID>&os_name=Windows&browser_name=chrome
&device_memory=8&cpu_core_num=4&browser_language=zh-CN&browser_platform=Win32
&screen_width=1366&screen_height=768&client=desktop
```

```text
GET  /oauth-authorize?user_code=…&client_surface=desktop&download_source=default  → 200 HTML
GET  /oauth2/device/authorize?user_code=…&app_id=3001        未登录 → 401
     {"error":"login_required","error_description":"account session is missing or expired",
      "login_url":"/unified-login?login_redirect=%2Foauth-authorize%3Fclient_id%3Dmcode-public…"}
GET  /unified-login?login_redirect=<encode(/oauth-authorize?client_id=mcode-public&user_code=…)>
     &source=agent_desktop&download_source=default                → 200 HTML（微信扫码页）
POST /v1/api/user/device/register   body {"uuid":"<uuidv4>"}
     → data {deviceID:578994059192999936, deviceIDStr:"…", userID:"", realUserID:"", isNewDevice:1}
POST /v1/api/user/getOpenData       body {"code":"<微信回调 code>"}
     → data {unionID:"oZDa…", openID:"oyTb…"}
POST /oauth2/login   body {loginType:"5", unionID, deviceID, login_redirect, download_source}
     → statusInfo.code 1200044 「未绑定手机号」（必须走短信绑定）
POST /v1/api/user/login/sms/send   body {phone, randStr:"@ycg", ticket:<腾讯防水墙>, countryCode:"+86"}
     → statusInfo.code 0
POST /oauth2/login   body {loginType:"5", phone, unionID, code:"<短信码>", countryCode:"+86",
                           deviceID, login_redirect, download_source}
     → {code:0, msg:"ok", data:{user_id, str_user_id, str_biz_uid,
         username:"MiniMax546824", avatar, is_register:0}}
     并写入会话 cookie
GET  /oauth2/device/authorize?user_code=…&app_id=3001 → {"status":"pending","expires_in":254,
     "client_info":{"client_name":"MiniMax Code"},
     "audience":{"name":"agent-backend","display_name":"MiniMax Code"},
     "scopes":[{"name":"agent.default","display_name":"MiniMax Code Default",…}],
     "account_info":{"name":"探索者9417","identifier":"<手机号>","avatar":"…"},
     "groups":[{"group_id":"2109949810109846391",…}]}
POST /oauth2/device/authorize  body {user_code, decision:"approve", group_id:"<取自上一响应 groups>"}
     → {"status":"approved"}
```

`loginType:"5"` = 微信登录，`is_register:0` 说明登录时自动注册。
浏览器侧请求**必须带 cookie**（`POST /oauth2/device/authorize` 还带 `origin: https://account.minimax.cn`）；
客户端侧 `/oauth2/device/code`、`/oauth2/token` **不带 cookie、不带 origin** `[抓包]`。

> 两个 `expires_in` 不是同一个：设备码的是 `300`（`/oauth2/device/code`）；
> 浏览器扫码会话的是 `254`（`GET /oauth2/device/authorize`，随扫码倒计时递减）。

### 1.4 令牌与凭证存储

* `access_token`：Bearer，`expires_in = 43200`（12h）`[抓包]`
* `refresh_token`：长期，续期用（§2）
* 客户端租约策略：取令牌时要求**剩余有效期 ≥ 30s**，否则先刷新 `[源码 tui/src/tui/launcher.ts:213]`
* **令牌形态（2026-11-17 实测更正）**：两者都是**不透明串**，不是 JWT：
  * `access_token` 形如 `mmoat_<52 字符>`，共 60 字符
  * `refresh_token` 形如 `mmort_<52 字符>`，共 60 字符
  * 均无 `.` 分隔符 → **无法从令牌本身读到期时间**，只能用响应里的 `expires_in`
    自己算 `expires_at` 落盘。`[实测，与本文件早前写的「access_token 是 JWT」矛盾，以实测为准]`
  * 源码 `oauth-client.ts:298-320` 里「解 JWT 取 `sub`/`account_id`、`scope` 缺失时从 JWT 取」
    这条分支在本供应商的令牌上**走不到**（非 JWT），不影响登录链
* 建议 `credential.toml`（与项目其他供应商一致）：

```toml
access_token = "..."
refresh_token = "..."
expires_at = <毫秒时间戳>   # 收到响应时刻 + expires_in*1000
```

`expires_at` 是本实现判断「是否需要刷新」的**唯一**依据（无 JWT 可解），
缺失时一律视为需要刷新。

刷新令牌失败/过期 → 必须重新走登录（§1），无其他恢复路径 `[源码 auth-core.ts:handleUnauthorized]`。

### 1.5 登录错误

| 上游 | 含义 |
|-|-|
| `400 {"error":"authorization_pending"}` | 未授权，继续轮询 |
| `400 {"error":"access_denied"}` | 用户拒绝 |
| `400 {"error":"expired_token"}` | 设备码过期（>300s）|
| `400 {"error":"invalid_token_response"}`（客户端自造） | 响应缺 `access_token`/`refresh_token`/`token_type!=Bearer`/缺 `expires_in`/scope 不含 `agent.default` `[源码 oauth-client.ts:290]` |

## 2. 令牌续期方式

**端点与参数**（同一 `/oauth2/token`）`[源码 oauth-client.ts:189]`：

```text
POST https://account.minimax.cn/oauth2/token
content-type: application/x-www-form-urlencoded
body: grant_type=refresh_token&refresh_token=<RT>&client_id=mcode-public
      &scope=agent.default&audience=agent-backend
```

* 响应格式与设备码成功响应相同（`access_token`/`expires_in`/`refresh_token`/…）；
  若响应不带新 `refresh_token`，沿用旧 RT `[源码 oauth-client.ts:295]`
* **本次抓包未触发刷新**（access_token 有效期内），上表为源码定论，**实测未验证** `[未验证]`
* **本实现**已写好 `minimax/auth.rs::refresh` + 自动续期（凭证进入 30s 窗口时触发），
  但尚未实际触发过一次——需等令牌接近到期或手动抹去 `expires_at` 才能测
* 401 处理：对话收到 401 → 刷新一次后重试 `[源码 auth-core.ts:330 handleUnauthorized]`
* 撤销（登出用）：`POST /oauth2/revoke`，`token=<RT>&token_type_hint=refresh_token&client_id=mcode-public` `[源码]`
* 客户端有跨进程令牌共享（Unix socket / named pipe），我方单进程实现**不需要**

**另有一条旧式续期端点**（不走 OAuth，仅 TUI 启动时兜底用）`[源码 matrix-account-client.ts:420]`：

```text
POST https://agent.minimaxi.com/v1/api/user/renewal?device_platform=mcode&…（§6 那组 query）
accept: application/json
content-type: application/json
user-agent: MiniMaxCode
authorization: Bearer <access_token>
yy / x-timestamp / x-signature：同 §6 签名头（此 POST **无 body**，故 `yy` 用 `_{}`、
`x-signature` 不带 body）
→ {"data":{"token":"<新 access_token>"}, …}
```

它只换 `access_token`、**不返回新 refresh_token**；401/403 则判定登录失效。
我方实现用标准 `refresh_token` grant 即可，**不用这条**；仅备份说明客户端存在两种续期路径。

## 3. Chat Completions 映射

上游是 **anthropic-messages**。下游 `POST /v1/chat/completions` 需转成 Messages 协议。

### 3.1 端点与请求头

```text
POST https://agent.minimax.cn/mavis/api/v1/llm/v1/messages
content-type: application/json
x-api-key: <access_token>
authorization: Bearer <access_token>
anthropic-version: 2023-06-01
user-agent: MiniMaxAgent
```

`[抓包]` 26 次对话**全部**带 `x-api-key` + `authorization` 双认证 + `anthropic-version: 2023-06-01`。

主要请求头并集（26/26 全部出现，除注明外）：

| 头 | 官方值 | 必需性 |
|-|-|-|
| `content-type` | `application/json` | 必需 |
| `accept` | `application/json` | 可选 |
| `accept-encoding` | `gzip, deflate, br, zstd` | 可选 |
| `x-api-key` | `<access_token>` | 见下 |
| `authorization` | `Bearer <access_token>` | 见下 |
| `anthropic-version` | `2023-06-01` | 极可能必需 |
| `user-agent` | `MiniMaxAgent`（**不带** `Mozilla/…` 前缀） | 可选（WAF 可能校验）|
| `anthropic-dangerous-direct-browser-access` | `true` | 26/26 都发，但语义只是浏览器直连绕 CORS；服务端调用无意义 |
| `x-mavis-agent-id` | `main` | 客户端 agent 标识 |
| `x-mavis-session-id` | `mvs_<32hex>` | 会话 ID，响应头回显同名头 |
| `x-mavis-timezone-offset` | `28800` | 时区秒偏移（东八区） |
| `x-mavis-turn-phase` | `initial`(8) / `iteration`(2) / 缺席(16) | 首轮与工具迭代轮标记 |
| `anthropic-beta` | `interleaved-thinking-2025-05-14` | 仅 3/26，见 §5.3 |
| `x-stainless-{arch,lang,os,package-version,retry-count,runtime,runtime-version,timeout}` | `x64/js/Windows/0.91.1/0/node/v24.19.0/1200` | Anthropic 官方 SDK 自动生成，无业务含义 |
| `priority` / `sec-fetch-{dest,mode,site}` | `u=4, i` / `empty,no-cors,none` | 浏览器/Chromium fetch 自动加，无业务含义 |

**各头必需性全部 `[未验证]`**（抓包无法删头验证）。实现取最小集：
`content-type`、`anthropic-version`、`user-agent: MiniMaxAgent` + 双认证头。

> `x-api-key` 与 `authorization: Bearer` **同时发**是 Anthropic SDK 的 `apiKey` + `authToken` 两条通道并存，
> 值相同。我方只发两者之一是否可行 `[未验证]`；稳妥做法是两者都发同值。
> `x-api-key` 值为 OAuth access_token 而非 `sk-ant-oat…` 前缀，故 pi 的 `isOAuthToken` 判定不适用。

### 3.2 请求体字段映射 `[抓包，26 次实录]`

| Chat Completions | 上游 Messages body | 依据 |
|-|-|-|
| `model` | `model`：完整 ID（`MiniMax-M3.1-Flash` / `MiniMax-M3`），**不是** `name`（`M3.1-Flash`） | `[抓包]` |
| `max_tokens` | `max_tokens`：正常对话 `128000`，标题/探测 `1000` | `[抓包]` |
| `stream` | `stream: true`（26/26 恒为 true） | `[抓包]` |
| `messages[].role=system` 或 `developer` | 抽出拼成 `system: [{type:"text", text:"…"}]`（**数组**，非字符串） | `[抓包]` |
| `messages[].role=user` | `{role:"user", content:[{type:"text",text:"…"}]}`；**数组形态是主流** | `[抓包]` |
| `messages[].role=assistant` + `content` | `{role:"assistant", content:[{type:"text",text:"…"}]}` | `[抓包]` |
| `messages[].role=assistant` + `tool_calls` | `{role:"assistant", content:[{type:"thinking",…}?, {type:"text",…}?, {type:"tool_use", id, name, input}]}`（多个 `tool_use` 并列，抓包见一次两个） | `[抓包]` |
| `messages[].role=tool` | `{role:"user", content:[{type:"tool_result", tool_use_id:<id>, content:"<纯字符串>", is_error:false}]}` | `[抓包]` 14/14 次 `content` 都是**字符串**，不是 text 块数组 |
| `tools[].function.name/description/parameters` | `tools[{name, description, input_schema}]`（`input_schema` 是**对象**，不是字符串） | `[抓包]` |
| — | `tools[].eager_input_streaming: true` | `[抓包]` 18 次带 tools 的请求全带此字段；是否必需 `[未验证]` |
| `temperature` / `top_p` / `stop` / `tool_choice` / `user` / `n` / `logprobs` | **不发** | 官方 26 次全未发；支持性 `[未验证]` |
| `response_format` | `output_config.format = {type:"json_schema", schema}` | `[源码]`，官方未走到，`[未验证]` |

`content` 块类型分布（`[抓包]` 26 次汇总，按块计数）：

| 块类型 | 次数 | 备注 |
|-|-|-|
| `text` | 107 | 其中 24 个带 `cache_control` |
| `thinking` | 21 | 均带 `signature` |
| `tool_use` | 14 | |
| `tool_result` | 14 | 其中 2 个带 `cache_control` |
| `image` | **0** | 目录 `modalities.input` 声明支持，但本次未走到 |

图片块按 Anthropic 标准实现 `[源码 pi-mono anthropic.ts:convertContentBlocks]`：

```json
{"type":"image","source":{"type":"base64","media_type":"image/png","data":"<base64>"}}
```

* 仅支持 base64，**无 `url` 来源类型** `[源码]`；下游 `image_url` 带 http URL 时需先下载再转 base64
* `media_type` 枚举 `image/jpeg|image/png|image/gif|image/webp` `[源码]`
* 只有图无文字时，客户端会在前面补一个占位 `{type:"text",text:"(see attached image)"}` `[源码]`
* 视频：客户端在 payload 边界把 `source.media_type` 以 `video/` 开头的块改成 `{"type":"video",…}`
  `[源码 local-turn-payload-transform.ts:patchVideoBlock]`；Chat Completions 无视频输入，**不实现**

实际 body（`[抓包]` 15:06:26 首次对话，字段裁剪）：

```json
{
  "model": "MiniMax-M3.1-Flash",
  "max_tokens": 128000,
  "stream": true,
  "system": [{"type": "text", "text": "…", "cache_control": {"type": "ephemeral"}}],
  "messages": [
    {"role": "user", "content": "<system-reminder> …"},
    {"role": "user", "content": [{"type": "text", "text": "…", "cache_control": {"type": "ephemeral"}}]}
  ],
  "tools": [{"name": "read", "description": "…", "eager_input_streaming": true,
             "input_schema": {"type": "object", "properties": {}}}],
  "output_config": {"effort": "default"}
}
```

`system` 形态（`[抓包]` 强约束）：

* 恒为**数组**，且 18/18 次都是**恰好 1 个** `{type:"text"}` 块，该块**恒带** `cache_control`
* 8/26 次**完全不带** `system` 字段（不是空数组）——`system` 缺失是合法请求

`content` 的字符串形态（`[抓包]`）：仅出现在 `messages[0]`，10 次（且这 10 次正好是 `max_tokens=128000`
的多轮长会话；`max_tokens=1000` 的短请求全部用数组）。字符串内容是被塞进第一条 user 消息里的
`<system-reminder>` 段纯文本，**不是** JSON 文本。我方实现统一用数组形态即可（`[未验证]` 是否完全等价）。

### 3.3 思考过程回传

`[抓包]` assistant 历史消息里的 thinking 块形状：

```json
{"type": "thinking", "thinking": "User keeps saying hi. Just respond briefly.",
 "signature": "73daefb188184fcf12a6e8bb9e124dc43e6f914041ae96dc0ec13f740ca39a99"}
```

* `signature`：**64 个 hex 字符**（32 字节），21/21 个 thinking 块长度全部一致
* 7/26 次请求携带历史 thinking 块（多轮续轮），其余首轮请求无
* 块序：`thinking` 恒在 `text` **之前**，含工具时 `tool_use` 在 `text` **之后**

Anthropic 语义要求 thinking 块连签名原样回传，否则 400 `[未验证]`（本次未发送缺失签名的样本）。
我方 server 只做无状态转发；下游 Chat Completions 无 thinking 块概念 → 只能**丢弃**历史 thinking，属已知能力损失。

### 3.4 工具

* `input_schema` 必须是 JSON **对象**（`[抓包]`）；下游 `parameters` 本就是对象，直接搬
* `tool_use.id` 格式：`call-<uuid v4>`（如 `call-b6cef677-d568-49b9-ae31-77a285492c1e`），
  与 `tool_result.tool_use_id` 一一对应，`[抓包]` 14 对全部匹配、无孤儿
* `eager_input_streaming: true`：`[抓包]` 18/18 次带 `tools` 的对话请求**每个** tool 都带；
  而 4 次 `count_tokens` 的 tool **全部不带**（属边界剥离，见 §3.7）。是否必需 `[未验证]`
* 工具结果统一进 `role:"user"` 的 `tool_result` 块（Anthropic 语义），**不是** `role:"tool"`；
  多个结果并列在同一个 user message 的 `content` 数组（抓包见一次两个）
* `tool_result.content` 为**字符串**，`is_error` 恒有（`false`/`true`）

#### 实测补充 `[实测 2026-11-17]`

* **`tool_choice` 四种形态全部可用**（此前 `[未验证]`）：实测
  `{"type":"any"}` / `{"type":"tool","name":"t"}` / `{"type":"auto"}` / `{"type":"none"}`
  均返回 200。故 CC 四值可直接映射：`auto→auto`、`none→none`、
  `required→any`、`function(name)→{type:"tool",name}`。
  **无工具时不要发 `tool_choice`**（`any` 会因无工具被拒）。
* **`tool_result.content` 给字符串完全可用**：实测带 `is_error:false` 的字符串结果返回 200。
* **`max_tokens` 可省略**：实测不传该字段仍 200 且正常出流。本实现固定给满
  `128000`（目录 `limit.output`），因为下游 `ChatRequest` 无该字段。
* **`x-api-key` 不是必需的**：实测只发 `authorization: Bearer` 也能成功。
  本实现仍按抓包习惯**双发同值**，兼容性最好。
* **错误体形状**（实测，HTTP 400）：
  ```json
  {"type":"error","error":{"type":"invalid_request_error",
   "message":"invalid params, unknown model 'No-Such-Model' (2013)"},
   "request_id":"074b7dc06304bb6dbe8eea615d862ad3"}
  ```
  `type` 取值观测到 `invalid_request_error`；其余按 Anthropic 标准
  （`authentication_error`/`rate_limit_error`/`overloaded_error`/`not_found_error`）映射。

### 3.5 缓存断点 `cache_control`

`[抓包]` 值恒为 `{"type":"ephemeral"}`，落点**固定 3 处**（Anthropic 上限 4 个）：

| 位置 | 出现条件 | 例 |
|-|-|-|
| `system[0]`（唯一块） | 带 `system` 就带 | 18/18 |
| `tools[<最后一个>]` | 带 `tools` 就带，且**只在最后一个工具上** | index 28/29、0/1 |
| `messages[<最后一条>].content[<最后一块>]` | 只要有 messages 就带；落在 `text` 或 `tool_result` 块上，**从不**落在 `thinking`/`tool_use` 上 | m0[0]、m11[1]、m13[0] … 全为末消息末块 |

`count_tokens` 请求也带同样的三处断点（`[抓包]` 4/4）。

**是否必需 `[未验证]`**。建议实现时**不发**：下游 Chat Completions 无对应字段，
且断点位置依赖客户端固定布局（末条消息末块），我方消息布局不同，乱放可能反而降低命中率。

### 3.6 流式响应

`[抓包]` 26/26 响应头（无 `content-length`，SSE）：

```text
content-type: text/event-stream; charset=utf-8
cache-control: no-cache
content-encoding: br
vary: Accept-Encoding, Accept-Encoding        （重复两次）
x-request-id: <32hex>
x-session-id: <32hex>
x-mm-request-id: <同 x-trace-id>
x-trace-id: <32hex>
trace-id: <32hex>, <32hex>                     （逗号分隔两个）
x-mavis-session-id: mvs_<32hex>               （回显请求头）
server-timing: gw;dur=315, up;dur=1030         （网关 315ms + 上游 1030ms）
alb_receive_time: <unix 秒.微秒>
alb_request_id: <sha1 40hex>
```

**SSE 事件体未被抓包工具保存**（`test/minimax_capture.py` 只存 JSON 响应体，流被丢弃）
→ 曾按 Anthropic Messages 标准推断；**2026-11-17 已实测补全**（见下）：

#### 实测帧序列 `[实测 2026-11-17]`

```text
event: message_start        data: {"type":"message_start","message":{...,"usage":{"input_tokens":0,"output_tokens":0}}}
event: ping                 data: {"type":"ping"}                       ← 文档原缺，必须忽略
event: content_block_start  data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}
event: content_block_delta  data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"…"}}
event: content_block_delta  data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"<64hex>"}}
event: content_block_stop   data: {"type":"content_block_stop","index":0}
event: message_delta        data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{…}}
event: message_stop         data: {"type":"message_stop"}
```

与推断的差异（全部实测）：

1. **有 `ping` 帧**，文档原未记载，解析必须忽略。
2. **`message_start` 的 `usage` 全是 0**，真实用量只在 `message_delta`。
3. `content_block_start` 的 `content_block.type` 实测三种：`text` / `thinking` / `tool_use`。
   * `thinking` 块用 `thinking_delta`（增量在 `thinking` 字段）；
   * `tool_use` 块起始帧带 `id`/`name`，`input` **恒为空对象 `{}`**；
   * `tool_use` 的增量是 `input_json_delta`，其 `partial_json` 逐片拼接，
     **实测首片为空串 `""`**，随后才是 `"{"` / `"\"city\":\"北京\""` / `"}"`。
4. `signature_delta` 在 thinking 块结束前出现，`content_block_stop` 紧随其后；下游无对应概念，丢弃。
5. **`index` 是全局块序号**（thinking=0、text=1、tool_use=2…），
   而下游 `tool_calls[].index` 必须在工具内部从 0 连续编号 → **必须重映射**。

#### usage 字段（实测，**含关键陷阱**）

`message_delta` 的真实形状：

```json
{"input_tokens":79,"output_tokens":223,"cache_read_input_tokens":128,
 "service_tier":"standard","output_tokens_details":{"thinking_tokens":123}}
```

**`input_tokens` 不含缓存命中部分**——这是本项目实现时最易错的一点：

* 同一 payload 走 `/messages/count_tokens` 返回 **206**（M3.1-Flash）/ **180**（M3）；
* 流内 `message_delta` 却只给 `input_tokens=78~79`，另计 `cache_read_input_tokens=128`；
* `78 + 128 = 206` ✅ 与 count_tokens 吻合。

故下游 `prompt_tokens` 必须算作 **`input_tokens + cache_read_input_tokens`**，
`cached_tokens` 填 `cache_read_input_tokens`，`reasoning_tokens` 取
`output_tokens_details.thinking_tokens`。
（若只取 `input_tokens`，下游每次都会少报掉全部缓存 token，且 prompt 缓存统计失真。）

`pi` 的 anthropic provider（`third_party/pi-mono/packages/ai/src/providers/anthropic.ts`）按此形状解析，
其中包括 `usage.cache_read_input_tokens` / `cache_creation_input_tokens`。

### 3.7 count_tokens

```text
POST https://agent.minimax.cn/mavis/api/v1/llm/v1/messages/count_tokens
content-type: application/json
x-api-key + authorization: Bearer + user-agent: MiniMaxAgent
（**不带** anthropic-version、**不带** accept、**不带** anthropic-dangerous-*、**不带** x-stainless-*）

body: {"model":"MiniMax-M3.1-Flash","messages":[...],"stream":false,
       "system":[...],"thinking":{"type":"adaptive","display":"summarized"}}
→ 200 {"input_tokens": 6653}
```

`[抓包]` 4 次实测（M3.1-Flash 无 tools、有 tools，M3 无 tools、有 tools）：

* 请求体键只有 `model/messages/stream/system/tools/thinking`，`stream` 恒为 `false`
* **必有** `thinking: {"type":"adaptive","display":"summarized"}`（两个模型都是 adaptive + summarized）
* **无** `output_config`、**无** `max_tokens`、**无** `tool_choice`
* `tools[].eager_input_streaming` 被**剥离**（keys 只剩 `name/description/input_schema`）
* 响应体仅 `{"input_tokens": N}`，无 `output_tokens`
* 同样的 messages+system+tools，`M3` 返回 6645、`M3.1-Flash` 返回 6653（差 8），有 tools 时 28646 vs 28654
  （同样差 8）——`model` 字段本身带来固定 8 token 开销（差值为常量，故只是模型名 tokenize 结果不同）

严格字段白名单 `[源码 messages-count-tokens-payload.ts:27]` 允许 `model/messages/system/tools/thinking/tool_choice`，
多传 `max_tokens`/`output_config`/`eager_input_streaming` 会 400 `[未验证]`（源码注释如此声明）。

我方 OpenAI 侧无对应端点，**不实现**（`pi` 不调用）。

### 3.8 响应映射

```text
stop_reason                     → finish_reason
end_turn / stop_sequence        → "stop"
max_tokens                      → "length"
tool_use                        → "tool_calls"
（无此值）                       → "content_filter" 不用
```

| 上游 Messages usage | 下游 Chat Completions |
|-|-|
| `input_tokens` **+** `cache_read_input_tokens` | `prompt_tokens`（**必须相加**，见 §3.6） |
| `output_tokens` | `completion_tokens` |
| `prompt_tokens + output_tokens` | `total_tokens` |
| `cache_read_input_tokens` | `prompt_tokens_details.cached_tokens` |
| `output_tokens_details.thinking_tokens` | `completion_tokens_details.reasoning_tokens` |
| `cache_creation_input_tokens` | 无对应位，丢弃 `[项目现有做法同 trae]` |

* `thinking` 块 → 下游 `reasoning_content` 扩展字段（`pi` 识别，同 trae/raccoon 做法）
* `message_start` 的 `message.id` → 下游 `chatcmpl-` 前缀 id（如果上游 id 格式不兼容，自造即可）
* 下游 `stream:false`：上游 `stream` 恒为 true（§3.2），必须**聚合完整 SSE** 后拼成单个
  `chat.completion` 对象返回；中途 `event: error` 应报错而不是吐半截内容
* 上游 **无** `id`/`model` 字段的 SSE 帧不能转发；每帧 `choices[0].delta` 只带增量内容

## 4. models 端点映射

```text
GET https://agent.minimax.cn/mavis/api/v1/models?region=cn&buildEnv=prod
accept: application/json
accept-encoding: gzip, deflate, br, zstd
authorization: Bearer <access_token>       # 只有 Bearer，没有 x-api-key
user-agent: <Electron UA>                  # Mozilla/… MiniMax/3.1.16 Chrome/148… Electron/42.11.16
```

`[抓包]` 与对话接口的头差异：目录接口**不带** `x-api-key`、**不带** `anthropic-version`、
**不带** `x-mavis-*`、UA 是**桌面浏览器完整 UA**而非 `MiniMaxAgent`。
响应头：`content-type: application/json; charset=utf-8`、`content-encoding: br`、
`vary: Accept-Encoding`、`x-request-id`、`trace-id`、`x-trace-id`、`alb_receive_time`、`alb_request_id`。

`[源码 model-system/connectivity/provider-request.ts]` `buildModelDiscoveryHeaders` 同时设
`authorization` 与 `x-api-key`，但抓包只见到 `authorization`（因为此供应商的 `apiKey` 字段就是 `sk-xxx` 占位，
真实流程走 OAuth）→ 实现只发 `authorization: Bearer` 即可 `[实测]`（2026-11-17 跑通，200）。

> **路径不能从对话 baseURL 推导**：源码 `providerModelsUrls(anthropic-messages)` 会去拼
> `${base}/v1/models` → `…/mavis/api/v1/llm/v1/models`，但抓包的真实路径是
> `…/mavis/api/v1/models`（少了 `llm/v1` 一级）。→ **实现写死抓包路径**，不要用源码的拼接规则。
> 两个 query 参数 `region=cn&buildEnv=prod` 也必须带（缺了行为 `[未验证]`）。

> 易混：源码里还有 `GET /mavis/api/v1/models-dev/catalog`，那是 **BYOK provider 预设目录**（models.dev
> 第三方模型供应商清单 + 镜像 `https://filecdn.minimax.chat/public/models-dev/catalog/<sha>/api.json`），
> 与本节的 **MiniMax 自有模型列表** 无关，本实现不用。

`[抓包]` 响应（结构裁剪，保留全部关键字段）：

```json
{
  "version": "1.0",
  "ttlSeconds": 300,
  "providers": [{
    "providerId": "minimax",
    "config": {
      "name": "MiniMax",
      "npm": "@ai-sdk/anthropic",
      "models": {
        "MiniMax-M3": {
          "name": "M3", "attachment": true, "reasoning": true, "temperature": true, "tool_call": true,
          "limit": {"context": 512000, "output": 128000},
          "modalities": {"input": ["text","image","video"], "output": ["text"]},
          "options": {"reasoningSummary": "auto"},
          "variants": {"none-thinking": {"thinking": {"type": "disabled"}},
                       "thinking": {"thinking": {"type": "adaptive"}}},
          "thinking_config": {"mode": "switchable", "default_value": "true"},
          "capabilities": {"support_files_api": true, "files_api_upload_endpoint": "/v1/files/upload",
                           "max_attachments_count": 9, "max_image_bytes_inline": 10485760,
                           "max_video_bytes_inline": 52428800, "max_request_body_bytes": 67108864},
          "context_window_options": [512000, 1000000],
          "context_window_option_hints": {"1000000": "higher_usage"}
        },
        "MiniMax-M3.1-Flash": {
          "name": "M3.1-Flash", "attachment": true, "reasoning": true, "temperature": true, "tool_call": true,
          "limit": {"context": 512000, "output": 128000},
          "modalities": {"input": ["text","image","video"], "output": ["text"]},
          "options": {"reasoningSummary": "auto"},
          "variants": {"none-thinking": {"thinking": {"type": "disabled"}},
                       "thinking": {"thinking": {"type": "adaptive"}}},
          "thinking_config": {"mode": "forced_on"},
          "capabilities": {"support_files_api": true, "files_api_upload_endpoint": "/v1/files/upload",
                           "max_attachments_count": 4, "max_image_bytes_inline": 10485760,
                           "max_video_bytes_inline": 52428800, "max_request_body_bytes": 67108864},
          "context_window_options": [512000, 1000000],
          "context_window_option_hints": {"1000000": "higher_usage"},
          "effort_options": ["default","low","medium","high","xhigh","max"],
          "default_effort": "default"
        }
      },
      "model_order": ["MiniMax-M3.1-Flash", "MiniMax-M3"],
      "whitelist": ["MiniMax-M2.7-highspeed","MiniMax-M2.7","MiniMax-M3","MiniMax-M3.1-Flash"],
      "options": {"apiKey": "sk-xxx", "baseURL": "https://agent.minimaxi.com/mavis/api/v1/llm/v1"}
    }
  }]
}
```

映射规则：

| 上游字段 | 下游 `/v1/models` | 备注 |
|-|-|-|
| `config.models` 的 key | `id`（`minimax/MiniMax-M3.1-Flash`） | 用 key，不用 `name` |
| `config.model_order` | 列表顺序 | `["MiniMax-M3.1-Flash", "MiniMax-M3"]`（注意与 `models` 的 key 字典序相反）|
| `name` | 不用（展示名，`M3`/`M3.1-Flash`） | |
| `limit.context` | 上下文窗口 512000 | `context_window_options` 含 1000000，切换方式 `[未验证]` |
| `limit.output` | 最大输出 128000 | 与抓包的 `max_tokens: 128000` 吸合 |
| `reasoning` | 是否支持思考 | 两模型均 `true` |
| `attachment` | 是否支持附件 | 两模型均 `true` |
| `modalities.input` 含 `image` | 是否支持图片 | 两模型均 `[text,image,video]` |
| `options.reasoningSummary = "auto"` | 上游自行决定是否输出思考摘要 | 抓包里对话请求从不发 `display`，与 `auto` 一致 |
| `thinking_config.mode` | 思考档位形态（§5.1） | `switchable` / `forced_on` |
| `effort_options` / `default_effort` | 可选档位集 | 只 M3.1-Flash 有 |
| `ttlSeconds: 300` | 目录建议缓存 300 秒 | `[抓包]`；源码内未见读取该字段（搜 `ttlSeconds` 无命中），我方可不缓存 |
| `whitelist` | **不用于列表** | 含 M2.7，但 `models` 未下发 → 是否可调用 `[未验证]` |
| `variants` | 不用 | 与 `thinking_config` 重复的思考开/关预设 |
| `capabilities.*` | 不用 | 文件 API、附件上限、body 上限（64MiB） |
| `options.apiKey` / `options.baseURL` | **忽略** | 占位值/旧域名，见 §0 出入 1 |

海外：`region=en&buildEnv=prod` + `agent.minimax.io` `[未验证]`。

### 本实现落地后的补充 `[实测]`

* 鉴权：只带 `authorization: Bearer` + `accept: application/json` 即返回 200，
  `accept-encoding` / `user-agent` 都不是必需（仍照抓包带上 UA）
* 响应实际内容与上面节选**完全一致**，无任何多余/缺失字段
* 列表结果：`minimax/MiniMax-M3.1-Flash`、`minimax/MiniMax-M3`（顺序与 `model_order` 一致）
* 倍率：目录**不提供**任何倍率/折扣字段 → 对外模型名**不带** ` (x…)` 后缀
  （与 raccoon 不同，raccoon 目录里有 `billing_effective_multiplier`）
* `ttlSeconds: 300` 本实现**不缓存**：每次 `/v1/models` 都实时请求目录，
  因为目录是模型可用性的实时真相源（如 M2.7 一旦下发就能立即出现）

## 5. reasoning_effort 字段映射

### 5.1 两个落点（按模型分）

`[抓包 + 源码 tui/.../local-turn-payload-transform.ts:patchManagedThinkingPayload]`

| 模型 | `thinking_config.mode` | 上游字段 | 允许值 |
|-|-|-|-|
| `MiniMax-M3` | `switchable`（默认 `"true"`） | `thinking`：`{"type":"adaptive"}` / `{"type":"disabled"}` | 二值开关，**无档位** |
| `MiniMax-M3.1-Flash` | `forced_on` | `output_config`：`{"effort":"…"}`，**不发 `thinking`** | `default`/`low`/`medium`/`high`/`xhigh`/`max` |

⚠️ 上表「允许值」是**上游实际接受的字段值**，不是客户端 UI 的档位名。
官方 UI 对 M3 显示 `on`/`off`，但那是客户端内部字面量，会在发请求前被翻译成
`adaptive`/`disabled`；把 `on`/`off` 直接发给上游会被**静默忽略**（§5.2 实测）。

抓包交叉验证（26 次，按模型与字段组合计数）：

| 模型 | thinking | output_config | 次数 |
|-|-|-|-|
| M3.1-Flash | 缺席 | `{"effort":"default"}` | 2 |
| M3.1-Flash | 缺席 | `{"effort":"low"}` | 8 |
| M3.1-Flash | 缺席 | `{"effort":"medium"}` | 2 |
| M3.1-Flash | 缺席 | `{"effort":"high"}` | 2 |
| M3.1-Flash | 缺席 | `{"effort":"xhigh"}` | 4 |
| M3.1-Flash | 缺席 | `{"effort":"max"}` | 2 |
| M3.1-Flash | `{"type":"adaptive","display":"summarized"}` | `{"effort":"low"}` | 1（**仅 count_tokens**）|
| M3 | `{"type":"adaptive"}` | 缺席 | 2 |
| M3 | `{"type":"disabled"}` | 缺席 | 3 |

结论：
* **M3.1-Flash 从不在对话请求里发 `thinking`**；`thinking` 只在 `count_tokens` 出现（带 `display:"summarized"`）
* **M3 从不发 `output_config`**
* `display` 字段在对话请求里**完全没出现**（`[源码]` 里默认值 `"summarized"`，可能被网关剥离或客户端仅在计数路径加）

### 5.2 档位语义

* **M3 的档位值是字面量 `"on"` / `"off"`**，不是 `low`/`high`：
  `isMiniMaxM3ThinkingMode(v) = v==='on'||v==='off'` `[源码 thinking.ts:36]`，
  映射到 `thinking:{type: on?'adaptive':'disabled'}` `[源码 resolveMiniMaxM3ThinkingProtocol]`。
  官方 UI 对 M3 只给「开/关」二档
* **M3.1-Flash 的档位值是目录里的 `effort_options`**（`default`/`low`/`medium`/`high`/`xhigh`/`max`），
  用户所选值**原样**写进 `output_config.effort` `[源码 model-ref.ts:397]`
* `default`：客户端 UI 的「默认」，**原样发给上游**，不是「不发送」（抓包 2 次 `{"effort":"default"}`）
* 档位集合**禁止硬编码**，随目录 `effort_options` 变化；`default_effort` 为默认值
* M3 的 `enabled` 取自 `effort==='on'`，且 `minimumMaxTokens = 1024`（防止关思考时 max_tokens 过小导致空响应）`[源码 thinking.ts:102]`
* 内部 pi 档位名只是运输键：`thinkingLevelMap: {high: <用户值>}`，让 pi 的 `high` 透传成原值 `[源码]`

#### 上游真实接受范围 `[实测 2026-11-17]`

上文「M3 的档位值是字面量 `on`/`off`」说的是**官方客户端 UI 的内部取值**，
它在上游请求体里被翻译成 `thinking.type: adaptive|disabled`。
直接把这些字面量发到上游会被**静默忽略**（不报错、照常思考）——这点极易踩坑，故单列实测结果。

固定 prompt（概率推导题，`max_tokens=3000`），读 `message_delta.usage.output_tokens_details.thinking_tokens`：

**M3（`switchable`）**

| 请求字段 | 结果 |
|-|-|
| `thinking:{"type":"adaptive"}` | ✅ 开思考，132 / 175 / 233（3 次） |
| `thinking:{"type":"disabled"}` | ✅ **真正关闭**，0 / 0 / 0（3 次） |
| `thinking:{"type":"enabled"}` | ⚠️ 静默忽略，仍思考（141） |
| `thinking:{"type":"on"}` | ⚠️ 静默忽略，仍思考（89） |
| `thinking:{"type":"off"}` | ⚠️ 静默忽略，仍思考，97 / 101 / 137（3 次） |
| `output_config:{"effort":"…"}`（`low`~`max` 5 档） | ❌ **完全无效**，79~250 全落在单档噪声范围内 |
| 只发 `output_config`、**不发** `thinking` | ❌ **完全不思考**（thinking_tokens 恒 0） |
| `thinking:{"type":"adaptive","budget_tokens":N}` | ❌ 无规律（1024→107、4096→217、16000→158、32000→95） |

→ **M3 只有开/关二值可控**，档位概念在 M3 上不存在。`on`/`off` 是客户端内部字面量，
**不能**直接当上游字段值用；`budget_tokens` 不被识别。

**M3.1-Flash（`forced_on`）**

| 请求字段 | 结果 |
|-|-|
| `output_config:{"effort":"low"}` | ✅ 46 / 65 / 69 |
| `output_config:{"effort":"medium"}` | ✅ 75 / 76 / 78 |
| `output_config:{"effort":"high"}` | ✅ 70 / 70 / 114 |
| `output_config:{"effort":"xhigh"}` | ✅ 82 / 89 / 129 |
| `output_config:{"effort":"max"}` | ✅ 125 / 218 / 266 |
| `output_config:{"effort":"default"}` | 与省略字段无差别（76/152/73 vs 93/136/97，噪声内相同） |
| 完全省略两个字段 | 正常思考（93 / 136 / 147） |
| `thinking:{"type":"disabled"}` | ❌ **HTTP 400** `invalid params, model …` |
| `thinking:{"type":"off"}` | ⚠️ 静默忽略，照常思考 |

→ **Flash 的 `effort` 是真实档位**，单调生效（相邻档有重叠：`high` 有时低于 `medium`，
但整体趋势 clear）。`default` 与不传等价，`none` 不允许（同 `disabled`）。

**结论差异表**

| 控制手段 | M3 | M3.1-Flash |
|-|-|-|
| `thinking.type` | ✅ 唯一有效手段（`adaptive`/`disabled`） | ❌ `disabled` 会 400，其余静默忽略 |
| `output_config.effort` | ❌ 完全无效 | ✅ 唯一有效手段 |
| 省略全部字段 | 不思考 | 正常思考（等于 `default`） |

### 5.3 `anthropic-beta: interleaved-thinking-2025-05-14`

`[抓包]` 仅 3/26 次出现，且全部是 `MiniMax-M3` + `thinking:{"type":"disabled"}`（即关思考的长上下文续轮）。
`MiniMax-M3.1-Flash` **从未**带该 beta 头。

`[源码 pi-mono anthropic.ts:901]`：`needsInterleavedBeta = interleavedThinking && !forceAdaptiveThinking`，
即**非 adaptive 模型**才会带。M3.1-Flash 属 adaptive → 不带；M3 在关思考时带。

实测它不影响 200 结果，我方实现可完全不发该头 `[未验证]`。

### 5.4 我方配置映射建议

| `[minimax] reasoning_effort` | M3 | M3.1-Flash |
|-|-|-|
| `low` | `thinking:{"type":"adaptive"}` | `output_config:{"effort":"low"}` |
| `medium` | 同上 | `{"effort":"medium"}` |
| `high` | 同上 | `{"effort":"high"}` |
| `xhigh` | 同上 | `{"effort":"xhigh"}` |
| `max` | 同上 | `{"effort":"max"}` |

两条硬约束（§5.2 实测）：

* **M3 只有开/关**，`low`~`max` 对 M3 **全部等价于开思考**，属上游能力限制；
  对 M3 **绝不能**发 `output_config`（无效）或 `on`/`off`（静默忽略）。
* **Flash 绝不能发 `thinking`**：`disabled` 会被 HTTP 400 拒，`off` 被静默忽略。
  档位只能走 `output_config.effort`。

**实现状态**：已接入请求体（`minimax/chat.rs` 的 `thinking_fields`），
落点由每轮拉取的目录 `models[<model>].thinking_config.mode` 分流：
`forced_on` → 只发 `output_config.effort`；其余（含目录拉取失败）→ 只发 `thinking.type`。

**配置档位 → 上游档位：就近映射**（`MinimaxReasoningEffort::nearest_effort`）：

配置文件**只能**填 `low`/`medium`/`high`/`xhigh`/`max` 五档（写其它值会报错）。
上游档位集合由目录 `effort_options` 决定，**不硬编码**，故需要「就近」：

1. 上游表里有同名档位 → 直接用（当前目录五档齐全，全部命中同名）；
2. 否则按「五档序号距离」挑最近的，**距离相同取较低档**（更省 token）；
3. 上游表里没有任何可识别档位名 → 回退到本档位的标准名；
4. `default` **不参与**距离计算——它表示「让上游自己决定」，与本档位不是同维度。

**不提供「关闭思考」档位**：上游对 Flash 无「关」档位（`disabled` 会被 400 拒），
M3 的 `disabled` 又会让模型完全不思考、退化明显，故配置层不暴露该选项。

M3（`switchable`）无档位，`low`~`max` 五档一律映射为 `thinking.type=adaptive`。

**实测验证**（2026-11-17，固定概率题，`max_tokens=3000`，每档 3 次，
读 `usage.completion_tokens_details.reasoning_tokens`）：

| 配置档位 | M3.1-Flash thinking_tokens | 说明 |
|-|-|-|
| `low` | 100 / 105 / 102 | 基线 |
| `medium` | 100 / 131 / 143 | 明显更高 |
| `high` | 215 / 98 / 148 | 相邻档重叠（§5.2 已知现象）|
| `xhigh` | 222 / 235 / 123 | |
| `max` | 146 / 345 / 272 | 最高 |

单调趋势成立（均值 102 → 125 → 154 → 193 → 254）。M3 在 `low` 与 `max` 下
thinking_tokens 均在 140~370 区间、无档位差异，符合「M3 只有开/关」的结论。

注意 `nearest_effort` 的判定只依赖目录 `effort_options` 的**字符串**，
若上游将来改名（如把 `xhigh` 改为 `x-high`），会退化到第 3 条规则（回退标准名），
此时发出去的档位名可能不在上游表里 —— 需要在目录变更时同步 `rank_of_upstream`。

## 6. 余额获取

抓包中**无**余额请求（客户端 UI 未打开过额度面板），但**源码里有端点**，`[源码 tui/src/account/matrix-account-client.ts]`：

```text
GET https://www.minimaxi.com/v1/api/openplatform/coding_plan/remains
accept: application/json
authorization: Bearer <access_token>
X-Group-Id: <opGroupId>        # 可选，模板订阅的计划组
```

响应关键字段（`base_resp.status_code != 0` 即失败）：

| 字段 | 含义 |
|-|-|
| `model_remains[0]` | 主模型额度窗口（只用第一个）|
| `current_interval_status` | `3` = 无限量 |
| `current_interval_remaining_percent` | 5 小时窗口剩余百分比（0–100）|
| `current_interval_total_count` / `current_interval_usage_count` | 百分比缺失时用 `usage/total` 算 |
| `end_time` | 5 小时窗口重置时间（ms）|
| `current_weekly_*` / `weekly_end_time` | 周窗口，同名规则 |
| `model_remains[]` 中名字含 `video` 且 `total_count>0` | 视频额度（单列）|

前置依赖：`opGroupId` 来自会员查询（非额度接口本身）：

```text
GET  https://agent.minimaxi.com/v1/api/user/info            # 取 realUserID
POST https://agent.minimaxi.com/matrix/api/v1/user/get_user_extra_info     body {}
POST https://agent.minimaxi.com/matrix/api/v1/commerce/get_membership_info body {} / {workspace_id}
```

会员响应里的 `has_token_plan` / `op_group_id` / `token_plan_tier` / `token_plan_expires_at` /
`op_credit_summary.total_remaining_amount`（或 `opcredit_balance`）= 余额与套餐。

两套域名不一致（额度在 `www.minimaxi.com`，会员在 `agent.minimaxi.com`）
`[源码矩阵常量]`；抓包未验证，且 `agent.minimaxi.com` 是源码里标为 legacy 的旧域 →
实际是否已迁到 `agent.minimax.cn` `[未验证]`。

`/v1/api/user/*` 与 `/matrix/api/v1/*` 需额外签名头 `[源码]`：

```text
yy:          GET : md5(encodeURIComponent(path+query) + "_{}" + md5(str(unix_ms)) + "ooui")
             POST: md5(encodeURIComponent(path+query) + "_" + <body> + md5(str(unix_ms)) + "ooui")
x-timestamp: <unix 秒>
x-signature: GET : md5(<unix 秒> + "I*7Cf%WZ#S&%1RlZJ&C2")
             POST: md5(<unix 秒> + "I*7Cf%WZ#S&%1RlZJ&C2" + <body>)
user-agent:  MiniMaxCode
```

源码注释明说这两个内联字串只是**客户端归属标记，不是凭据**，授权靠同一个 `Authorization`。

所有 `/v1/api/user/*` 与 `/matrix/api/v1/*` 还带一组 query 参数 `[源码 buildUrl]`（与§1.3 浏览器侧那组**不同**）：

```text
device_platform=mcode&biz_id=3&app_id=3001&version_code=22201&unix=<ms>
&timezone_offset=28800&lang=zh&sys_language=zh&device_id=0&os_name=linux
&browser_name=mcode&user_id=<realUserID|0>&client=mcode
```

额度接口（`coding_plan/remains`）**不带**这组 query、也不带签名头，只带 `Authorization` + 可选 `X-Group-Id`。

**本次任务按用户要求未分析余额**，上表仅为源码直读，字段语义 `[未验证]`。

## 7. 签到协议

**未获取端点**。抓包 61 条无签到请求；全包 `packages/` + `third_party/` 搜
`checkin`/`check_in`/`sign_in`/`daily_reward` 也无业务命中（仅无关的定时器/文案）。
`MiniMax Code` 客户端本身**没有签到功能**，无法从本客户端得到签到协议。

若需实现，要么另行提供网页版/其他客户端的签到抓包，要么放弃。**禁止推测路径。**

### 本实现的处理（`ofm-server`）

`Provider::claim` 返回 **`skipped = true`**，`message` 为
`无签到功能（官方客户端无签到接口，端点未知）`，**不返回错误**。

理由：自动签到循环（`cli/serve.rs`）每轮对每个供应商都调 `claim`，若返回 `Err`
就会往 `providers/minimax/claim.log` 写一条「失败」，而那会把「我们无从得知该账号
有没有签到」误报成「签到失败」。

`[minimax] claim_intervals` 配置项因此**暂时无效果**，保留只为配置形状一致。

## 8. 上游异常响应

### 8.1 OAuth

标准 OAuth 2.0 错误体（HTTP 非 2xx + 表单 POST）`[抓包]`：

```json
{"error": "authorization_pending", "error_description": "the user has not finished authorizing yet"}
```

错误码处理见 §1.2/§1.5。

`[实测]` 补充：本实现同时兼容另一种表示——**HTTP 200 + `{"status":"pending"}`**。
两套都按 §1.5 的语义分类（`Pending`/`SlowDown`/`Denied`/`Expired`），
所以两套并行存在也不影响。`slow_down` 按 OAuth 规范将轮询间隔 +5s。

### 8.2 对话 `/messages`

* HTTP 401 → 刷新令牌后重试一次 `[源码 auth-core.ts]`。
  `[实测]` 本实现进入 30s 刷新窗口时**预先续期**（`access_token()`），
  故正常路径不会撞 401；401 时映射 `ProviderError::Unauthorized` → 下游 401。
* 错误响应体形状 `[实测 2026-11-17]`（HTTP 400，非法模型名）：
  ```json
  {"type":"error","error":{"type":"invalid_request_error",
   "message":"invalid params, unknown model 'No-Such-Model' (2013)"},
   "request_id":"074b7dc06304bb6dbe8eea615d862ad3"}
  ```
  与 Anthropic 标准一致。本实现按 `error.type` 分类为
  `Authentication`/`InvalidRequest`/`RateLimit`/`Api`，并转成下游 OpenAI 错误体。
* 流内**无** `event: error` 样本（本次全部 200）；解析器仍保留该分支，
  按 `error.type` 分类后 `Event::Error` 转发。

### 8.3 模型目录

响应无业务码信封（直接 `{version, ttlSeconds, providers}`），
HTTP 非 200 即失败；`providers` 为空/缺 `models` 视为目录不可用 `[源码 list-models]`。

`[实测]` 本实现按此处理：非 2xx → `HttpError::Status`（带 code + 截断原文）；
解析失败 → `HttpError::Decode`；`providers` 空/`models` 缺 → 返回**空列表**
而不是报错（目录可用但暂无模型，`status` 会显示「0 个模型」）。

未测：令牌失效时的确切状态码与错误体（本次未触发）。预期为 401，
本实现进入刷新窗口时预先续期；对话侧的 `chat` 亦复用同一 `access_token()`，
故 401 只会出现在「凭证被服务端提前吊销」这一异常路径上（此时映射为下游 401）。

### 8.4 账号中心（登录/登出链）

两套错误形状并存 `[抓包]`：

| 端点 | 形状 |
|-|-|
| `/oauth2/*`（设备码、令牌、授权页） | `{"error":"<code>","error_description":"<msg>"}`，HTTP 非 200 |
| `/v1/api/user/*`、`/oauth2/login` | `{"data":…, "statusInfo":{code, httpCode, message, serviceTime, requestID}}`，**可能 HTTP 200 而 `code != 0`** |

已见 `statusInfo.code`：`0` 成功、`1200044` 未绑定手机号。
`/oauth2/login` 成功时又是另一形状：`{code:0, msg:"ok", data:{…}}`（无 `statusInfo`）。
→ 解析时两种形状都要兼容；`code`/`statusInfo.code` 任一非 0 即失败。

## 9. 未验证清单

### 已由本实现实测确认（2026-11-17）

* [x] 设备码登录全链：`/oauth2/device/code` → 轮询 `/oauth2/token` → 拿到令牌全成功
* [x] 请求体与 PKCE：`code_challenge_method=S256` + 43 字符 verifier/challenge 被接受
* [x] 令牌形态：**非 JWT**，`mmoat_` / `mmort_` 前缀不透明串，各 60 字符（详见 §1.4）
* [x] `expires_in = 43200`（12h），与抓包一致
* [x] 设备码超时行为：超过 `expires_in` 后不授权，轮询会收到
  `400 {"error":"expired_token"}`，到点本地方也自行中止
* [x] 模型目录：`GET /mavis/api/v1/models?region=cn&buildEnv=prod` → 200，
  字段与 §4 完全一致（`models` 两个键、`model_order` 顺序、`whitelist` 含 M2.7、
  `thinking_config.mode` 为 `switchable`/`forced_on`、`effort_options` 仅 M3.1-Flash 有）
* [x] 目录接口只带 `authorization: Bearer` 即可（不需 `x-api-key`）
* [x] **SSE 帧序列全貌**（见 §3.6）：有 `ping` 帧；`message_start` 的 usage 全 0；
  真实 usage 只在 `message_delta`；`signature_delta` 收尾 thinking 块
* [x] **`input_tokens` 不含缓存**：`input_tokens + cache_read_input_tokens`
  与 `/count_tokens` 完全吻合（78+128=206；75+128=203）
* [x] `output_tokens_details.thinking_tokens` 存在，可取到思考 token 数
* [x] `tool_use` 流式分片：起始帧带 `id`/`name` 且 `input` 为 `{}`；
  `input_json_delta` 首片为空串，随后逐片拼接
* [x] `tool_choice` 四种形态（`auto`/`any`/`tool`/`none`）全部 200
* [x] `max_tokens` 可省略（实测不传仍 200 正常出流）
* [x] `x-api-key` 非必需（只发 `authorization: Bearer` 亦 200）
* [x] M3.1-Flash 发 `thinking:{"type":"disabled"}` → **HTTP 400 被拒**
  （印证 `mode: forced_on`，只能走 `output_config.effort`）
* [x] `tool_result.content` 给字符串可用
* [x] 错误响应体形状（HTTP 400，见 §8.2）
* [x] 端到端：`/v1/chat/completions` 流式/非流式、工具调用、多轮工具回传、
  `reasoning_content`、usage 全部经 `ofm-server` 打通
* [x] **上游思考强度字段的真实接受范围**（§5.2）：M3 只认 `thinking.type` 的
  `adaptive`/`disabled`，`on`/`off`/`enabled` 与 `budget_tokens`、`output_config.effort`
  全部无效；Flash 只认 `output_config.effort`，`effort` 单调生效（low≈65 → max≈218
  thinking_tokens），`thinking.type=disabled` 会被 400 拒

### 待验

抓包已确认、无需再验的项（供对照）：设备码/轮询/双认证头/请求体字段集/thinking 两个落点/
`cache_control` 三处断点/`system` 形状/`tool_use`+`tool_result` 形状/UA 差异/目录响应全文/`count_tokens` 白名单。

待验：

* [ ] 刷新令牌实测（§2，仅源码定论；本次 access_token 有效期内未触发）
* [ ] 目录接口在令牌失效时的确切状态码与错误体（§8.3）
* [ ] `temperature`/`top_p`/`stop_sequences`/`metadata` 上游是否接受
  （官方从不发送 ≠ 不支持；`tool_choice` 已验证支持）
* [ ] 流内 `event: error` 的实际形状（本次全部 200，无非 2xx 样本）
* [ ] `cache_control` 是否必需、乱放/少放是否报错
* [ ] `tools[].eager_input_streaming` 是否必需
* [ ] `anthropic-dangerous-direct-browser-access` 是否必需（服务端调用）
* [ ] 各请求头的必需性（抓包无法删头验证）
* [ ] `MiniMax-M2.7`/`MiniMax-M2.7-highspeed` 能否被 `/messages` 调用（`whitelist` 有、`models` 无）
* [ ] 图片/视频输入实测（`modalities` 声明支持，抓包 26 次未出现 `image` 块）
* [ ] `context_window_options` 里 1000000 的切换方式（请求字段还是目录变体）
* [ ] 海外 `region=en` 的域名与参数
* [ ] 余额端点（§6）：仅有源码，无抓包；服务域名/`opGroupId` 来源链未实测
* **签到端点完全不知**（§7）：客户端无此功能，需另供抓包

## 10. 参考文件

| 文件 | 说明 |
|-|-|
| `packages/oauth-core/src/oauth-client.ts` | 设备码/令牌/刷新/撤销实现（160行附近：轮询分支）|
| `packages/oauth-core/src/contracts.ts` | `client_id`/`scope`/`audience` 常量 |
| `packages/oauth-core/src/endpoint-config.ts` | 域名与 `buildEnv`/`region` 矩阵 |
| `packages/config/src/config.ts:1570-1595` | `PRESET_BASE_URLS`（`cn-prod`/`en-prod`）与 `LEGACY_MANAGED_PRESET_BASE_URLS` |
| `packages/config/src/minimax-model-catalog.ts` | 内置模型目录（含 M2.7）|
| `packages/local-runtime/src/model-provider/thinking.ts` | 思考强度协议（M3 `on|off`、effort 透传）|
| `packages/local-runtime-v2/src/service/turn-system/agent-host/assembly/local-turn-payload-transform.ts` | `patchManagedThinkingPayload`：两个落点的实际写入逻辑；`patchVideoBlock` |
| `packages/local-runtime/src/context/messages-count-tokens-payload.ts` | `count_tokens` 字段白名单与 `thinkingDisplay` 默认值 |
| `packages/local-runtime-v2/src/service/model-system/connectivity/provider-request.ts` | `providerCompletionUrl`/`providerModelsUrls`/`buildProviderHeaders` |
| `third_party/pi-mono/packages/ai/src/providers/anthropic.ts` | SSE 解析、图片块转换、`forceAdaptiveThinking` 与 beta 头规则 |
| `packages/tui/src/account/matrix-account-client.ts` | 账号身份/会员/额度（余额）端点、`yy` 签名头、旧的 `renewal` 续期 |
| `test/capture_output/minimax_requests.jsonl` | 抓包原文（61 条）|
| `test/minimax_capture.py` | 抓包脚本（**注意**：只保存 JSON 响应体，SSE 流未保存）|

## 11. 实现顺序与要点

按 `MAINTAIN.md` 顺序：

| 步 | 内容 | 依赖 | 状态 |
|-|-|-|-|
| 1 | 登录（设备码 + 轮询）写 `credential.toml` | §1.1/§1.2 | ✅ 已实现（`minimax/auth.rs`）|
| 2 | 模型列表 `GET /v1/models?region=cn&buildEnv=prod` | §4 | ✅ 已实现（`minimax/models.rs`）|
| 3 | 对话转 Messages + SSE 回转 CC | §3.1–§3.6/§3.8 | ✅ 已实现（`minimax/chat.rs` + `minimax/stream.rs`），端到端实测通过 |
| 4 | 思考强度映射（按模型分两个落点）| §5 | ✅ 已实现：目录 `thinking_config.mode` 分流落点 + 配置五档就近映射上游档位（§5.4）|
| 5 | 令牌续期 `refresh_token` | §2 | ⚠︎ 已写（`auth::refresh` + 自动续期），**未实测** |
| 6 | 签到 | §7 | ❌ 端点未知，不可实现 |

余额（非 `MAINTAIN.md` 必选项）：源码有端点（§6）但无抓包，且依赖会员/工作区两条前置请求，
实现成本高于对话主链，建议后续再做。

实现代码：`src/providers/minimax/`（`api`/`auth`/`credential`/`models`/`chat`/`stream`/`mod`）。

要点：

* **域名写死预设，不读目录里的 `baseURL`**（§0 出入 1）
* 认证域（`account.minimax.cn`）/ API 域（`agent.minimax.cn`）**分离**，两者头也不同（§3.1 vs §4）
* 登录只能半自动：打印 `user_code` + `verification_uri_complete`，让用户去浏览器完成扫码/短信，
  我方只能轮询。**无法纯命令行完成**（§0 出入 4）
* **浏览器需由用户自己的图形会话打开**：`xdg-open` 在无 `DISPLAY`/`WAYLAND_DISPLAY`
  的会话（如 ssh/agent 环境）里仍会 `spawn` 成功但什么也不打开，所以本实现会先判环境；
  实际使用时由用户在自己的终端跑 `ofm-server login minimax`
* 下游 `stream:false` 必须自己聚合 SSE（§3.8）；上游恒 `stream:true`
* 自动刷新阈值：取令牌时剩余有效期 < 30s 就先刷新 `[源码]`；401 时刷新重试一次
* `reasoning_effort` 档位**从目录读**（`effort_options`），不硬编码；配置五档
  (`low`~`max`) 按就近规则映射，M3 只有开/关
* **思考落点按模型的 `thinking_config.mode` 分流**：`switchable` → 发 `thinking.type`
  （`adaptive`/`disabled`）；`forced_on` → 只发 `output_config.effort`，**绝不发 `thinking`**
  （实测 M3.1-Flash 发 `thinking.type=disabled` 会被 400 拒）。目录查不到时按 `switchable` 处理。
  每轮对话会拉一次目录来判定，失败降级不影响对话本身。
* **`prompt_tokens` 必须算 `input_tokens + cache_read_input_tokens`**（§3.6 关键陷阱）
* 思考字段**绝不能混发**：M3 会静默忽略 `output_config`，Flash 会用 400 拒绝 `thinking.type=disabled`
* M3 的 `on`/`off` 是官方客户端内部字面量，**不是**上游字段值；发到上游等于没发（§5.2）