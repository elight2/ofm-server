# trae 客户端协议

来源：2026-11-14 用 mitmproxy 抓取 Trae CN 1.107.1（appVersion 3.4.18）真实流量。
抓包脚本 `tmp/trae-capture/log_flows.py`，原始摘要 `tmp/trae-capture/requests.log`。
所有值均为实测；未经证实的标「未验证」。

## 0. 一句话结论

客户端发出的对话请求体是**密文**（base64 → 高熵随机字节，entropy≈7.99）。
**但服务端不强制加密**：同一端点发明文 JSON 直接被解析并正常返回（§5.1 实测）。
主回答来自 `/api/agent/v3/create_agent_task`；`llm_utils_chat` 只做标题/图标。
明文链路已实测**对话 + 工具调用完整可用**，无需逆向加密。

## 1. 域名

| 用途 | Host | 来源 |
|-|-|-|
| 对话/agent | `https://api5-normal.mchost.guru` | 实测（客户端实际使用）|
| 配置声明的 agent host | `https://trae-api-cn.mchost.guru` | `product.json` → `bootConfig/agent/trae/normal` |
| 控制面（登录/用户）| `https://api.trae.cn` | `bootConfig/account/trae` |
| 控制面（icube/市场）| `https://api.trae.com.cn` | `bootConfig/iCube` |
| 签到/积分 | `https://api.trae.cn/trae/api/v2/…` | 实测 |

`api5-normal` 与 `trae-api-cn` 的差异**未定位到下发来源**（抓包内所有响应体里都搜不到
`api5`/`mchost` 字样，可能在被截断的大响应里）。实现时应向用户确认用哪个，或按
`native/config/query` 动态解析。

## 2. 认证

三层，全部实测通过：

### 2.1 登录链（PKCE）

```
1) GET https://www.trae.cn/authorization
     ?login_version=1&auth_from=trae&login_channel=native_ide
     &plugin_version=2.3.102387&auth_type=local&client_id=ono9krqynydwx5
     &redirect=0&login_trace_id=<uuid>&auth_callback_url=http://127.0.0.1:40909/authorize
     &machine_id=<64hex>&device_id=<num>&x_device_id=<num>&x_machine_id=<64hex>
     &x_device_brand=&x_device_type=linux&x_os_version=Arch%20Linux&x_env=
     &x_app_version=3.4.18&x_app_type=stable
     &code_challenge=<b64url43>&code_challenge_method=S256&channel_name=common
   → 登录页（浏览器完成抖音/手机号登录）

2) POST https://api.trae.cn/cloudide/api/v3/trae/oauth/GetPCAuthCode
   body {"ClientID":"ono9krqynydwx5","CodeChallenge":"…","CodeChallengeMethod":"S256",
         "DeviceID":"…","PlatformCode":"IDE_PC"}          需门户 Cookie
   → Result.AuthCode（ExpireDuration 600000）

3) POST https://api.trae.cn/trae/api/v3/oauth/ExchangeToken    无认证
   body {"ClientID":"ono9krqynydwx5","AuthCode":"…","CodeVerifier":"…",
         "DeviceInfo":{"DeviceID","MachineID","PlatformCode":"IDE_PC","DeviceType":"PC",
           "DeviceName","DeviceModel":"","ClientVersion":"3.4.18",
           "DevicePublicKey":"<PEM EC prime256v1>",
           "DeviceBrand":"","DeviceCPU":"…","OSInfo":"linux","OSVersion":"Arch Linux"},
         "IDEVersion":"3.4.18"}
   → Result.{Token, RefreshToken, RefreshExpireAt, BoundDeviceID, ClientID, DeviceBindStatus}
```

要点：

- `client_id = ono9krqynydwx5` 固定
- 回调端口实测 `40909`（参考实现用 18080）
- `CodeVerifier` → `S256(CodeVerifier)` = `CodeChallenge`，标准 PKCE
- **`DevicePublicKey` 会被服务端绑定**（`DeviceBindStatus: BOUND`）
  → 这是加密能力的密钥来源候选，见 §5
- `Token` = JWT（`Cloud-IDE-JWT`），`RefreshToken` 长期有效
  （实测 `RefreshExpireAt - iat` ≈ 180 天）

### 2.2 续期

```
POST https://api.trae.cn/trae/api/v3/oauth/ExchangeToken
```
用 RefreshToken 换 Token（参考实现 `trae-oauth.ts` 分支 1）。
本次抓包未触发续期，**未验证具体请求体**。

