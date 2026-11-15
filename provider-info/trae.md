# trae 客户端协议

按 `provider-info/template.md` 组织。除标注「未验证」外，全部为 2026-11-14/15 实测。
抓包脚本 `tmp/trae-capture/log_flows.py`，原始记录 `tmp/trae-capture/{requests.log,flows.jsonl,t5-t9.sse}`。

## 0. 总结

* 客户端(WebUI)版本: Trae CN 1.107.1（appVersion `3.4.18`，buildVersion `2.3.102387`，x-ide-version-code `20261110`）
* 分析方式: deb 解包 + 二进制符号/字符串分析 + mitmproxy 抓包 + 明文 curl 递增实验
* UA: 三套，按发起进程分，不能混用（对话必须是 `TraeClient/TTNet`，见 §3.1）

| 发起进程 | UA |
|-|-|
| Rust agent（TTNet/cronet，`libsscronet.so`）| `TraeClient/TTNet` |
| VSCode/Electron 侧 JS | `VSCode 1.107.1 (Trae CN)` |
| Electron 桌面壳 | `Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) TraeCN/1.107.1 Chrome/142.0.7444.235 Electron/39.2.7 Safari/537.36` |

### 对比Chat Completions不支持的功能

上游接受、我方需忽略或拒绝的 Chat Completions 请求字段：

* `temperature` / `top_p` / `top_k` / `min_p`
* `presence_penalty` / `frequency_penalty` / `logit_bias` / `logprobs` / `top_logprobs`
* `seed` / `stop` / `n`
* `response_format` / `verbosity` / `modalities` / `audio`
* `tools[].function.strict`（上游 schema 只有 `name`/`description`/`parameters`）
* `tool_choice` / `parallel_tool_calls`
* `functions` / `function_call`（上游只有 `tools` 形态）
* `max_tokens` / `max_completion_tokens`（上游请求体未见此字段；模型目录另有 `max_tokens`，属服务端配额而非调用方可调）
* `stream_options`（上游无条件返回 usage，见 §3.3）
* `prediction` / `store` / `service_tier` / `web_search_options` / `prompt_cache_options` / `moderation`

证据：`/api/ide/v1/batch_get_detail_param` 全量响应（2757110 字节，覆盖 11 个 function）中
不存在 `temperature`、`top_p`、`top_k`、`seed`、`stop`、`logprob`、`response_format`、
`penalty` 任一字段名。`stream` 是唯一实测过的开关字段（只验过 `true`）。

注意：「目录里没有该字段」不等于「请求体带该字段会被拒绝」。未实测发送未知字段的行为，
实现按「接受但忽略」处理，忽略项记入响应日志。见 §6。

### 本协议不支持、Chat Completions 无法表达的上游功能

* `create_agent_task` 的自主 agent 循环（多轮工具自执行、记忆、上下文注入、plan_item）
* 会话落库（`solo_hub/v1/conversations/messages/batchInsert`）与历史同步（`sync_history_state`）
* 服务端 system prompt（每个 function/model 自带 PE，客户端不传 system，见 §6 未验证项）
* 计费身份白名单 `access.identity_list`、消耗倍率 `model_entitlement_usage_cost`
* `reasoning_effort_config`（目录含 `support_thinking`/`options`/`default_level`）
* 模型内部名带 `__dev` 后缀（`deepseek-v4.1-flash__dev`），Chat Completions 无对应概念

### 本协议与官方客户端的出入

以下偏离已在 2026-11-14 经用户批准（维护标准要求协议与官方客户端一致）：

1. **对话端点不同。** 官方桌面客户端主回答来自 `create_agent_task`（请求体密文）；
   本实现用 `llm_utils_chat`（请求体明文，可注入 tools）。理由：`create_agent_task`
   明文完整 schema 未知（只确认必填 `conversation_id`），且明文路径已实测通过
   `MAINTAIN.md` 两条验收；参考实现 `tmp/dsh-ofm-ng/TROUBLESHOOT.md` 也直连实测过该端点。
