# trae 客户端协议

来源：mitmproxy 抓取 Trae CN 1.107.1 真实流量 + 明文 curl 递增实验。
抓包脚本 `tmp/trae-capture/log_flows.py`（第一轮）、`log_flows2.py`（第二轮，含全量 header、
body 解码/熵分类、可选 `TRAE_FORCE_401` 短路）；原始记录 `tmp/trae-capture/{flows.jsonl,flows2.jsonl,requests.log,t5-t9.sse}`；
2026-11-15 错误与非流式实验记录 `tmp/trae-capture/{err1,e2-e5,f1,f2,g1,h5,h8,h9,re_*,hard_*,img_dataurl}.{json,sse,out}`。
标注方式：`[抓包]` = 官方客户端真实请求；`[curl]` = 我方直接构造请求实测；`[二进制]` = `libai_agent.so`/`libharness.so` 符号与字符串；`[未验证]`。

## 0. 总结

* 客户端(WebUI)版本: Trae CN **1.107.1**（appVersion `3.4.18`，buildVersion `2.3.102387`，`x-app-version-code`/`x-ide-version-code` = `20261110`）
* 分析方式: deb 解包 + 二进制符号分析 + mitmproxy 抓包 + 明文 curl 递增实验
* UA: 三套，按发起进程分，不可混用。对话必须 `[curl]` 用 `TraeClient/TTNet`（UA 参与限流分桶，见 §5）

| UA | 发起进程 `[抓包]` |
|-|-|
| `TraeClient/TTNet` | Rust agent（TTNet/cronet，`libsscronet.so`）→ `/api/agent/v3/*`、`/api/ide/v1/*` |
| `VSCode 1.107.1 (Trae CN)` | VSCode/Electron JS → `api.trae.cn/trae/api/v2/*`（签到、积分）|
| `Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) TraeCN/1.107.1 Chrome/142.0.7444.235 Electron/39.2.7 Safari/537.36` | Electron 桌面壳 → `ExchangeToken`、`icube/*` |

### 本协议不支持, 但Chat Completions有的功能

请求侧，我方须忽略（不转发给上游）：

* `temperature` / `top_p` / `top_k` / `min_p` `[curl f1]` — 带 `temperature:0.7` 上游不报错、正常回答，即**静默忽略已实测**
* `presence_penalty` / `frequency_penalty` / `logit_bias` / `logprobs` / `top_logprobs` `[未验证字段级]`
* `seed` / `stop` / `n` `[未验证字段级]`
* `response_format` / `verbosity` / `modalities` / `audio` `[未验证字段级]`
* `stream_options` `[curl g1]` — 上游无条件发 `event:token_usage`，无需开关
* `tools[].function.strict` `[抓包]` — 上游 tool schema 只有 `name`/`description`/`parameters`
* `functions` / `function_call`（**请求顶层**旧形态）`[抓包]` — 上游顶层只有 `tools`。注意别和 `messages[].tool_calls[].function_call` 混淆，后者是上游的**必填键名**（§3.2）
* `prediction` / `store` / `service_tier` / `web_search_options` / `prompt_cache_options` / `moderation` `[未验证字段级]`

证据：`/api/ide/v1/batch_get_detail_param` 全量响应（`[抓包]` 2757110 字节，11 个 function）内不存在
`temperature`/`top_p`/`top_k`/`seed`/`stop`/`logprob`/`penalty`/`response_format` 任一字段名。
`[curl f1]` 证明额外未知字段不触发 400，即上游做宽松绑定 → 「接受但忽略」策略安全。
标注 `[未验证字段级]` 的行未逐个字段单独实测，其结论来自上面两条证据（目录无字段名 + 未知字段被忽略）。

响应侧，CC 无法表达、我方作为扩展字段透传：

* `delta.reasoning_content` `[抓包+curl]` — 思考增量；CC `delta` 只有 `content`/`refusal`/`tool_calls`/`role`/`audio`/`function_call`，此字段按业界通行命名透传

**`stream: false` 无法实现** `[curl g1]`：上游 `stream:false` 仍回 `Content-Type: text/event-stream` 和同样的 SSE 帧。
下游要非流式，必须由我方在内存中聚合 SSE 再拼 `chat.completion` 对象。

### 本协议与官方客户端的出入

以下偏离逐条经用户批准（2026-11-14/15；维护标准要求与官方客户端一致）。分两类：A 类是我方主动改动上游字面值，B 类是官方行为不可观测（请求体密文）只能取推断值。

1. **对话端点不同** `[curl vs 抓包]` — A 类。官方主回答来自 `create_agent_task`（体密文，明文完整 schema 未知，仅确认必填 `conversation_id`）；本实现用 `llm_utils_chat`。理由：明文 `llm_utils_chat` 已通过 `MAINTAIN.md` 两条验收（对话 + 工具调用），参考实现 `tmp/dsh-ofm-ng/TROUBLESHOOT.md` 亦直连实测该端点可用。
2. **请求体不加密** `[curl]` — A 类。官方发 base64 密文（entropy≈7.99）；本发明文 JSON。服务端不强制加密：认证先于 body 解析（无 JWT+明文 → `401/1001`；有 JWT+明文缺 app_id → `400/4001 expr_path=app_id`；有 JWT+含 app_id → `200` 正常回答）。→ 不需要逆向 `libharness.so`；与 §3.6 的客户端明文直通分支互证。
3. **不实现会话落库** `[抓包]` — A 类。官方每轮附带 `solo_hub/.../batchInsert` + `sync_history_state` + `workflow/start`（摘要/记忆）；本实现每请求自带完整 messages 历史，无跨请求会话态。
4. **`finish_reason` 改写** `[curl t7]` — A 类，**已批准（2026-11-15）**。上游**工具轮次也只回 `stop`**（t7 有 tool_calls，done 帧仍 `{"finish_reason":"stop"}`）。本实现在本轮产出 tool_calls 时改写为 `tool_calls`，以符合 CC 规范。回退方式：删掉改写，原样透传 `stop`（代价：依赖 `finish_reason=="tool_calls"` 判断退出条件的客户端会把工具轮当最终回答，agent 循环失效）。
5. **响应 `id`/`created` 由我方生成** `[抓包]` — A 类。上游响应无 conversation/response id 与时间戳，只有 `event:metadata` 的内部 `session_id`（uuid）。
6. **`app_id` 取参考实现常量** `6eefa01c-1036-4c7e-9ca5-d891f63bfcd8` `[curl]` — B 类，出处 `tmp/dsh-ofm-ng/.../trae-product.ts:322`；抓包请求体是密文，无法从流量验证此值。放 body `app_id` 或头 `X-App-Id` 均实测通过。
7. **`function` 取 `solo_agent`** `[curl + 抓包]` — B 类，**已批准（2026-11-15）**。官方对话请求里的 `function` 值看不到（体密文），且该字段是服务端**模型准入闸门**，不是可选装饰。取值依据见 §3.2；两个候选的目录覆盖与逐条代价见 §4.3。
8. **图片不走官方上传链路** `[curl vs 抓包]` — A 类，**已批准（2026-11-15）**。官方四步：附件落本地 → `get_resource_upload_url` → 传 TOS → `commit_resource_upload_result` → `Read` 工具交独立多模态模型转描述（§3.7）。我方：CC 的 `image_url`（含 `data:` URL）内联直发，一步完成。理由：内联实测有效；上传链路要复刻 TOS 分片协议 + 凭证信封，成本高，且走完拿到的只是**图片的文字描述**，能力并不等价。
9. **流内错误映射为 HTTP 状态码 + 裁剪 message** `[curl e4/e5/3003]` — A 类，**已批准（2026-11-15）**。详见 §5.2。
10. **上游独有、CC 无法表达因而放弃的能力** `[抓包]` — B 类后果。`create_agent_task` 自主 agent 循环（多轮工具自执行、记忆、plan_item）、服务端 PE 与 `extra_config`/`model_extra_config` 行为开关、`reasoning_effort_config`（目录有 `support_thinking`/`options`/`default_level`；我方入参能否映射过去已实测为**不能**，见 §3.2）、`access.identity_list` 准入身份、`model_entitlement_usage_cost` 计费倍率。