### 2.3 两种 token 头

| 头 | 值 | 用在哪 |
|-|-|-|
| `Authorization: Cloud-IDE-JWT <jwt>` | JWT | 对话、模型列表、签到、积分 |
| `x-icube-token: <jwt>` | 同一个 JWT | `api.trae.com.cn/icube/*` |
| 门户 Cookie | `passport/token` | 仅 `GetPCAuthCode`、`GetUserToken`、`GetUserInfo` |

## 3. 请求头（两套，按发起进程分）

### 3.1 TTNet（Rust agent 进程，`libsscronet.so`）

用于 `api5-normal.mchost.guru/api/agent/v3/*`、`/api/ide/v1/*`：

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
request-traffic-type: prod          （部分端点带）
```

**没有** `X-Cloudide-Token` / `X-Ide-Token` / `X-Uid` / `X-App-Id` —— 参考实现
`traeSOLOHeaders()` 发的那几个是旧格式。

### 3.2 VSCode/Electron 进程

用于 `api.trae.cn/trae/api/v2/*`（签到、积分）：

```
user-agent: VSCode 1.107.1 (Trae CN)
authorization: Cloud-IDE-JWT <jwt>
content-type: application/json
accept: */*
x-app-version: 3.4.18
x-device-id: <num>
x-device-type: linux
x-os-version: Arch Linux
```

签到请求**不带** `x-machine-id`（参考实现带）。

### 3.2b 最小必带头集（2026-11-15 curl 递增实验实测）

- 只带 `authorization` → `400 expr_path=app_version_code, missing required parameter`
- `x-app-version` + `x-app-version-code` + `x-ide-version` + `x-ide-version-code`
  四者齐全才过参数校验（缺一律报 app_version_code）
- `user-agent` 必须是 `TraeClient/TTNet`：curl UA 持续撞 `4011 rate limit`，
  同请求换 TTNet UA 立即放行 → **限流按 UA 分桶，UA 是准入标签**
- `x-machine-id` / `x-device-id` 实测不强制（无认证层依赖），实现仍整套照抄 §3.1
- 版本号/UA 写死为抓包常量，客户端升级时属维护项

### 3.3 Electron 桌面壳

用于 `ExchangeToken`、`icube/api/v1/user`：

```
user-agent: Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) TraeCN/1.107.1 Chrome/142.0.7444.235 Electron/39.2.7 Safari/537.36
```

## 4. 对话

### 4.1 端点分工（实测，重要）

| 端点 | 实际用途 | 请求体 | 响应 |
|-|-|-|-|
| `POST /api/agent/v3/create_agent_task` | **主对话**（用户消息的回答）| **密文** | SSE 明文 |
| `POST /api/agent/v3/llm_utils_chat` | 会话标题+图标生成（客户端）；一次性工具任务 | **密文**（明文亦可，§5.2）| SSE 明文 |
| `POST /api/agent/v3/workflow/start` | `summarize_message` / `extract_topics_and_update_memory` | **密文** | SSE 明文 |
| `POST /api/agent/v3/sync_history_state` | 历史状态同步 | **密文** | JSON |
| `POST /api/solo_hub/v1/conversations/messages/batchInsert` | 会话消息落库 | 明文 JSON | JSON |
| `POST /api/ide/v1/batch_get_detail_param` | 模型列表（11 个 function）| 明文 JSON | JSON 2.7MB |
| `POST /api/ide/v1/get_client_config` | 端点配置 | 明文 JSON | JSON |

这印证了参考实现 `TROUBLESHOOT.md` 的判断：`llm_utils_chat` 不用于主会话。

### 4.2 create_agent_task 响应 SSE（这是要解析的主体）

事件顺序实测（一次 `hi`，24 帧）：

```
id:N\nevent:<name>\ndata:<json>\n\n
```

| event | 内容 |
|-|-|
| `agent_identity` | `{agent_type:"solo_agent",agent_id:"solo_agent",agent_name:"SOLO",agent_run_id}` |
| `task_created` | `{task_id, agent_run_id}` |
| `model_config` | `{config_name, model_name:"<name>__dev", extra_config, model_extra_config{…}}` |
| `agent_status` | `{agents:[{agent_run_id,status:"running"/"completed",run_mode}]}` |
| `chat_memory_trigger` | 记忆触发 |
| `context_usage` | `{items:[], environment_context_used:true}` |
| `history` | 历史（2 次）|
| `metadata` | `{model, session_id, prompt_completion_id, prompt_cache_pool_record}` |
| `timing_cost` | 服务端耗时 |
| **`thought`** | **正文增量**，见下 |
| `token_usage` | token 统计 |
| `required_context` | `{contexts:["pending_message"]}` |
| `turn_completion` | `{task_completion:true}` 结束 |

`thought` 帧结构：

```json
{"agent_type":"solo_agent","agent_id":"solo_agent","agent_name":"SOLO",
 "agent_run_id":"…","parent_agent_run_id":null,
 "first_data":false,
 "reasoning_content":"The",         // 思考增量
 "thought":""}                       // 正文增量