2. **请求体不加密。** 官方客户端发 base64 密文（entropy≈7.99）；本发明文 JSON。
   服务端不强制：无 JWT+明文→`401 code 1001`；有 JWT+明文缺 app_id→`400 expr_path=app_id`；
   有 JWT+明文含 app_id→`200` 正常回答。即认证先于 body 解析，加密不是准入条件。
3. **不实现会话落库。** 官方每轮都 `batchInsert` + `sync_history_state`；
   本实现在 `llm_utils_chat` 上每请求自带完整 messages 历史，不落库、无跨请求会话态。
4. **`finish_reason` 需改写。** 上游工具轮次也只回 `stop`（实测 t7 含 tool_calls，
   done 帧仍是 `{"finish_reason":"stop"}`）。为兼容 CC 客户端，若本轮产出 tool_calls，
   我方须改写为 `tool_calls`。这是主动偏离上游值，需批准。
5. **响应 `id`/`created` 由我方生成。** 上游响应无 conversation/response id 与时间戳字段
   （只有 `event:metadata` 的 `session_id`，且是上游内部 uuid）。
6. **`app_id` 固定值** `6eefa01c-1036-4c7e-9ca5-d891f63bfcd8`，取自参考实现
   `trae-product.ts:322`，非抓包所得（抓包请求体为密文，看不到该字段）。

### 域名

| 域名和端点 | 功能 |
|-|-|
| `api5-normal.mchost.guru/api/agent/v3/llm_utils_chat` | 对话（本实现主路径，明文可用）|
| `api5-normal.mchost.guru/api/agent/v3/create_agent_task` | 官方主对话（密文，本实现不用）|
| `api5-normal.mchost.guru/api/agent/v3/{workflow/start,sync_history_state,initialize,append_msg,stop_chat,subscribe_events}` | agent 辅助，本实现不用 |
| `api5-normal.mchost.guru/api/ide/v1/batch_get_detail_param` | 模型列表（明文，2.7MB）|
| `api5-normal.mchost.guru/api/solo_hub/v1/conversations/messages/batchInsert` | 会话消息落库（明文）|
| `api.trae.cn/trae/api/v3/oauth/ExchangeToken` | 登录换令牌 / 续期（无认证）|
| `api.trae.cn/cloudide/api/v3/trae/oauth/GetPCAuthCode` | 浏览器侧取 AuthCode（需门户 Cookie）|
| `api.trae.cn/cloudide/api/v3/trae/{CheckLogin,GetUserInfo}` `common/GetUserToken` | 登录态 / 用户信息 |
| `api.trae.cn/trae/api/v2/ug/{checkin_credits/status,checkin_credits/claim,activity/info}` | 签到 |
| `api.trae.cn/trae/api/v2/pay/{ide_user_ent_usage,ide_user_pay_status}` | 积分余额 |
| `www.trae.cn/authorization` | 登录页（浏览器打开）|
| `api.trae.com.cn/icube/api/v1/user`、`native/config/query` | 设备/用户登记、远端配置 |
| `trae-api-cn.mchost.guru` | `product.json` 声明的 agent host，抓包中 0 次实际请求；`api5-normal` 下发来源未定位 |
| `trae-ws-cn.mchost.guru/custom_model` | `product.json` 声明的 WS，本实现不用 |

## 1. 登录方式

自动 PKCE（S256），`client_id` 固定 `ono9krqynydwx5`。三步：

