# trae 客户端协议

来源：mitmproxy 抓取 Trae CN 1.107.1 真实流量 + 明文 curl 递增实验。
抓包脚本 `tmp/trae-capture/log_flows.py`；原始记录 `tmp/trae-capture/{flows.jsonl,requests.log,t5-t9.sse}`；
2026-11-15 错误与非流式实验记录 `tmp/trae-capture/{err1,e2-e5,f1,f2,g1,h5,h8,h9}.{json,sse,out}`。
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
* `functions` / `function_call`（旧形态）`[抓包]` — 上游只有 `tools`
* `prediction` / `store` / `service_tier` / `web_search_options` / `prompt_cache_options` / `moderation` `[未验证字段级]`

证据：`/api/ide/v1/batch_get_detail_param` 全量响应（`[抓包]` 2757110 字节，11 个 function）内不存在
`temperature`/`top_p`/`top_k`/`seed`/`stop`/`logprob`/`penalty`/`response_format` 任一字段名。
`[curl f1]` 证明额外未知字段不触发 400，即上游做宽松绑定 → 「接受但忽略」策略安全，原 §6.2 未验证项已解除。

响应侧，CC 无法表达、我方作为扩展字段透传：

* `delta.reasoning_content` `[抓包+curl]` — 思考增量；CC `delta` 只有 `content`/`refusal`/`tool_calls`/`role`/`audio`/`function_call`，此字段按业界通行命名透传

**`stream: false` 无法实现** `[curl g1]`：上游 `stream:false` 仍回 `Content-Type: text/event-stream` 和同样的 SSE 帧。
下游要非流式，必须由我方在内存中聚合 SSE 再拼 `chat.completion` 对象。

### 本协议与官方客户端的出入

以下偏离均在 2026-11-14/15 经用户批准（维护标准要求与官方客户端一致）：

1. **对话端点不同** `[抓包 vs curl]`。官方主回答来自 `create_agent_task`（体密文，明文完整 schema 未知，仅确认必填 `conversation_id`）；本实现用 `llm_utils_chat`。理由：明文 `llm_utils_chat` 已通过 `MAINTAIN.md` 两条验收（对话 + 工具调用），参考实现 `tmp/dsh-ofm-ng/TROUBLESHOOT.md` 亦直连实测该端点可用。
2. **请求体不加密** `[curl]`。官方发 base64 密文（entropy≈7.99）；本发明文 JSON。服务端不强制加密：认证先于 body 解析（无 JWT+明文 → `401/1001`；有 JWT+明文缺 app_id → `400/4001 expr_path=app_id`；有 JWT+含 app_id → `200` 正常回答）。→ 不需要逆向 `libharness.so`。
3. **不实现会话落库** `[抓包]`。官方每轮附带 `solo_hub/.../batchInsert` + `sync_history_state` + `workflow/start`（摘要/记忆）；本实现每请求自带完整 messages 历史，无跨请求会话态。
4. **`finish_reason` 需改写** `[curl t7]`。上游**工具轮次也只回 `stop`**（t7 有 tool_calls，done 帧仍 `{"finish_reason":"stop"}`）。为兼容 CC 客户端，本轮若产出 tool_calls 须改写为 `tool_calls`。**此条为主动偏离上游字面值，待你批准。**
5. **响应 `id`/`created` 由我方生成** `[抓包]`。上游响应无 conversation/response id 与时间戳，只有 `event:metadata` 的内部 `session_id`（uuid）。
6. **`app_id` 取参考实现常量** `6eefa01c-1036-4c7e-9ca5-d891f63bfcd8` `[curl]`，出处 `tmp/dsh-ofm-ng/.../trae-product.ts:322`；抓包请求体是密文，无法从流量验证此值。放 body `app_id` 或头 `X-App-Id` 均实测通过。
7. **上游独有、CC 无法表达因而放弃的能力** `[抓包]`：`create_agent_task` 自主 agent 循环（多轮工具自执行、记忆、plan_item）、服务端 PE 与 `extra_config`/`model_extra_config` 行为开关、`reasoning_effort_config`（目录有 `support_thinking`/`options`/`default_level`，我方 `reasoning_effort` 入参是否可映射到它 `[未验证]`）、`access.identity_list` 准入身份、`model_entitlement_usage_cost` 计费倍率。

### 域名