### 域名

| 域名和端点 | 功能 |
|-|-|
| `api5-normal.mchost.guru/api/agent/v3/llm_utils_chat` | 对话（本实现主路径）`[抓包+curl]` |
| `api5-normal.mchost.guru/api/agent/v3/create_agent_task` | 官方主对话（密文）`[抓包]`，本实现不用 |
| `api5-normal.mchost.guru/api/agent/v3/{workflow/start,sync_history_state,query_history_state,initialize,append_msg,stop_chat,subscribe_events}` | agent 辅助 `[抓包+二进制]`，本实现不用 |
| `api5-normal.mchost.guru/api/ide/v1/{get_resource_upload_url,commit_resource_upload_result}` | 图片上传两步（官方链路，本实现不用）`[抓包 flows2]`，见 §3.7 |
| `tos-d-lq.bytedancevod.com` | 图片实际上传主机 `[抓包 flows2 仅由 session_key 得知名，流量未捕获]` |
| `api5-normal.mchost.guru/api/v1/commercial/get_session_usage`、`solo_hub/v1/clis/requests/respond` | 会话用量 / CLI 请求响应 `[抓包 flows2]`，本实现不用 |
| `api5-normal.mchost.guru/api/ide/v1/batch_get_detail_param` | 模型列表 `[抓包+curl]` |
| `api5-normal.mchost.guru/api/ide/v1/get_client_config` | 端点配置 `[抓包]` |
| `api5-normal.mchost.guru/api/solo_hub/v1/conversations/messages/batchInsert` | 会话落库（明文）`[抓包]` |
| `api.trae.cn/trae/api/v3/oauth/ExchangeToken` | 登录换令牌 / 续期 `[抓包]` |
| `api.trae.cn/cloudide/api/v3/trae/oauth/GetPCAuthCode` | 浏览器侧取 AuthCode（需门户 Cookie，CLI 不可用）`[抓包+实测]` |
| `api.trae.cn/cloudide/api/v3/{trae/CheckLogin,trae/GetUserInfo,common/GetUserToken}` | 登录态 / 用户信息 `[抓包]` |
| `api.trae.cn/trae/api/v2/ug/*`、`trae/api/v2/pay/*` | 签到 / 积分 `[抓包]` |
| `www.trae.cn/authorization` | 登录页（浏览器打开）`[抓包]` |
| `api.trae.com.cn/icube/api/v1/{user,native/config/query}` | 设备登记、远端配置 `[抓包]` |
| `trae-api-cn.mchost.guru` | `product.json` 声明的 agent host，抓包中 0 次实际请求；`api5-normal` 下发来源未定位 `[未验证]` |
| `trae-ws-cn.mchost.guru/custom_model` | `product.json` 声明的 WS，本实现不用 |

## 1. 登录方式

自动 PKCE（`S256`），`client_id` 固定 `ono9krqynydwx5` `[抓包]`。三步，全部 1.107.1/3.4.18 实测。

```
1) 浏览器打开（我方只需拼 URL 并监听回调）[抓包]
   https://www.trae.cn/authorization?login_version=1&auth_from=trae&login_channel=native_ide
     &plugin_version=2.3.102387&auth_type=local&client_id=ono9krqynydwx5&redirect=0
     &login_trace_id=<uuid>&auth_callback_url=http://127.0.0.1:40909/authorize
     &machine_id=<64hex>&device_id=<num>&x_device_id=<num>&x_device_brand=&x_device_type=linux
     &x_os_version=Arch%20Linux&x_env=&x_app_version=3.4.18&x_app_type=stable
     &code_challenge=<b64url43>&code_challenge_method=S256&channel_name=common
   用户在页面完成抖音/手机号登录。

2) 授权完成后浏览器跳回 auth_callback_url，查询串已含授权码 [实测，2026-11-15]
   ?isRedirect=true&scope=trae
    &authCodeInfo={"AuthCode":"<44 字符>","ExpireAt":<ms>,"ExpireDuration":600000}
    &loginTraceID=<uuid>&host=https://api.trae.com.cn&userRegion=cn&userInfo={…}
   → 直接取 authCodeInfo.AuthCode，**无需**再调 GetPCAuthCode。
   GetPCAuthCode 是网页端（www.trae.cn 页面内）取授权码用的，需浏览器门户 Cookie，
   无 Cookie 调用返回 401 `{"Code":"20101","Message":"Token 无效"}`。

3) POST <回调返回的 host>/trae/api/v3/oauth/ExchangeToken   （无认证）[抓包 + 实测]
   {"ClientID":"ono9krqynydwx5","AuthCode":"…","CodeVerifier":"…",
    "DeviceInfo":{"DeviceID","MachineID","PlatformCode":"IDE_PC","DeviceType":"PC",
      "DeviceName","DeviceModel":"","ClientVersion":"3.4.18",
      "DevicePublicKey":"<PEM EC prime256v1>","DeviceBrand":"","DeviceCPU":"…",
      "OSInfo":"linux","OSVersion":"Arch Linux"},
    "IDEVersion":"3.4.18"}
   → Result.{Token, RefreshToken, RefreshExpireAt, BoundDeviceID, ClientID, DeviceBindStatus}
```

续期（`RefreshToken` 换新 `Token`）`[实测 2026-11-15]`：

```
POST <host>/trae/api/v3/oauth/ExchangeToken
{"ClientID":"ono9krqynydwx5","ClientSecret":"","RefreshToken":"…",
 "DeviceInfo":{…同上但 DevicePublicKey 为同一密钥的公钥…},
 "DeviceProof":{"Signature":"<b64 DER>","Timestamp":<秒>,"Nonce":"<32 hex>"},
 "IDEVersion":"3.4.18"}

DeviceProof 签名消息 = ["POST","/trae/api/v3/oauth/ExchangeToken",ClientID,RefreshToken,
                        Timestamp,Nonce].join("\n")，ECDSA-SHA256(设备私钥) → DER → 标准 base64。
注意签名 payload 是 **RefreshToken 本身**（不是 JSON body）；见 out/main.js 的 pFe。
```

要点 `[抓包]`：