```
1) 浏览器打开
   https://www.trae.cn/authorization?login_version=1&auth_from=trae&login_channel=native_ide
     &plugin_version=2.3.102387&auth_type=local&client_id=ono9krqynydwx5&redirect=0
     &login_trace_id=<uuid>&auth_callback_url=http://127.0.0.1:40909/authorize
     &machine_id=<64hex>&device_id=<num>&x_device_id=<num>&x_device_brand=&x_device_type=linux
     &x_os_version=Arch%20Linux&x_env=&x_app_version=3.4.18&x_app_type=stable
     &code_challenge=<b64url43>&code_challenge_method=S256&channel_name=common
   用户在页面完成抖音/手机号登录。

2) POST https://api.trae.cn/cloudide/api/v3/trae/oauth/GetPCAuthCode   （需门户 Cookie）
   {"ClientID":"ono9krqynydwx5","CodeChallenge":"…","CodeChallengeMethod":"S256",
    "DeviceID":"…","PlatformCode":"IDE_PC"}
   → Result.AuthCode（ExpireDuration 600000，10 分钟）

3) POST https://api.trae.cn/trae/api/v3/oauth/ExchangeToken   （无认证）
   {"ClientID":"ono9krqynydwx5","AuthCode":"…","CodeVerifier":"…",
    "DeviceInfo":{"DeviceID","MachineID","PlatformCode":"IDE_PC","DeviceType":"PC",
      "DeviceName","DeviceModel":"","ClientVersion":"3.4.18",
      "DevicePublicKey":"<PEM EC prime256v1>","DeviceBrand":"","DeviceCPU":"…",
      "OSInfo":"linux","OSVersion":"Arch Linux"},
    "IDEVersion":"3.4.18"}
   → Result.{Token, RefreshToken, RefreshExpireAt, BoundDeviceID, ClientID, DeviceBindStatus}
```

要点：

* 回调端口 `40909`（官方硬编码；参考实现用 18080）。`S256(CodeVerifier)` = `CodeChallenge`。
* 服务端会绑定设备：`DeviceBindStatus: BOUND`，`RefreshExpireAt - iat` ≈ 180 天。
* `Token` 是 JWT（`exp` 约 14 天；实测样本 `exp` 2026-11-28）。
* `machine_id`（64hex）/`device_id`（数字）与 `DevicePublicKey` 同源于本机生成的 EC P-256 密钥对，须一起持久化，不能每次随机。

存储 `providers/trae/credential.toml`：

```toml
token = "<Cloud-IDE-JWT>"
refresh_token = "<…>"
refresh_expire_at = "<RFC3339>"
machine_id = "<64hex>"
device_id = "<num>"
device_private_key_pem = "<EC prime256v1>"   # ExchangeToken 用，须与已绑定的 DevicePublicKey 配对
```

## 2. 令牌续期方式

```
POST https://api.trae.cn/trae/api/v3/oauth/ExchangeToken   （无认证）
```

用 `RefreshToken` 换 `Token`。**未验证**：本次抓包未触发续期，具体请求体来自参考实现
`trae-oauth.ts` 分支 1（`GrantType=refresh_token` 形态待核实），字段名与步骤 3 的
`{ClientID, AuthCode, …}` 不同，实现前必须实测一次。

`Token` 过期后的行为**未验证**（预期 `401 code 1001`）。策略：`exp` 前主动续期 + `401` 兜底再续一次。

## 3. Chat Completions协议转换

### 3.1 端点与请求头

`POST https://api5-normal.mchost.guru/api/agent/v3/llm_utils_chat`

curl 递增实验得出的准入条件（缺一即失败，实测）：

* `user-agent: TraeClient/TTNet` — **限流按 UA 分桶**，curl 默认 UA 持续 `4011 rate limit`，同请求换 TTNet 立即放行。UA 是准入标签，不是标识。
* `authorization: Cloud-IDE-JWT <jwt>` — 只带它则 `400 expr_path=app_version_code`。
* `x-app-version` + `x-app-version-code` + `x-ide-version` + `x-ide-version-code` 四者齐全，缺一律报 `app_version_code`。
* `content-type: application/json`。

其余照抄官方 §3.1 全集（实测 `x-machine-id`/`x-device-id` 不强制，但为维护标准一致仍整套发送）：

```
user-agent: TraeClient/TTNet
authorization: Cloud-IDE-JWT <jwt>
content-type: application/json
accept: */*
x-ide-version: 3.4.18
x-ide-version-code: 20261110
x-app-version: 3.4.18
x-app-version-code: 20261110
x-machine-id: <64hex>
x-device-id: <num>
x-device-cpu: Intel
x-device-type: linux
x-os-version: Arch Linux
x-flow-traceparent: 04-<32hex>-<16hex>-01
```