| 域名和端点 | 功能 |
|-|-|
| `api5-normal.mchost.guru/api/agent/v3/llm_utils_chat` | 对话（本实现主路径）`[抓包+curl]` |
| `api5-normal.mchost.guru/api/agent/v3/create_agent_task` | 官方主对话（密文）`[抓包]`，本实现不用 |
| `api5-normal.mchost.guru/api/agent/v3/{workflow/start,sync_history_state,initialize,append_msg,stop_chat,subscribe_events}` | agent 辅助 `[抓包+二进制]`，本实现不用 |
| `api5-normal.mchost.guru/api/ide/v1/batch_get_detail_param` | 模型列表 `[抓包+curl]` |
| `api5-normal.mchost.guru/api/ide/v1/get_client_config` | 端点配置 `[抓包]` |
| `api5-normal.mchost.guru/api/solo_hub/v1/conversations/messages/batchInsert` | 会话落库（明文）`[抓包]` |
| `api.trae.cn/trae/api/v3/oauth/ExchangeToken` | 登录换令牌 / 续期 `[抓包]` |
| `api.trae.cn/cloudide/api/v3/trae/oauth/GetPCAuthCode` | 浏览器侧取 AuthCode（需门户 Cookie）`[抓包]` |
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

2) POST https://api.trae.cn/cloudide/api/v3/trae/oauth/GetPCAuthCode   （需门户 Cookie）[抓包]
   {"ClientID":"ono9krqynydwx5","CodeChallenge":"…","CodeChallengeMethod":"S256",
    "DeviceID":"…","PlatformCode":"IDE_PC"}
   → Result.AuthCode（ExpireDuration 600000 = 10 分钟）

3) POST https://api.trae.cn/trae/api/v3/oauth/ExchangeToken   （无认证）[抓包]
   {"ClientID":"ono9krqynydwx5","AuthCode":"…","CodeVerifier":"…",
    "DeviceInfo":{"DeviceID","MachineID","PlatformCode":"IDE_PC","DeviceType":"PC",
      "DeviceName","DeviceModel":"","ClientVersion":"3.4.18",
      "DevicePublicKey":"<PEM EC prime256v1>","DeviceBrand":"","DeviceCPU":"…",
      "OSInfo":"linux","OSVersion":"Arch Linux"},
    "IDEVersion":"3.4.18"}
   → Result.{Token, RefreshToken, RefreshExpireAt, BoundDeviceID, ClientID, DeviceBindStatus}
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

* 用 `RefreshToken` 换 `Token` `[抓包 URL + 参考实现 trae-oauth.ts 分支 1]`
* **请求体 `[未验证]`**：本次抓包未触发续期。参考实现形态与 §1 步骤 3 的 `{ClientID, AuthCode, …}` 不同（疑为 `GrantType=refresh_token`）。实现前必须用真实 RefreshToken 实测一次，不能照抄参考实现。
* 已实测到的相关事实：JWT `source` 字段为 `refresh_token`，说明抓到的这个 Token 本身就是续期产物 `[抓包 jwt.txt]` → 续期链路真实存在且可用，只是请求体形态未知。
* `Token` 过期时的行为 = `401 {"code":1001,…}`（§5）`[curl e1/e3]`；策略：`exp` 前主动续期，`1001` 作为兜底触发续期后重试一次。

## 3. Chat Completions协议转换

以下每条都属 Trae CN 1.107.1 / appVersion 3.4.18，实测方式标注在行内。

### 3.1 请求头 `[抓包 §3.1 全集 + curl 递增实验 2026-11-15]`

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
| — | `function: "solo_work_lite"` | `[curl t5-t9]` 明文路径实测值；官方主对话用 `solo_agent` `[抓包]`；**工具往返实测用的是 `solo_agent`** → 定稿待定，见 §7 |
| `stream` | `stream: true` | `[curl g1]` 上游对 `stream:false` 仍回 SSE，故本实现恒发 `true`，非流式由我方聚合 |
| `messages[].role: user` | `{role:"user",content:[{type:"text",text:"…"}]}` | `[curl t8]` `content` 为字符串 → `cannot unmarshal string into …LLMRawMessage.messages.content of type []*idecopilot.LLMRawMessageContent` |
| `messages[].role: system` | `{role:"system",content:[{type:"text",text:"…"}]}` | **`[curl f2]` 实测被消费**：system「你必须只回复数字1」+ user「你好」→ 正文回 `1`，reasoning 明确复述「系统提示：“你必须只回复数字1”」。原未验证项解除 |
| `messages[].role: assistant` + `tool_calls` | `content:[]` + `tool_calls:[{id,type:"function",function_call:{name,arguments}}]` | `[curl t7]` 键名是 **`function_call`** 非 CC 的 `function`；与参考实现 `transformSOLOMessage` 一致 |
| `messages[].role: tool` | `{role:"tool",tool_call_id,name,content:[{type:"text",text:"…"}]}` | `[curl t7]` 实测上游接受回传并被模型消费（表格复述我给的虚构配置）|
| `tools[].function.parameters`（对象）| **字符串化 JSON** | `[curl t5]` 对象 → `cannot unmarshal object into …FunctionDefinition.tools.function.parameters of type string` |
| `tools[].function.{name,description}` | 同名 | `[curl t7]` |
| `max_tokens` 等忽略字段 | 不发 | `[curl f1]` 未知字段被宽松忽略，不发更稳妥 |