* 回调端口 `40909`（官方硬编码；参考实现用 18080）
* `S256(CodeVerifier)` = `CodeChallenge`，标准 PKCE
* 服务端绑定设备：`DeviceBindStatus: BOUND`；`machine_id`(64hex)/`device_id`(数字)/`DevicePublicKey` 同源于本机一对 EC P-256 密钥，**必须一起持久化，不能每次重新生成**
* `Token` 是 RS256 JWT，载荷 `{"data":{…}}`；实测样本 `iat` 2026-11-14T23:09:52+08 → `exp` 2026-11-28T23:09:52+08 = **14.0 天**
* `RefreshExpireAt - iat` ≈ 180 天

存储 `providers/trae/credential.toml`：

```toml
token = "<Cloud-IDE-JWT>"
refresh_token = "<…>"
refresh_expire_at = "<RFC3339>"
machine_id = "<64hex>"
device_id = "<num>"
device_private_key_pem = "<EC prime256v1>"   # 须与已绑定公钥配对，ExchangeToken 续期用
```

## 2. 令牌续期方式

```
POST https://api.trae.cn/trae/api/v3/oauth/ExchangeToken   （无认证）
```

* 用 `RefreshToken` 换 `Token` `[curl 2026-11-15 假令牌探测]`。**请求体已实测确认最小必填集**（用无效令牌探测，未触碰真令牌）：

  ```json
  {"ClientID":"ono9krqynydwx5","RefreshToken":"<refresh_token>"}
  ```

  * 只给 `ClientID` / 只给 `RefreshToken` / 或走 `{ClientID,AuthCode,CodeVerifier}` 但无 AuthCode 值 → **`400` `Code:"10101"`** `无效参数：{__Message.field}.`
  * 给齐两个字段但令牌无效 → **`401` `Code:"20101"`** `Token 无效：refresh token is invalid.`（`Data.__Message.error = "refresh token is invalid"`）
  * → 说明这两个字段就是必填集，其余（参考实现带的 `ClientSecret:"-"`、`UserID:""`）**可省**：带上它们仍走同一条 20101 分支。参考实现 `tmp/dsh-our-free-model/.../trae-auth.ts:refreshCredential` 的形态可用但多余。
  * `GrantType` 字段**不需要**（`{ClientID,GrantType,RefreshToken}` 报 10101）。
  * 控制面响应是**另一套信封**（与 `/api/agent/v3/*` 的 `{"code":…}` 不同）：
    `{"ResponseMetadata":{…,"Error":{"Code","Message","StandardCode","Data"},"TraceID",…},"Result":{…},"success","logId"}`
    → 续期与登录的错误解析不能用 §5 那套 `code` 数字，需单独分支。
* **未知项只剩一个**：RefreshToken 是否**轮换**（参考实现注释称旧 refreshToken 即刻失效 `[参考实现，未验证]`）。成功响应的字段结构不用等验证 —— 见本节末尾。
* 已实测到的相关事实：抓到的 JWT 载荷 `source=refresh_token`、`source_id` = RefreshToken 原文，证明该 Token 本身就是续期产物 `[抓包 jwt.txt + requests.old.log:1142]` → 续期链路真实可用。
* 令牌留存（`tmp/trae-capture/requests.old.log:1142`，旧版日志 header 截断但该 body 完整）：`RefreshToken`、`BoundDeviceID=28j7pl18l6wkyk`、`TokenExpireDuration=1209600000`(14天)、`RefreshExpireAt=1810220992928`。
* `Token` 过期时的行为 = `401 {"code":1001,…}`（§5）`[curl err1/e3]`；**我方策略**：`exp` 前主动续期，`1001` 兜底触发续期后重试一次。
  这是**我方设计，不是官方行为**。

**官方客户端不会因 401 而续期** `[抓包 flows2 2026-11-15，用 mitmdump request 钩子伪造 401]`：

* 手段：`TRAE_FORCE_401=1` 启动，把第一条 `llm_utils_chat` 短路成
  `401 {"code":1001,…}`（不发上游、不消耗额度）。伪造成功（抓包记录到该 401）。
* 结果：随后 **0 次** `ExchangeToken`、**0 次** `GetUserToken`、**0 次** 登录链请求；
  401 后 6 秒客户端直接改发 `create_agent_task` 并成功（200），
  全程 246 个带令牌的请求**用的是同一个 JWT**（`iat 1794668992` / `exp 1795878592`）。
* 判读：Token 远未过期（`exp` 2026-11-28），客户端按本地 `exp` 判断是否续期，
  收到 401 只当业务失败、不触发认证重取。→ **重启客户端同样无效**（重启不改 `exp`）。
* 且这次 401 被伪造在 `llm_utils_chat`（标题/图标生成，非主对话）上：该请求 401 后
  **客户端一次都没重试**（全程 `llm_utils_chat` 仅 2 次），主对话 `create_agent_task` 照常 200。
  即客户端把这个端点的认证失败当可忽略错误，连业务重试都没有 —— 更不可能触发续期。
* 误判更正：`POST /trae/api/v3/GetThirdPartyToken` 看似续期，实为
  `{"Types":["feishu","lark"]}` 飞书集成周期请求，与认证无关（每 ~5s 一次，401 前后都有）。
* **重启也无效，且已被抓包证实**：本次抓包窗口 10:26–10:58 **本身就含一次完整客户端启动**
  （进程启动时间 10:37:09，该分钟 149 条请求，含 `CheckLogin`）—— 全程 `ExchangeToken` **0 次**。
* 续期触发条件定位：`CheckLogin.Result.ExpiredAt = 1810220992928` = **2027-05-13**，
  与 ExchangeToken 响应里的 `RefreshExpireAt`（180 天）**同一数值**，而 JWT `exp` 是 2026-11-28。
  → 客户端把「登录有效期」按 refreshToken 到期日记账，JWT 只在临近自身 `exp` 时才惰性续期。
  现在距 `exp` 还有 13 天，所以无论重跑几次、重启与否都不会出现续期。
* 客户端 Electron 侧存储（`~/.config/Trae CN/User/globalStorage/state.vscdb`，57 个 key）
  **无任何认证相关 key、无 refresh 字样** `[自查后已删除副本]` → RefreshToken 不在明面库里，
  应在 Rust agent 侧加密存储；`~/.trae-cn` 下也无。

→ **§2 的缺口实际已缩小**：`ExchangeToken` 是同一个 URL，其 AuthCode 交换的成功响应结构
`[抓包 requests.old.log:1142]` 已知 = `Result.{Token, RefreshToken, RefreshExpireAt, BoundDeviceID,
   ClientID, DeviceBindStatus}`（外层 `ResponseMetadata`）。续期调用**几乎必然回同一结构**，
   字段名可直接实现 `[推断，未验证]`。唯一真正未知的是 **RefreshToken 是否轮换**（若轮换，
   我方必须持久化新值，旧值作废）。→ 不阻塞实现；定案选项见 §7.1。

## 3. Chat Completions协议转换

以下每条都属 Trae CN 1.107.1 / appVersion 3.4.18，实测方式标注在行内。

### 3.1 请求头 `[抓包 + curl 递增实验 2026-11-15]`

`POST https://api5-normal.mchost.guru/api/agent/v3/llm_utils_chat`

准入条件（curl 逐个删头实测，缺一即失败）：