官方**不发** `X-Cloudide-Token` / `X-Ide-Token` / `X-Uid` / `X-App-Id`；参考实现
`traeSOLOHeaders()` 那几个是过期格式。版本号与 UA 是抓包常量，客户端升级时属维护项。

### 3.2 请求映射（Chat Completions → 上游）

固定常量：`app_id`（见 §0 出入 6）、`function`、`stream: true`。

| Chat Completions | 上游 body | 规则 |
|-|-|-|
| `model: "trae/glm-5.2"` | `model` + `config_name` | 剥 `trae/` 前缀，两字段同值；实测 `glm-5.2`、`deepseek-v4.1-flash`（大小写不敏感需按目录 `config_name` 归一）|
| — | `function` | 本实现固定 `solo_work_lite`；官方主对话用 `solo_agent` |
| — | `stream` | 固定 `true`（明文路径只验过流式）|
| `messages[].role: user` | `{role:"user",content:[{type:"text",text:"…"}]}` | `content` **必须是数组**，否则 `cannot unmarshal string into …LLMRawMessage.messages.content of type []*idecopilot.LLMRawMessageContent` |
| `messages[].role: system` | — | **未验证**，见 §6 |
| `messages[].role: assistant` + `tool_calls` | `content:[]` + `tool_calls:[{id,type:"function",function_call:{name,arguments}}]` | 键名是 **`function_call`**，不是 CC 的 `function`；与参考实现 `transformSOLOMessage` 一致 |
| `messages[].role: tool` | `{role:"tool",tool_call_id,name,content:[{type:"text",text:"…"}]}` | `content` 同样是数组；`tool_call_id` 与 assistant 侧 `id` 对应 |
| `tools[].function.parameters`（对象）| 字符串化 JSON | **必须 stringify**，否则 `cannot unmarshal object into …FunctionDefinition.tools.function.parameters of type string`；实测上游按此 schema 生成并回调工具 |
| `tools[].function.{name,description}` | 同名 | — |
| 其余忽略字段 | 不发 | 见 §0 不支持列表 |

实测通过的对话 body：

```json
{"app_id":"6eefa01c-1036-4c7e-9ca5-d891f63bfcd8",
 "model":"glm-5.2","config_name":"glm-5.2","function":"solo_work_lite",
 "stream":true,
 "messages":[{"role":"user","content":[{"type":"text","text":"你好"}]}]}
```

实测通过的工具往返 body（我方注入 tools，上游接受并回调，与 web 版「无法注入工具」相反）：

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

### 3.3 响应映射（上游 SSE → CC chunk）

上游是标准 SSE 分帧（`id:N\nevent:<name>\ndata:<json>\n\n`），按空行切分即可，不需要第三方 eventsource 库。实测帧类型（`llm_utils_chat` 明文路径，含工具那次共 61 帧：57 `output`）：

| 上游 event | data | → Chat Completions |
|-|-|-|
| `metadata` | `{model:""<常空>, session_id:<uuid>, prompt_completion_id, ab_versions, app_version, ide_version, prompt_cache_pool_record}` | 首 chunk：`id="chatcmpl-"+本地生成`、`created` 取本地时间、`model` 回填请求模型名、`object=chat.completion.chunk`、`choices[0].delta.role="assistant"` |
| `timing_cost` | `{name:"llm_raw_chat_v2", provider_model_name, 各段耗时}` | 丢弃（`provider_model_name` 是内部推理服务名，如 `ep-20261020194436-p4fjc`，不外泄）|
| `output` | `{response, reasoning_content, tool_calls, multimodal_contents, phase}` | `delta.content ← response`（增量）；`delta.reasoning_content ← reasoning_content`（增量，非 CC 标准字段，见下）；`delta.tool_calls ← tool_calls`（按下文累积）|
| `extra_info` | `{model, version:1, content:<字符串化 reasoning_content>, input_token, expire_time}` | 丢弃 |
| `token_usage` | `{prompt_tokens, completion_tokens, total_tokens, reasoning_tokens, cache_read_input_tokens, cache_creation_input_tokens, cluster, *_total}` | `usage.prompt_tokens`/`completion_tokens`/`total_tokens`；`prompt_tokens_details.cached_tokens ← cache_read_input_tokens`；`completion_tokens_details.reasoning_tokens ← reasoning_tokens`；`cache_creation_input_tokens` 无 CC 标准位置，丢弃 |
| `done` | `{finish_reason:"stop"}` | `choices[0].finish_reason`；若本轮出现 tool_calls 则改写为 `tool_calls`（§0 出入 4），末帧后 `[DONE]` |