对话 body（实测通过，200 + 正常中文回答）：

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
| `done` | `{finish_reason:"stop"}` | `choices[0].finish_reason`；本轮有 tool_calls 则改写 `tool_calls`（§0 出入 4，待批准）；随后 `data: [DONE]` |
| **`error`** | `{code, message, extra:null}` | 见 §5，**必须先于内容帧判断** |

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

取 `function == "solo_work_lite"`（与本实现 §3.2 的 `function` 一致）且 `usage == "chat_completion"` 的条目：

| 上游 | CC `/v1/models` |
|-|-|
| `config_name` | `id = "trae/"+config_name`、`owned_by = "trae"` |
| `display_config.display_name` | 无标准位，可选放扩展 |
| `context_window_tokens.dev` | 无 CC 标准位（丢弃）|
| `is_invisible_to_user` | 过滤掉 `[未验证是否等价于不可用]` |
| — | `created` 上游无来源，取本地时间 |
| — | `object = "model"` |

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
| `4001` | `We're sorry, the param is invalid. Please try with a valid param.` | 假模型名 `nonexistent-model-x` `[e5]` |
| 其他 | `[未验证]` | 额度耗尽 / 内容风控 / `code` 全集未抓到 |

注意 `done` 帧仍是 `finish_reason:"stop"` —— **不能靠 `done` 判断成败，必须在解析时拦截 `event:error`**。

下游处理策略（**待你批准**，因为「已发 200 后再改状态码」不可行）：

1. **`event:error` 在第一个 `output` 帧之前**（实测两种都属此类，`error` 恒为 `id:1`）：我方尚未发出任何内容，故**不发 200**，直接映射为对应 HTTP 错误 —— `4011` → `429` + `error.type=rate_limit_error`（并尽量读 `Retry-After`，上游未提供）、`4001` → `400` + `invalid_request_error`。
2. **若将来出现 `error` 夹在内容帧之后**（未观测到，防御性处理）：此时 HTTP 200 已发出，只能以 `data: {"error":{…}}` 结尾 + `[DONE]`，CC 客户端按各家惯例处理。
3. 重试：`4011` 与 `1001` 各重试一次，间隔递增；不做无限重试，避免消耗积分。
4. 上游 `message` 是给人看的英文长句，不含内部细节；透传给下游时保留原文并附 `code`，便于用户区分「上游限流」与「我方 bug」。

## 6. 签到协议

Host `api.trae.cn`，走 VSCode UA 那套头（`[抓包]` 实测**不带** `x-machine-id`，与参考实现不同）：
`user-agent: VSCode 1.107.1 (Trae CN)`、`authorization: Cloud-IDE-JWT <jwt>`、`x-app-version: 3.4.18`、
`x-device-id`、`x-device-type: linux`、`x-os-version: Arch Linux`。

```
POST /trae/api/v2/ug/checkin_credits/status   {"req_source":1}      [抓包]
  → {"checked_in":true,"code":0,"credits":100,"did_checked_in":false,
     "enable":true,"extra_credits":100,"message":"success"}

POST /trae/api/v2/ug/checkin_credits/claim    {"req_source":1}      [未验证：抓包时当日已签到，未取到成功响应]

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

实现流程（README 要求每 1h）：`status.enable && !status.checked_in` → `claim` → 记录结果；
`checked_in` 字段语义为「今日已签到」，`did_checked_in` 疑为「本次请求触发成功」`[未验证]`。
实测对话消耗：`total_balance` 5579.62 → 5577.48（一次验收对话 -2.14）。

## 7. 待办 / 未验证

1. `RefreshToken` 续期请求体未实测（§2）—— 实现前必须补，不能照抄参考实现。
2. 未签到态 `claim` 成功响应与错误码（§6）。
3. `api5-normal` vs `trae-api-cn` 选择策略；`api5-normal` 下发来源未定位（抓包响应里搜不到 `api5`/`mchost`）。
4. `function` 用 `solo_work_lite` 还是 `solo_agent`（工具往返实测用了 `solo_agent` 且成功）→ 定稿后回写 §3.2/§4.3。
5. 除 `4011`/`4001` 外的流内 `error.code` 全集（额度耗尽、内容风控）。
6. `reasoning_effort` 能否映射到目录里的 `reasoning_effort_config`。
7. `multimodal_contents`/`phase` 非空条件；图片输入。
8. §0 出入 4（`finish_reason` 改写为 `tool_calls`）与 §5.2 下游错误映射策略 —— **需你批准**。