| 头 | 必需性 | 缺失后果 `[curl]` |
|-|-|-|
| `user-agent: TraeClient/TTNet` | **强制** | curl 默认 UA → HTTP 200 流内 `event:error` `code:4011` rate limit；同请求换 TTNet 立即放行 → **限流按 UA 分桶，UA 是准入标签不是标识** |
| `authorization: Cloud-IDE-JWT <jwt>` | 强制 | `401 {"code":1001}` |
| `x-app-version` + `x-app-version-code` + `x-ide-version` + `x-ide-version-code` | 四者齐全 | 缺一律 `400 {"code":4001,"message":"bad request: binding: expr_path=app_version_code, cause=missing required parameter"}` |
| `content-type: application/json` | 强制 | — |
| `x-machine-id` / `x-device-id` | 实测不强制 | 但为维护标准一致仍整套发送 `[抓包]` |
| `accept: */*`、`x-device-cpu`、`x-device-type`、`x-os-version`、`x-flow-traceparent` | 照抄官方 | 未单独验证必需性 `[未验证]` |

官方**不发** `X-Cloudide-Token` / `X-Ide-Token` / `X-Uid` / `X-App-Id`；参考实现 `traeSOLOHeaders()` 那几个是过期格式 `[抓包]`。
版本常量（`3.4.18` / `20261110` / `TraeClient/TTNet`）写死，客户端升级时属维护项。

### 3.2 请求映射（Chat Completions → 上游）`[curl，2026-11-14/15，Trae CN 1.107.1]`

| Chat Completions | 上游 body | 实测依据 |
|-|-|-|
| `model: "trae/glm-5.2"` | `model` + `config_name`（同值，剥 `trae/` 前缀）| `[curl t5-t9]`；假模型名 → 流内 `event:error code:4001` `[curl e5]`；大小写归一按目录 `config_name`（小写）`[抓包]` |
| — | `app_id: "6eefa01c-…"` | `[curl]` 必填，缺失 → `400 expr_path=app_id`；也可放头 `X-App-Id` |
| — | `function` | **定稿 `solo_agent`**（§0 出入 7，已批准）`[curl + 抓包 2026-11-15]`。**`function` 是模型准入闸门**（决定性实测）：`qwen3.8-flash` + `solo_work_lite` → 流内 `4001`；同模型 + `solo_agent` → `200` 正常回答（`provider_model_name=qwen3.8-flash`）。官方对话请求里的 `function` 值**看不到**（体密文）；间接证据 `[推断]`：官方所用 `qwen3.8-flash` 只存在于 `solo_agent` 目录，且明文 `batchInsert` 的 `agent_type=solo_agent` `[抓包]`。两个候选的目录覆盖与逐条代价见 §4.3 → 本实现取 `solo_agent` |
| `stream` | `stream: true` | `[curl g1]` 上游对 `stream:false` 仍回 SSE，故本实现恒发 `true`，非流式由我方聚合 |
| `messages[].role: user` | `{role:"user",content:[{type:"text",text:"…"}]}` | `[curl t8]` `content` 为字符串 → `cannot unmarshal string into …LLMRawMessage.messages.content of type []*idecopilot.LLMRawMessageContent` |
| `messages[].content[] = {type:"image_url", image_url:{url}}` | **唯一可用形态**；`url` 支持外链，也支持 `data:` URL | **`[curl]` 图片字段名已由 Go 报错确认**：`image_url` 传字符串 → `cannot unmarshal string into Go struct field LLMRawMessageContent.messages.content.image_url of type idecopilot.LLMRawMessageImageUrl`；传数字 → 同结构名。故 `LLMRawMessageContent` 有 `image_url`（`LLMRawMessageImageUrl` 类型）。`{type:"image", image_url:"<str>"}` → `400`；`{type:"image",url:"…"}` → 流内 `4001`。只有 `type:"image_url"` + 对象形态通过。<br>**`data:image/png;base64,…` 实测被真实解析**：1×1 纯红 PNG（像素 `FF0000`，已 zlib 解扫描线核实）→ 无 error，正常回答「浅粉色/珊瑚粉」（色相族对、明度不准 = 模型感知精度，非协议问题）。→ **我方不必自建图床**，CC 的 base64 图片可直接透传。<br>**外链不可达时**回 `3003 model service is unavailable`（HTTP 200 流内 error，见 §5.2）|
| `reasoning_effort`（CC）| **本实现无法支持，须声明为忽略** | **官方字段名与取值已由 `[抓包 flows2 2026-11-15]` 坐实**：明文 `batchInsert` 的 `model_info.reasoning_effort_level`，随 UI 切换依次出现 `high`（默认）→ `light` → `extra_high`，与目录 `options:[light,high,extra_high]`、`default_level:high`（`qwen3.8-flash`）完全对应。**但该字段只出现在明文落库请求里，对话请求体是密文**（同批 `create_agent_task` 解码后 entropy≈7.996、无魔术头），无法看到官方在对话请求里怎么传。<br>我方 `llm_utils_chat` 明文路径实测**不受该字段控制**：顶层放 `reasoning_effort_level:"bogus_level"` 与 `"light"` 行为一致（`[curl]`，函数 solo_agent/qwen3.8-flash），且非法枚举值不报错 —— 与「未知字段被宽松忽略」（§0）一致，无法区分「忽略」和「透传但不校验」。A/B 也无生效证据且方向相反（solo_work_lite/minimax-m3.1-flash：`light` reasoning_tokens 2784 > `extra_high` 1420，属噪声）。<br>→ **结论：思考强度要真正生效必须走 `create_agent_task`，其明文 schema 未知。当前端点下此参数按「接受并忽略、回日志提示」处理。**<br>另注：目录 `reasoning_effort_config.support_thinking` **不门控实际行为** —— `glm-5.2`+`solo_work_lite` 标 `false`，实测仍产出 224 字 reasoning `[curl]`。|
| 思考能力覆盖面 | — | 目录 `support_thinking` 计数 `[抓包]`：`solo_work_lite` 48 条中仅 1 条；`solo_agent` 74 条中 27 条 → 与 §3.2 的 `function` 选择绑定 |
| `messages[].role: system` | `{role:"system",content:[{type:"text",text:"…"}]}` | **`[curl f2]` 实测被消费**：system「你必须只回复数字1」+ user「你好」→ 正文回 `1`，reasoning 明确复述「系统提示：“你必须只回复数字1”」。原未验证项解除 |
| `messages[].role: assistant` + `tool_calls` | `content:[]` + `tool_calls:[{id,type:"function",function_call:{name,arguments}}]` | `[curl t7]` 键名是 **`function_call`** 非 CC 的 `function`；与参考实现 `transformSOLOMessage` 一致 |
| `messages[].role: tool` | `{role:"tool",tool_call_id,name,content:[{type:"text",text:"…"}]}` | `[curl t7]` 实测上游接受回传并被模型消费（表格复述我给的虚构配置）|
| `tools[].function.parameters`（对象）| **字符串化 JSON** | `[curl t5]` 对象 → `cannot unmarshal object into …FunctionDefinition.tools.function.parameters of type string` |
| `tools[].function.{name,description}` | 同名 | `[curl t7]` |
| `max_tokens` 等忽略字段 | 不发 | `[curl f1]` 未知字段被宽松忽略，不发更稳妥 |