`output` 帧实测规则：

* `response` 与 `reasoning_content` **同帧互斥填充**，都是**增量**（与 web 版 `plan_item` 累积快照不同）。首帧样例 `{"response":"本机","reasoning_content":null,"tool_calls":null,"multimodal_contents":null,"phase":null}`。
* `tool_calls` 增量按 `index` 累积：首帧带 `index/id/type/function_call.name`，`arguments` 为空串；后续帧 `id:"" type:"" name:""`，只在同一 `index` 上补 `arguments` 片段。`partial_arguments`/`namespace` 实测恒 `null`。

```json
{"tool_calls":[{"index":0,"id":"call_00_gu39ipiquzoj4c2v53mkyaq7","type":"function",
  "function_call":{"name":"get_system_info","arguments":"","partial_arguments":null,"namespace":null}}]}
{"tool_calls":[{"index":0,"id":"","type":"","function_call":{"name":"","arguments":"{}","partial_arguments":null,"namespace":null}}]}
```

* `reasoning_content` 在 CC 规范里没有（`delta` 只有 `content`/`refusal`/`tool_calls`/`role`/`audio`/`function_call`）。按业已通行的 `delta.reasoning_content` 透传，属主动扩展。
* 实测工具回传后模型正确消费回传内容（表格复述了我给的虚构配置）→ 多轮工具链打通。

### 3.4 模型列表（`/v1/models`）

```
POST https://api5-normal.mchost.guru/api/ide/v1/batch_get_detail_param   （明文）
{"functions":["personal_assistant","assistant","solo_agent","solo_agent_lite","solo_coder",
  "solo_agent_remote","solo_work_lite","solo_work_remote","solo_design_lite",
  "solo_design_remote","builder"],
 "agent_type":"","current_config_info":{"config_name":"","is_custom_model":false},
 "mode_type":0,"access_type":1,"ab_force_vids":"","ab_autotest_advanced_mode":0}
```

响应 2757110 字节：`{allow_tenant_user_add_model, client_config, client_config_ab_versions, function_configs[11]}`。
每个 `function_configs[i] = {function, config_info_list[], ab_versions[]}`；`solo_work_lite` 下 48 个模型。

单条目字段：`config_name` `config_source` `config_switch` `context_window_tokens{dev,max}`
`display_config` `display_contact_config`（字符串化 JSON：`access.identity_list`、`consumption_rate.data.rate`、`reasoning.enable`）
`extra_config`（字符串化 JSON）`is_default` `is_invisible_to_user` `model_detail_list[]`
`reasoning_effort_config{support_thinking,options,default_level}` `usage`（`chat_completion`/`summary`/`title_generation`）。

映射为 CC `/v1/models`：取 `function == "solo_work_lite"` 且 `usage == "chat_completion"` 的条目，
`id = "trae/" + config_name`、`created` 无来源（本地时间）、`context_window_tokens.dev` 作 `max_model_len`。
名字规则：目录 `config_name` 小写（`deepseek-v4.1-flash`），显示名 `display_name` 混合大小写
（`DeepSeek-V4.1-Flash`），内部 `model_detail_list[].model_name` 带 `__dev` 后缀。请求侧用 `config_name`。

### 3.5 请求体加密（背景，本实现不用）