```

实测规则：

- `reasoning_content` 与 `thought` **互斥填充**：思考阶段 `thought` 为空、
  `reasoning_content` 有增量；正文阶段反之。两者都是**增量**（与 web 版
  `plan_item` 的累积快照不同）。
- 结束信号：`turn_completion` + `agent_status.status=completed`
- 本次 `hi` 无工具调用，所以没抓到 tool_call 帧 —— **工具调用的事件形态未验证**

### 4.3 会话/消息 ID 体系

从 `batchInsert` 明文可见完整结构：

```
chat_session_id  6af879daef7c2370639738e9      24hex
message_id       6af879daef7c2370639738ec      用户消息
turn_id          6af879daef7c2370639738ee      一轮
task_id          task_6af879daef7c2370639738ef
plan_item id     pi_6af879ddef7c2370639738f2
trace_id         09cc05ff55fe408f83deae885474f752   （= x-flow-traceparent 中段）
```

即 `x-flow-traceparent` 的第二段就是会话 `trace_id`，服务端用它串联。

落库消息体（明文，含模型信息）关键字段：

```json
{"chat_session_id":"…","message_id":"…","role":"user","message_index":1,
 "content":"[{\"type\":\"text\",\"text_content\":\"hi\"}]",
 "query":"[{\"type\":\"text\",\"data\":{\"content\":\"hi\"}}]",
 "user_message_context":"<字符串化 JSON>",
 "agent_type":"solo_agent","agent_id":"solo_agent","agent_name":"Agent",
 "trace_id":"…","agent_process_support":"v3"}
```

`user_message_context.model_info`：

```json
{"provider":"","reasoning_effort_level":"high","is_preset":true,
 "config_name":"deepseek-v4.1-flash","config_source":1,
 "model_name":"deepseek-v4.1-flash","display_model_name":"DeepSeek-V4.1-Flash",
 "ak":"","base_url":"","use_remote_service":true,"multimodal":true,
 "prompt_max_tokens":936000}