对话 body（实测通过，200 + 正常中文回答；`function` 此处用当时实测的 `solo_work_lite`，**实现须发 `solo_agent`**）：

```json
{"app_id":"6eefa01c-1036-4c7e-9ca5-d891f63bfcd8",
 "model":"glm-5.2","config_name":"glm-5.2","function":"solo_work_lite",
 "stream":true,
 "messages":[{"role":"user","content":[{"type":"text","text":"你好"}]}]}
```

工具往返 body（实测通过，`[curl t7/t8]`）：

```json
{"app_id":"…","model":"deepseek-v4.1-flash","config_name":"deepseek-v4.1-flash",
 "function":"solo_agent","stream":true,
 "tools":[{"type":"function","function":{"name":"get_system_info",
   "description":"获取本机操作系统与CPU/内存配置",
   "parameters":"{\"type\":\"object\",\"properties\":{},\"required\":[]}"}}],
 "messages":[
   {"role":"user","content":[{"type":"text","text":"使用工具获取信息, 简述本机系统配置"}]},
   {"role":"assistant","content":[],
    "tool_calls":[{"id":"call_00_gu39…","type":"function",
      "function_call":{"name":"get_system_info","arguments":"{}"}}]},
   {"role":"tool","tool_call_id":"call_00_gu39…","name":"get_system_info",
    "content":[{"type":"text","text":"OS=Arch Linux; CPU=…"}]}]}
```

### 3.3 响应映射（上游 SSE → CC chunk）`[curl t7/t9，2026-11-15]`

上游是标准 SSE 分帧 `id:N\nevent:<name>\ndata:<json>\n\n`，按空行切分即可，**不需要第三方 eventsource 库**。

实测帧集合（工具那次 61 帧 = 57 `output` + 各 1；纯问答那次 121 `output`）：

| 上游 event | data `[curl]` | → Chat Completions |
|-|-|-|
| `metadata` | `{model:""<恒空>, session_id:<uuid>, prompt_completion_id, ab_versions, app_version, ide_version, ide_version_type, prompt_cache_pool_record}` | 首 chunk：`id="chatcmpl-"+本地`、`created` 本地时间、`model` 回填请求模型名、`object=chat.completion.chunk`、`delta.role="assistant"` |
| `timing_cost` | `{name:"llm_raw_chat_v2", provider_model_name, 各段耗时}` | 丢弃。`provider_model_name` 是内部服务名（见过 `ep-20261020194436-p4fjc`、`glm-5.2`），不外泄 |
| `output` | `{response, reasoning_content, tool_calls, multimodal_contents, phase}` | `delta.content ← response`；`delta.reasoning_content ← reasoning_content`（扩展字段）；`delta.tool_calls ← tool_calls`（按 `index` 累积，见下）|
| `extra_info` | `{model, version:1, content:<字符串化 reasoning_content>, input_token, expire_time}` | 丢弃 |
| `token_usage` | `{prompt_tokens, completion_tokens, total_tokens, reasoning_tokens, cache_read_input_tokens, cache_creation_input_tokens, cluster, *_total}` | `usage.prompt_tokens/completion_tokens/total_tokens`；`prompt_tokens_details.cached_tokens ← cache_read_input_tokens`；`completion_tokens_details.reasoning_tokens ← reasoning_tokens`；`cache_creation_input_tokens` 无 CC 标准位，丢弃 |
| `done` | `{finish_reason:"stop"}` | `choices[0].finish_reason`；本轮有 tool_calls 则改写 `tool_calls`（§0 出入 4，已批准）；随后 `data: [DONE]` |
| **`error`** | `{code, message, extra:null}` | 见 §5，**必须先于内容帧判断** |
| `progress_notice` | **裸字符串** `"Processing_1794708540"`、`";Processing_1794708541631326453_rRktuc"` `[curl 图片/难题请求]` | 丢弃。`data` 不是 JSON（`json.loads` 会失败），解析时必须容忍非 JSON 帧。出现在图片下载、长思考等慢请求中，实测在 `error` **之前** |

`output` 帧规则（实测）：

* `response` 与 `reasoning_content` **同帧互斥填充**（一帧只出一个），都是**增量**，与 web 版 `plan_item` 的累积快照不同。首帧样例 `{"response":"本机","reasoning_content":null,"tool_calls":null,"multimodal_contents":null,"phase":null}`。
* `tool_calls` 按 `index` 累积：首帧带 `index/id/type/function_call.name`，`arguments` 为空串；后续帧 `id:"" type:"" name:""`，只在同一 `index` 上补 `arguments` 片段。`partial_arguments`/`namespace` 实测恒 `null`。

```json
{"tool_calls":[{"index":0,"id":"call_00_gu39…","type":"function","function_call":{"name":"get_system_info","arguments":"","partial_arguments":null,"namespace":null}}]}
{"tool_calls":[{"index":0,"id":"","type":"","function_call":{"name":"","arguments":"{}","partial_arguments":null,"namespace":null}}]}
```

* `multimodal_contents`、`phase` 在多次实测中恒 `null`，图片输入不支持 `[未验证其非空条件]`。

### 3.4 非流式 `[curl g1]`

上游对 `stream:false` 仍返回 `Content-Type: text/event-stream` + 相同 SSE 帧。
下游 `stream:false` 时：我方聚合 `output` 的 `response` 为 `message.content`、`reasoning_content` 为扩展字段、累积 `tool_calls`、`token_usage` 填 `usage`，拼成单个 `chat.completion` 对象返回。聚合失败（中途 `event:error`）见 §5。

### 3.5 官方主对话端点备查（本实现不用）`[抓包，Trae CN 1.107.1]`

若上游改为强制加密、或 `llm_utils_chat` 明文路径失效，须切到这里；届时需重新逆向 §0 出入 2 的加密。

`POST /api/agent/v3/create_agent_task`（请求体密文，响应 SSE 明文），事件序列（一次 `hi` 共 24 帧）：

`agent_identity` → `task_created` → `model_config` → `agent_status` → `chat_memory_trigger`
→ `context_usage` → `history`×2 → `metadata` → `timing_cost` → **`thought`** → `token_usage`
→ `required_context` → `turn_completion{task_completion:true}`，配合 `agent_status.status=completed` 收尾。

`thought` 帧：`{agent_type, agent_id, agent_name, agent_run_id, parent_agent_run_id, first_data, reasoning_content, thought}`
—— 与 `llm_utils_chat` 的 `output` 同构（`reasoning_content`/`thought` 互斥、均为增量），只是字段名不同。

ID 体系（从明文 `batchInsert` 可见，各 24hex 递增）：`chat_session_id` / `message_id` / `turn_id` /
`task_id`(`task_`前缀) / `plan_item`(`pi_`前缀)；`trace_id` = `x-flow-traceparent` 第二段，服务端用它串联。

落库消息含 `model_info`：`reasoning_effort_level:"high"`、`config_name`、`multimodal:true`、
`use_remote_service:true`、`prompt_max_tokens`（用户侧 936000，assistant 回显 168000）。
明文 `create_agent_task` 请求仅确认必填 `conversation_id`，完整 schema `[未验证]`。