官方对 `/api/agent/v3/*` 的请求体加密（base64 → 高熵字节，无魔术头）：
`llm_utils_chat` 2415B/entropy 7.919，`create_agent_task` 30000B(日志截断)/7.994，
`sync_history_state` 122B/6.513，`workflow/start` 491B/7.608、735B/7.699。响应方向恒明文。
`libharness.so` 有 `harness/cloud-api/src/shared/event_encryption.rs` 及
`encryption_version`/`unprotected`/`cloud SSE encryption protocol rejected` 字符串。
服务端不强制（§0 出入 2），故不逆向。若上游改为强制，回来处理。

## 4. 签到

`provider-info/template.md` 无此节，按 README「自动签到」补。Host `api.trae.cn`，头见 §3.2 VSCode 套（不带 `x-machine-id`，实测确认）。

```
POST /trae/api/v2/ug/checkin_credits/status   {"req_source":1}
  → {"checked_in":true,"code":0,"credits":100,"did_checked_in":false,
     "enable":true,"extra_credits":100,"message":"success"}

POST /trae/api/v2/ug/checkin_credits/claim    {"req_source":1}    未验证：抓包时已签到，未取到成功响应
POST /trae/api/v2/ug/activity/info            {"version_code":"3.4.18","req_source":1}
  → commercial_activities[]: checkin_credits(type 104, work_credits 200),
    new_user_credits(102,2000), send_message_reward(105,200), student_auth_reward(107),
    code_fission_202609(103), work_desktop_download…
POST /trae/api/v2/pay/ide_user_ent_usage      {"require_usage":true,"req_source":1}
  → total_balance, usage_summary, balance_summary[], user_entitlement_pack_list[]
POST /trae/api/v2/pay/ide_user_pay_status     {}
```

`req_source`：桌面 `1`，web `3`。实现流程：`status.enable && !status.checked_in` → `claim`，每 1h 一轮。
实测对话消耗：`total_balance` 5579.62 → 5577.48（-2.14）。

## 5. 官方 create_agent_task 路径（备查，本实现不用）

主回答事件序列（抓包，一次 `hi` 共 24 帧）：`agent_identity` → `task_created` → `model_config`
→ `agent_status` → `chat_memory_trigger` → `context_usage` → `history`×2 → `metadata` →
`timing_cost` → **`thought`**（正文/思考增量，字段 `reasoning_content`+`thought` 互斥）→
`token_usage` → `required_context` → `turn_completion{task_completion:true}`，
配合 `agent_status.status=completed` 收尾。

ID 体系（从明文 `batchInsert` 可见）：`chat_session_id`/`message_id`/`turn_id`/`task_id`(`task_`前缀)
/`plan_item`(`pi_`前缀) 各 24hex；`trace_id` 即 `x-flow-traceparent` 第二段。
落库 `model_info` 含 `reasoning_effort_level:"high"`、`config_name`、`prompt_max_tokens`
（用户侧 936000，assistant 回显 168000）、`multimodal:true`、`use_remote_service:true`。
明文 `create_agent_task` 请求已确认必填 `conversation_id`，完整 schema 未知。

## 6. 未验证 / 待办

1. `system` 角色消息上游怎么处理（客户端把 PE 放在服务端，`llm_utils_chat` 明文路径未测 system）。
2. 上游请求体带未知/不支持字段时是忽略还是 400（决定 §0 忽略策略是否安全）。
3. 非流式（`stream:false`）行为未测。
4. `RefreshToken` 续期请求体未实测（§2）。
5. 未签到态 `claim` 成功响应与错误码未抓（§4）。
6. `api5-normal` vs `trae-api-cn` 选择策略未定；`api5-normal` 下发来源未定位（抓包响应体里搜不到 `api5`/`mchost`，可能在截断的大响应内）。
7. `multimodal_contents` / `phase` 字段何时非空未观察到，图片输入不支持。
8. `tools` 注入时 `function` 用 `solo_work_lite` 还是 `solo_agent`（工具往返那次实测用的是 `solo_agent`）需定稿。