```

注意用户消息 `prompt_max_tokens: 936000`，assistant 侧回显 `168000` +
`model_name: "deepseek-v4.1-flash__dev"`。

## 5. 请求体加密：**客户端加密，服务端不强制**

### 5.1 客户端侧形态（抓包）

`/api/agent/v3/*` 的 request body 全是 base64：

| 端点 | 解码字节 | Shannon |
|-|-|-|-|
| `llm_utils_chat` | 2415 | 7.919 |
| `create_agent_task` | 30000（日志截断）| 7.994 |
| `sync_history_state` | 122 | 6.513 |
| `workflow/start` | 491 / 735 | 7.608 / 7.699 |

无明文前缀、无魔术头 → 对称加密。`libharness.so` 有
`harness/cloud-api/src/shared/event_encryption.rs` 与
`encryption_version` / `unprotected` / `cloud SSE encryption protocol rejected`。
响应方向始终明文。

### 5.2 服务端实测（2026-11-14 直接 curl，明文 body）→ **阻塞解除**

对照组先确认「认证先于 body 解析」，所以 400 里的字段名是真提示，不是兜底错误：

| 请求 | 结果 |
|-|-|
| 无 JWT + 明文 body | `401` code 1001 `not able to authenticate` |
| 有 JWT + 明文 body 缺 app_id | `400` `binding: expr_path=app_id, cause=missing required parameter` |
| 有 JWT + 明文 body 含 `app_id` | **`200` + 正常 SSE 回答** |
| `create_agent_task` 明文 body | `400` `expr_path=conversation_id, cause=missing required parameter` |

即：**服务端按普通 JSON 绑定明文请求体**，`create_agent_task` 也接受明文（它已经在
找 `conversation_id` 这个业务字段了）。加密不是准入条件。
→ **不需要逆向 `libharness.so`。**

`app_id = 6eefa01c-1036-4c7e-9ca5-d891f63bfcd8`，可放 body `app_id` 或头 `X-App-Id`
（两种都实测通过）。值取自参考实现 `trae-product.ts:322`。

### 5.3 实测通过的明文请求格式（`llm_utils_chat`）

三个必须点，全是靠 400 报错定位出来的：

1. **`app_id` 必填**
2. **`tools[].function.parameters` 是字符串化 JSON**，不是对象
   否则 `cannot unmarshal object into …FunctionDefinition.tools.function.parameters of type string`
3. **`messages[].content` 是数组** `[{type:"text",text:"…"}]`，不能是字符串
   否则 `cannot unmarshal string into …LLMRawMessage.messages.content of type []*idecopilot.LLMRawMessageContent`

对话：

```json
{"app_id":"6eefa01c-1036-4c7e-9ca5-d891f63bfcd8",
 "model":"glm-5.2","config_name":"glm-5.2","function":"solo_work_lite",
 "stream":true,
 "messages":[{"role":"user","content":[{"type":"text","text":"你好"}]}]}
```

工具调用（**我方注入 tools，实测上游接受并回调**，与 §9 里 web 版「无法注入工具」的
结论相反 —— 桌面端点可以）：

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

注意 assistant 侧是 `function_call`（不是 OpenAI 的 `function`），与参考实现
`transformSOLOMessage` 一致。

### 5.4 实测响应 SSE（明文路径）

```
event:metadata      {model:"", session_id:<uuid>, prompt_cache_pool_record:null}
event:timing_cost   {name:"llm_raw_chat_v2", …, provider_model_name:"glm-5.2"}
event:output        {response, reasoning_content, tool_calls,
                     multimodal_contents, phase}      ← 增量
event:extra_info    {model, version:1, content:"<字符串化 reasoning_content>",
                     input_token, expire_time}
event:token_usage   {prompt_tokens, completion_tokens, total_tokens,
                     reasoning_tokens, cache_read_input_tokens,
                     cache_creation_input_tokens, cluster}
event:done          {finish_reason:"stop"}
```

- `response` = 正文增量，`reasoning_content` = 思考增量，两者同帧互斥
- tool_calls 增量形态：
  `{"index":0,"id":"call_00_…","type":"function","function_call":{"name":"get_system_info","arguments":"","partial_arguments":null,"namespace":null}}`
  后续帧在**同一 `index`** 上补 `arguments` 片段
- 实测工具回传后模型正确消费（表格形式复述了我给的虚构配置）→ **多轮工具链打通**

### 5.5 遗留（不阻塞实现）

- 明文是否是长期受支持路径，未知。若上游改为强制加密，再回来做 §5.1 的逆向。
- 桌面客户端真实主会话用 `create_agent_task`，其完整明文 schema 未知
  （只知道必填 `conversation_id`）。参考实现用 `llm_utils_chat` 已能通过
  `MAINTAIN.md` 两条验收，先按此实现。

## 6. 模型列表

```
POST https://api5-normal.mchost.guru/api/ide/v1/batch_get_detail_param
```

请求体（明文）：

```json
{"functions":["personal_assistant","assistant","solo_agent","solo_agent_lite",
  "solo_coder","solo_agent_remote","solo_work_lite","solo_work_remote",
  "solo_design_lite","solo_design_remote","builder"],
 "agent_type":"","current_config_info":{"config_name":"","is_custom_model":false},
 "mode_type":0,"access_type":1,"ab_force_vids":"","ab_autotest_advanced_mode":0}
```

响应 2.7MB，`function_configs[]` 每 function 一套模型目录。每个模型：

```
config_name / model_name("<name>__dev") / display_name / config_source / is_default
context_window_tokens{dev,max} / max_mode / multimodal / tool_response_multimodal
model_detail_list[]{max_tokens,max_turn,prompt_max_tokens,model_extra_config}
display_contact_config  ← 字符串化 JSON: access.identity_list, consumption_rate.data.rate, reasoning.enable
reasoning_effort_config{support_thinking, options, default_level}
usage                   ← chat_completion | summary
```

`access.identity_list` 是**准入身份白名单**（实测 `[0,5,1,2,3,100]`）。

模型名映射规则（实测）：`config_name` 用户可见，`model_name` 带 `__dev` 后缀；
`DeepSeek-V4.1-Flash` 的 `config_name` 是 `deepseek-v4.1-flash`（大小写不同）。

## 7. 签到

Host `https://api.trae.cn`，头见 §3.2。

```
POST /trae/api/v2/ug/checkin_credits/status   {"req_source":1}
  → {"checked_in":true,"code":0,"credits":100,"did_checked_in":false,
     "enable":true,"extra_credits":100,"message":"success"}

POST /trae/api/v2/ug/checkin_credits/claim    {"req_source":1}      本次未抓到（已签到）

POST /trae/api/v2/ug/activity/info            {"version_code":"3.4.18","req_source":1}
  → commercial_activities[]: checkin_credits(type104, work_credits200),
    new_user_credits(102, 2000), send_message_reward(105, 200),
    student_auth_reward(107), code_fission_202609(103), work_desktop_download…

POST /trae/api/v2/pay/ide_user_ent_usage      {"require_usage":true,"req_source":1}
  → total_balance, usage_summary{consumed_amount,total_amount,consumption_ratio},
    balance_summary[], user_entitlement_pack_list[]

POST /trae/api/v2/pay/ide_user_pay_status     {}
```

`req_source`：桌面客户端 = `1`，web = `3`。

## 8. 其它实测端点

| 端点 | 说明 |
|-|-|
| `POST api.trae.cn/cloudide/api/v3/trae/CheckLogin` `{"GetNickNameEditStatus":true}` | 登录态，`Result.{UserID,WID,Host,Region,ExpiredAt}` |
| `POST api.trae.cn/cloudide/api/v3/common/GetUserToken` 空 body + Cookie | 换 JWT |
| `POST api.trae.cn/cloudide/api/v3/trae/GetUserInfo` 空 body + Cookie | 用户信息 |
| `POST api.trae.com.cn/icube/api/v1/user` | 设备/用户登记，body 含 `mid/did/uid/packageType/productCode/platform/branch/arch/tenant/appVersion/buildVersion/traeVersionCode/scope`，头用 `x-icube-token` |
| `GET api5-normal…/api/remote/v1/chat_sessions?page_size=100` | 会话列表 |
| `GET api5-normal…/api/remote/v1/skills` `/plugins` `/git/repositories` `/explorer` `/scheduled_tasks` | SOLO 资源 |
| `POST api5-normal…/api/solo_hub/v1/clis/register` | CLI 注册 |
| `GET api5-normal…/api/solo_hub/v1/wsmessages/poll` | 消息轮询 |
| `POST api.trae.com.cn/icube/api/v1/native/config/query?mid=…` | 远端配置 |
| `GET api.trae.com.cn/service/settings/v3/?os=linux&aid=1006732&…` | AhaNet 配置（`aid=1006732`）|

## 9. 参考实现差异（`tmp/dsh-ofm-ng`、`tmp/dsh-our-free-model`）

| 项 | 参考实现 | 本次客户端实测 |
|-|-|-|
| agent host | `trae-api-cn.mchost.guru` | `api5-normal.mchost.guru` |
| 对话端点 | `llm_utils_chat` | `create_agent_task` |
| 请求体 | 明文 OpenAI 风格 JSON | **base64 密文** |
| SSE | `event:output` | `event:thought` + `agent_identity`/`turn_completion` |
| UA | 自定义字符串 | `TraeClient/TTNet` |
| 头集合 | `X-Cloudide-Token`/`X-Ide-Token`/`X-Uid`/`X-App-Id` | 不存在这些；只有 §3.1 |
| `X-Device-Type` | `macos` | `linux`（跟随真实 OS）|
| 签到的 `req_source` | 未带 machine-id | `1`，且确实不带 machine-id |
| 登录回调端口 | 18080 | 40909 |
| function | `solo_work_lite` | 11 个，主对话 `solo_agent` |

结论：参考实现的协议**已过期或本就与桌面客户端不同**。不能照搬。

## 10. 待办（按依赖顺序）

1. ~~定 §5 加密方案~~ → **已解除**：明文实测通过，含工具调用（§5.2–§5.4）。
   逆向 `libharness.so` 不再需要。
2. ~~抓带工具调用的对话~~ → 已通过明文实测覆盖（§5.3–§5.4）。
3. 定登录方式（`MAINTAIN.md` 第 3 步），**需用户决定**：
   a) 自动 PKCE（§2.1），用户在浏览器完成一次抖音/手机号登录；
   b) 用户手工提供 RefreshToken（实测有效期 180 天，可自动续期）。
4. 定 §1 `api5-normal` vs `trae-api-cn` 的选择策略。
5. 抓一次**未签到状态**的 `claim`，确认成功响应与错误码。