### 3.6 二进制字符串佐证（`[二进制 libharness.so]`，2026-11-15）

不依赖 mitm 的补充证据，来自 `strings` 提取的 serde 字段名表（Rust serde 会把结构体字段名编进二进制）：

* **加密是可选协议而非强制**：同一份枚举里并列 `plaintext_passthrough`、`absent`、
  `unprotected`、`no_encrypted_fields`、`decrypt_failed_passthrough`、
  `unsupported encryption version`、`encrypted history missing version`、
  `cloud SSE encryption processed`。→ 与 §0 出入 2 的实测互证：客户端本身就有明文直通分支。
* **采样参数存在于客户端，但属「自定义模型」配置**，不在预设模型请求链上：
  结构 `ModelHyperParamsConfig{thinking_enable, reasoning_param, temperature, top_p, top_k}`，
  邻居是 `CustomModelHyperParams` / `CustomModelConfigDetail` / `custom_model_uniq_id` /
  `skauth_type` / `region`（用户自填 OpenAI 兼容模型那套）。
  同一段还出现 `OpenAIRequest{model, max_tokens, stream, temperature, tools, tool_choice}`
  + `OpenAIMessage{content, tool_calls, tool_call_id}` → 客户端对自定义模型走**标准 OpenAI 协议**，
  对自家预设模型走 trae 私有协议。
  → §0「上游不支持采样参数」仅对**预设模型**成立；若日后接自定义模型（`is_custom_model`）
  则是另一套明文 OpenAI 请求，可支持 temperature。
* **客户端自带完整 ErrorCode 枚举**（数百个，含 `ModelPremiumQuotaDrained`、
  `TodayUserRequestLimit`、`FreeActivityQuotaExhausted`、`RequestWaitExceedQueueSize`、
  `RiskRequestImageContentSafety`、`ContextLengthExceeded`、`NotSupportMultiMedia`、
  `ModelNotExisted`、`EnterpriseSeatQuotaExhausted`、`OutOfQuota`、`AbnormalAccountLimitRequest`
  等）。这是**客户端侧**枚举，不等同于 §5 上游 `{"code":4011}` 的数字码；
  但可用于判断「额度耗尽/风控」这类场景官方是怎么命名的。`[未验证]` 数字 code 映射表。


### 3.7 图片：官方链路 ≠ 我方可用链路 `[抓包 flows2 2026-11-15]`

**官方桌面客户端不内联图片**，实测四步：

```
1) 图片落本地:  ~/.trae-cn/attachments/<uuid>_<uuid>_test.png
2) POST /api/ide/v1/get_resource_upload_url   {"targets":["<ms>_<hash>_png_<W>x<H>"],"biz_type":"image"}
     → {session_key:<base64 JSON>, store_infos:[{auth,upload_id,batch_id,…}], upload_hosts:["tos-d-lq.bytedancevod.com"]}
     session_key 解出 {accountType:ImageX, fileType:image, storeInfos:[{StoreUri,Auth,UploadID,BatchID,…}],
                       uploadHost, uri, userId}
     Auth = "GJZWJT7CHNZUT4T18CN3:version:v2:<HMAC-JWT>"，内含 bucket/expires/fileInfos[].oidKey（凭证不落文档）
3) 实际 PUT/分片上传到 tos-d-lq.bytedancevod.com（**不在本抓包范围**：`--allow-hosts` 只放了 trae 域名，
     该主机名来自 `product.json` 声明的 AhaNet 通道；见 §7）
4) POST /api/ide/v1/commit_resource_upload_result  {session_key:…} → `{}`（空对象即成功）
```

然后 agent 侧**不是把图片喂给对话模型**，而是调工具：

```json
{"id":"…","name":"Read",
 "params":{"file_path":"/home/main/.trae-cn/attachments/…_test.png",
           "target":"Describe all visible content in the image"},
 "result":{"status":"success","images":[{"url":"trae-res://image/tos-cn-i-zaeny81f73/cn/<uid>/image/…_1024x560",
            "detail":"auto","size":{"width":1024,"height":560}}]}}
```

配置开关（明文 `model_extra_config`）：`enable_read_tool_image_to_text:true`、
`read_tool_image_to_text_config_name:"multimodal_image_kimi-k2.5"` → **官方用一个独立的多模态模型
（Kimi-K2.5）把图转成描述文字**，再由主模型（qwen3.8-flash，`multimodal:true`）继续。

**我方实现走不通也不必走这条路**：`content[]{type:"image_url",image_url:{url}}` 在
`llm_utils_chat` 明文路径实测直接生效（§3.2，1×1 红 PNG 被识别为粉色调），无需上传、无需工具、
无需 TOS 凭证。差异属 §0 出入 8。

## 4. 模型列表获取

### 4.1 上游请求 `[抓包 + curl，Trae CN 1.107.1]`

```
POST https://api5-normal.mchost.guru/api/ide/v1/batch_get_detail_param   （明文）
{"functions":["personal_assistant","assistant","solo_agent","solo_agent_lite","solo_coder",
  "solo_agent_remote","solo_work_lite","solo_work_remote","solo_design_lite",
  "solo_design_remote","builder"],
 "agent_type":"","current_config_info":{"config_name":"","is_custom_model":false},
 "mode_type":0,"access_type":1,"ab_force_vids":"","ab_autotest_advanced_mode":0}
```

响应 2757110 字节（未截断，实测 4 次同长度一致）。

### 4.2 解析规则 `[抓包]`

顶层 `{allow_tenant_user_add_model, client_config, client_config_ab_versions, function_configs[11]}`；
`function_configs[i] = {function, config_info_list[], ab_versions[]}`；`solo_work_lite` 下 48 个模型。

单条目字段：`config_name` `config_source` `config_switch` `context_window_tokens{dev,max}`
`display_config` `display_contact_config`（**字符串化 JSON**：`access.identity_list`、`consumption_rate.data.rate`、`reasoning.enable`）
`extra_config`（**字符串化 JSON**）`is_default` `is_invisible_to_user` `model_detail_list[]`
`reasoning_effort_config{support_thinking,options,default_level}` `usage`。

`model_detail_list[]` 项：`icon{dark,light}` `max_tokens` `max_turn` `model_entitlement_usage_cost`
`model_name`（带 `__dev`）`prompt_max_tokens` `tool_call_history_max_tokens` `all_history_max_tokens`
`model_extra_config`（**字符串化 JSON**，数十个行为开关）`custom_model_type` `custom_models`。

名字三层 `[抓包]`：目录 `config_name` 小写（`deepseek-v4.1-flash`）/ 显示名混合大小写（`DeepSeek-V4.1-Flash`）/ 内部 `model_name` 带 `__dev` 后缀。请求侧用 `config_name`。

### 4.3 映射到 `/v1/models`

取 **`function == "solo_agent"`**（与本实现 §3.2 `function` 定稿一致 `[curl 实测门控]`）
且 `usage == "chat_completion"` 的条目 → **56 个**。

两个候选的完整代价 `[抓包，由 flows.jsonl 的 2757110 字节全量目录实测统计]`：

| | `solo_agent` | `solo_work_lite` |
|-|-|-|
| 目录条目 | 74 | 48 |
| 其中 `usage=chat_completion`（可对话） | **56** | 33 |
| 独占的 `chat_completion`（选另一个就取不到；交集 25） | **31** | 8 |
| `support_thinking=true`（全条目口径） | 27 | 1 |

* 选 `solo_agent`（本实现）取不到的 8 个 `chat_completion` 模型：`seed-code-pro-0430`、`Doubao-Seed-2.0-Code`、`glm-5-turbo`、`kimi-k2.6`、`sagitta`、`aquila`、`file_search_agent`、`explore_sub_agent_v2`（后两个是子 agent，非用户可选模型）。
* 选 `solo_work_lite` 取不到的 31 个，含官方主对话在用的 `qwen3.8-flash`、`deepseek-v4.1-flash-{300K,600K,1M}`、`kimi-k3-auto`、`glm-5.3_advisor` → 且这些会直接撞 §5.2 的 `4001`。
* → **定稿 `solo_agent`**（§0 出入 7）。净收益 23 个可对话模型（31−8）、`support_thinking` 覆盖 1 → 27。

**不得按 `is_invisible_to_user` 过滤 `[抓包 + curl，2026-11-15]`**：`solo_agent` 的 56 个 `chat_completion` 条目里 **38 个 `is_invisible_to_user=true`**，其中 **`glm-5.2` 是我方实测通过、正常出中文回答的模型**（`[curl f1/f2/g1]`，`provider_model_name=glm-5.2`）。同组还包含 `deepseek-v4.1-flash-{300K,600K,1M}`、`kimi-k3-auto`、`glm-5.1/5/4.7/4.6`、`qwen-3.7-plus` 等。
→ 该标志只表示「客户端 UI 不展示」，**与 API 可用性无关**；若按其过滤，`/v1/models` 只剩 18 个并会砍掉已验证可用的条目。原「`[未验证是否等价于不可用]`」项就此关闭。

| 上游 | CC `/v1/models` |
|-|-|
| `config_name` | `id = "trae/"+config_name`、`owned_by = "trae"` |
| `display_config.display_name` | 无标准位，可选放扩展 |
| `context_window_tokens.dev` | 无 CC 标准位（丢弃）|
| `is_invisible_to_user` | **不过滤**，见上方实测（`glm-5.2` 标 true 但实测可用）|
| — | `created` 上游无来源，取本地时间 |
| — | `object = "model"` |

**同一 `config_name` 下有两套 `model_detail_list`（决定「更大上下文」）`[抓包 + 抓包 flows2]`：**

| `model_name` | `prompt_max_tokens` | `max_tokens` |
|-|-|-|
| `qwen3.8-flash__dev` | 168000 | 32000 |
| `qwen3.8-flash__max` | **936000** | **64000** |

官方「更大上下文」开关实测落点：`mode_type: 0 → 2`，且 `model_info.model_name` 由
`qwen3.8-flash__dev` 变为 `qwen3.8-flash__max` `[抓包 flows2 batchInsert 明文]`。
→ 后缀语义：`__dev` = 默认上下文，`__max` = 大上下文（`mode_type=2`）。
**但 `mode_type` 在明文对话请求里实测不改变行为**（顶层放 `mode_type:2` 与对照组一致，
同属「未知字段被宽松忽略」）→ 大上下文是否等价于直接请求 `model_name` 的 `__max` 变体，
`[未验证]`，且我方 `llm_utils_chat` 只有 `model`/`config_name` 两字段可用。
建议：`/v1/models` 暴露 `config_name`，`prompt_max_tokens`/`max_tokens` 上限按 `__max` 记，
并在文档标注「大上下文模式当前无法通过明文路径选择」。

## 5. 上游异常响应

**两类，实现必须都处理。第二类是 HTTP 200，靠 HTTP 状态码判断会漏。**

### 5.1 HTTP 层错误 `[curl，Trae CN 1.107.1，2026-11-15]`

| HTTP | 上游 body | 触发条件 `[实测]` | → 下游（CC 规范错误）|
|-|-|-|-|
| `401` | `{"code":1001,"message":"We're sorry, but we are not able to authenticate you. …"}` | 无 `authorization` `[err1]`；或伪造 JWT `[e3]` | `401` + `error.type=authentication_error`，同时**触发本地续期**（§2）|
| `400` | `{"code":4001,"message":"bad request: binding: expr_path=<字段>, cause=missing required parameter"}` | 缺版本头 → `expr_path=app_version_code` `[e2]`；缺 app_id → `expr_path=app_id` `[curl 2026-11-14]` | `400` + `error.type=invalid_request_error`；`expr_path` 写进 `error.message`。**若我方请求本应齐全却报此错 = 协议漂移，属维护项** |
| `400` | `{"code":4001,"message":"bad request: json: cannot unmarshal <类型> into Go struct field <结构>.<路径> of type <类型>"}` | `tools[].function.parameters` 传对象 `[t5]`；`messages[].content` 传字符串 `[t8]` | 同上。这两条是 trae 特有的强类型陷阱，映射代码写错时最先撞到这里 |
| `400` | `{"code":4001,"message":"bad request: invalid character 'o' in literal null (expecting 'u')"}` | body 不是合法 JSON `[h9]`。上游用 Go 解码器原生报错，格式各异 | 同上 |
| `403` | `{"code":991402,"message":"git token not found"}` | `[抓包]` 仅 `/api/remote/v1/git/repositories`，本实现不用该端点 | 不透传 |
| `400` | `{"code":400,"message":"[LocalConvSync.FetchCliConversation] hub error: code=2900101, message=cli device is offline"}` | `[抓包]` `batchInsert`，本实现不落库 | 不透传 |

### 5.2 SSE 流内错误（HTTP 200）`[curl e4/e5，2026-11-15]`

上游在业务/限流错误时**照样返回 HTTP 200 + SSE**，帧形如：

```
id:1
event:error
data:{"code":4011,"message":"We're sorry, your requests have exceeded the rate limit. Please wait and try again later.","extra":null}

id:2
event:done
data:{"finish_reason":"stop"}
```

| `code` | message | 触发 `[实测]` |
|-|-|---|
| `4011` | `We're sorry, your requests have exceeded the rate limit. Please wait and try again later.` | 用 curl 默认 UA 发正常请求 `[e4]` → **限流按 UA 分桶**，TTNet UA 同请求立即放行 |
| `4001` | `We're sorry, the param is invalid. Please try with a valid param.` | 假模型名 `nonexistent-model-x` `[e5]`；`{type:"image",url:"…"}` 形态 `[curl]`；**模型不在所给 `function` 的目录内**：`qwen3.8-flash`+`solo_work_lite` → `4001`，同模型+`solo_agent` → `200` `[curl 2026-11-15]` → 实现须在映射前按目录校验 model×function |
| `3003` | `model service is unavailable\nerror, status code: 400, message: Error while downloading: <url>, error: Get "<url>": dial tcp …: connect: connection refused Request id: 02179…` | **图片外链不可达** `[curl]`。上游把内部错误（目标 URL、Go 网络错误、上游 Request id）整段塞进 `message` → **透传给下游前必须裁剪** |
| 其他 | `[未验证]` | 额度耗尽 / 内容风控 / `code` 全集未抓到 |

注意 `done` 帧仍是 `finish_reason:"stop"` —— **不能靠 `done` 判断成败，必须在解析时拦截 `event:error`**。

下游处理策略（**已批准，2026-11-15**；即 §0 出入 9，因为「已发 200 后再改状态码」不可行）：

判定依据用**「是否已产出内容帧」**，不用 `error` 的 `id`：实测 `4011`/`4001` 的 `error` 是 `id:1`，但 `3003`（图片下载失败）前面已有 2 帧 `progress_notice`，`error` 在 `id:3` —— 所以「`error` 恒为 `id:1`」不成立。

1. **`error` 之前没有任何 `output` 帧**（实测三类全属此类；`progress_notice` 不算内容）：我方尚未发出内容，故**不发 200**，直接映射 HTTP 错误 —— `4011` → `429` + `rate_limit_error`（上游无 `Retry-After`）、`4001` → `400` + `invalid_request_error`、`3003` → `400` + `invalid_request_error`（message 裁剪后）。未观测到的 `code` 一律 `502` + `api_error`，`message` 取首行，原始 `code` 附在 `error.code`（§7.2 会陆续补进此表）。
2. **`error` 出现在 `output` 之后**（尚未观测到，防御性）：HTTP 200 已发出，只能以 `data: {"error":{…}}` 结尾 + `[DONE]`。
3. 重试：`4011` 与 `1001` 各重试一次，间隔递增；不做无限重试，避免消耗积分。
4. `4011`/`4001` 的 `message` 是给人看的英文整句、无内部细节，保留原文并附 `code`；`3003` 例外，须只取首行 `model service is unavailable`，丢弃后面的 Go 错误与 Request id。

## 6. 签到协议

Host `api.trae.cn`，走 VSCode UA 那套头（`[抓包]` 实测**不带** `x-machine-id`，与参考实现不同）：
`user-agent: VSCode 1.107.1 (Trae CN)`、`authorization: Cloud-IDE-JWT <jwt>`、`x-app-version: 3.4.18`、
`x-device-id`、`x-device-type: linux`、`x-os-version: Arch Linux`。

```
POST /trae/api/v2/ug/checkin_credits/status   {"req_source":1}      [抓包 + curl]
  → {"checked_in":true,"code":0,"credits":100,"did_checked_in":false,
     "enable":true,"extra_credits":100,"message":"success"}

POST /trae/api/v2/ug/checkin_credits/claim    {"req_source":1}      [curl 2026-11-15 实测]
  → HTTP 200 {"code":0,"message":"success"}                          （当日已签到时，无 credits 字段）

POST /trae/api/v2/ug/activity/info            {"version_code":"3.4.18","req_source":1}   [抓包]
  → commercial_activities[]: checkin_credits(type 104, work_credits 200),
    new_user_credits(102, 2000), send_message_reward(105, 200), student_auth_reward(107),
    code_fission_202609(103), express_lottery(2), work_desktop_download…
  每项含 Enabled / activity_id / activity_type / start_time_ms / end_time_ms / work_extra

POST /trae/api/v2/pay/ide_user_ent_usage      {"require_usage":true,"req_source":1}   [抓包]
  → {total_balance, balance_summary[{source,total_balance}], is_credits_billing,
     usage_summary{consumed_amount,total_amount,consumption_ratio}, user_entitlement_pack_list[], trial_status}

POST /trae/api/v2/pay/ide_user_pay_status     {}                    [抓包]
```

`req_source`：桌面客户端 `1`，web `3` `[抓包]`。

**claim 语义陷阱（实现必须处理）`[curl]`**：当日**已签到**时再调 `claim`，仍返回
`HTTP 200 {"code":0,"message":"success"}` —— 与「领取成功」响应**无法区分**。
所以不能靠 `claim` 的返回值判断是否真领到积分，只能：
`status` 判 `enable && !checked_in` → `claim` → 再读 `status`/`ide_user_ent_usage` 校验余额变化。
`claim` 无破坏性副作用：已用只读接口核对，重复调用未扣款、未额外发积分。

字段语义（实测，注意与直觉相反）`[抓包 + curl]`：

* `checked_in: true` = 今日已签到
* `did_checked_in: false` **在 `checked_in:true` 时为 false** → 疑为「本次请求触发签到成功」，不是「今日是否签过」。`[未验证：未取到 `true` 样本]`
* `credits:100 / extra_credits:100` = 本次签到基础/额外额度；`enable:true` = 活动开启

余额读数（只测端点，未逐次记录消耗，故无法拆分）`[curl]`：
11-14 抓包末 `total_balance` 5577.92（`consumed_amount` 1722.08 / `total_amount` 7300）
→ 11-15 10:15 为 5675.78（`consumed_amount` 1624.22），净 **变化 +97.86**。
量级与「今日签到 +100」一致，且 11-15 上午另有若干次成功对话消耗。
`shortcut: 未做逐次余额采样，无法证明 +97.86 全部来自签到；若实现要精确对账，需在 claim 前后各读一次 status+usage`。

实现流程（README 要求每 1h）：`status.enable && !status.checked_in` → `claim` → 复读 `status` 确认 `checked_in` 变 `true` 并记余额增量；`checked_in` 已为 `true` 则本轮直接跳过，**不要调 claim**（避免把「已签到」误记为「本次领到」）。

## 7. 剩余未验证

协议分析阶段结束：§0 出入 10 条已全部经用户批准（2026-11-14/15），实现尚未开始（`TODO.md` #1）。
下面只列真正未定的事项，正文各节已定稿的结论不再在此重复。

### 7.1 RefreshToken 是否轮换（唯一需用户决定）

§2 的必填请求体与响应结构都已确定，剩下的问题是**旧 refreshToken 在续期后是否作废**（决定我方要不要把新值写回 `credential.toml`）。实现上先按「无条件写回」处理，两种情况都正确；要定案二选一：

* a) 用真实 RefreshToken 直接 `curl` 一次 `ExchangeToken` —— 立刻可定，但**旧 refreshToken 可能被轮换作废**（参考实现称即刻失效，未验证），届时客户端与 `tmp/trae-capture/jwt.txt` 的凭证一起失效，用户须重新登录一次。
* b) 等 JWT `exp`（2026-11-28）临近时抓客户端自然续期 —— 无副作用，但要等约两周。§2 已证明只有这条被动路径能看到续期。

### 7.2 留待观察（不阻塞实现）

* `api5-normal.mchost.guru` vs `trae-api-cn.mchost.guru` 的选择策略；`api5-normal` 的下发来源未定位（§0 域名表）。
* 流内 `error.code` 全集（额度耗尽、内容风控未观测到）。客户端侧 ErrorCode 枚举见 §3.6，数字映射未知。未知 code 的兜底策略已写在 §5.2。
* `did_checked_in: true` 的样本（§6），下次签到时顺手可取。
* `tos-d-lq.bytedancevod.com` 的实际 PUT 分片协议未捕获（`--allow-hosts` 不含该主机）。本实现不走该链路（§0 出入 8），仅在日后要复刻官方图片行为时才需要。
* 大上下文模式能否经明文路径选择：`mode_type` 实测不生效，`__dev`/`__max` 变体是否可直接指定 `[未验证]`（§4.3）。
