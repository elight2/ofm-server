# trae 接口协议（Web 版）

来源：2026-11-14 浏览器抓包（`www.trae.cn/agent`），账号 Free。
所有结论都来自真实请求/响应，未经证实的部分标「未验证」。

## 1. 认证

### 1.1 两层凭证

| 层 | 凭证 | 用途 | 传递方式 |
|-|-|-|-|
| 门户会话 | Cookie（passport/token 等） | 换取 JWT；仅对 `api.trae.cn` 的 cloudide 接口有效 | `credentials: include` |
| JWT | `Cloud-IDE-Token`（localStorage 明文串） | 对话、模型列表、签到 | `Authorization: Cloud-IDE-JWT <token>` |

关键实测：

- `POST https://api.trae.cn/cloudide/api/v3/common/GetUserToken`
  - 无 Cookie → `401`，`ResponseMetadata.Error.Code = 20101`
  - 带 Cookie → `200`，`Result = { Token, ExpiredAt, TenantID, UserID }`
  - `ExpiredAt` 为 ISO8601 带时区，实测有效期约 8 小时
  - 带 `Authorization` 但无 Cookie **不生效**（仍 401）
- 浏览器把 `Result.Token` 原样写入 `localStorage['Cloud-IDE-Token']`（996 字符，未加前缀）
- 使用方式：`Authorization: Cloud-IDE-JWT ` + 该串
- 纯 JWT（无 Cookie）可完成：模型列表、建会话、发消息、SSE、签到查询/领取
- 纯 JWT 不可完成：`GetUserToken`（换新 JWT）、`GetUserInfo`（返回 20310）

结论：**Cookie 是根凭证，JWT 是派生凭证**。server 版必须存储 Cookie，或存储用户
手工粘贴的 JWT（约 8 小时失效，无法自动续期）。

### 1.2 登录

未抓到登录跳转全过程（登录态已存在）。代码中可见：

- 登录页：`https://www.trae.cn/login`，参数 `redirect_url`、`hide_saas_login=true`
- 登录方式：抖音/手机号（`GetUserInfo.Result.LastLoginType` 实测为 `douyin`）

登录流程的具体回调未验证，需另行抓包。

## 2. 对话

### 2.1 请求链

```
POST /api/remote/v1/chat_sessions            创建会话（首条消息可内联）
POST /api/remote/v1/chat_sessions/{sid}/messages   追加消息（会话已存在时）
GET  /api/remote/v1/chat_sessions/{sid}/events?reply_to_message_id={mid}   SSE
```

Host：`https://www.trae.cn`
公共头：

```
authorization: Cloud-IDE-JWT <token>
content-type: application/json
x-trae-client-type: web
x-preferenced-language: zh-cn
x-trae-user-timezone: Asia/Shanghai
```

### 2.2 创建会话（含首条消息）

```json
{
  "mode": "code",
  "initial_message": {
    "chat_session_id": "",
    "content": [],
    "query": "[{\"type\":\"text\",\"data\":{\"content\":\"你好\"}}]",
    "model_name": "glm-5.3-flash",
    "agent_type": "solo_agent_remote",
    "agent_id": "solo_agent_remote",
    "model_selection_strategy": "manual",
    "custom_model": {
      "provider": "",
      "is_preset": true,
      "config_name": "glm-5.3-flash",
      "config_source": 1,
      "model_name": "glm-5.3-flash",
      "display_model_name": "GLM-5.3-Flash",
      "ak": "",
      "base_url": "",
      "use_remote_service": true,
      "multimodal": true,
      "reasoning_effort_level": "high"
    },
    "common_params": "<JSON 字符串>"
  },
  "env": "remote",
  "auto_create_project": false,
  "origin": "web"
}
```

响应：

```json
{"code":0,"data":{"chat_session_id":"6af8...","status":2,"message_id":"6af8...","remote_project_id":"rp_..."},"message":"success"}
```

要点：

- `query` 和 `common_params` 都是**字符串化的 JSON**，不是对象
- `model_name` 必须与 `custom_model.config_name` 一致
- 自动选模型：`model_selection_strategy: "auto"`，省略 `custom_model`，`model_name: ""`
- `reasoning_effort_level` 非法值不报错（实测传 `turbo` 仍 200），由上游静默降级

### 2.3 common_params

```json
{
  "language": "zh-cn", "app_language": "zh-cn", "quality": "stable",
  "app_version": "1.0.0.467",
  "web_id": "<CheckLogin.ResponseMetadata.WID>",
  "biz_user_id": "<GetUserInfo.Result.UserID>",
  "user_unique_id": "<同 UserID>",
  "scope": "marscode", "tenant": "marscode",
  "region": "CN", "aiRegion": "CN", "tenant_name": "",
  "solo_chat_mode": "code", "is_privacy_mode": 0, "privacy_mode": "off",
  "user_identity": "Free", "is_freshman": "1"
}
```

追加消息（`/messages`）时额外加：

```json
{"biz_session_id": "<当前会话 id>", "message_source": "manual"}
```

### 2.4 追加消息

```json
{
  "content": [],
  "query": "[{\"type\":\"text\",\"data\":{\"content\":\"你好4\"}}]",
  "model_name": "glm-5.3-flash",
  "agent_type": "solo_agent_remote",
  "agent_id": "solo_agent_remote",
  "model_selection_strategy": "manual",
  "custom_model": { ...同上... },
  "common_params": "<JSON 字符串，含 biz_session_id>"
}
```

响应：`{"code":0,"data":{"accepted":true,"message_id":"...","status":"in_progress"},"message":"success"}`

### 2.5 SSE

```
GET /api/remote/v1/chat_sessions/{sid}/events?reply_to_message_id={mid}
Accept: text/event-stream
```

响应：`200`，`text/event-stream; charset=utf-8`。
格式为 `id: <turn>:<seq>` + `event: <name>` + `data: <json>`，帧间空行。

事件类型（实测全部）：

| event | 含义 |
|-|-|
| `status_changed` | 会话状态：2=Creating 3=Running 5=Finished 6=Failed |
| `platform_timing` | 容器启动耗时 |
| `metadata` | 消息元数据（首帧 init，随后 done） |
| `session_title_message` | 自动生成标题 |
| `session_icon_message` | 自动生成图标 |
| `model_config` | 实际使用模型，`<name>__dev` |
| `heartbeat` | 心跳 `{"ts":...}` |
| `timing_events` | 服务端耗时明细 |
| `plan_item` | **主体**：思考 / 工具调用 / 回复 |
| `token_usage` | 每轮 token 统计 |
| `done` | 结束 |

`plan_item` 关键字段（均在同一 `id` 下反复推送**快照**）：

```json
{
  "id": "pi_...",
  "task_id": "task_...",
  "thought": "最终回复文本",
  "reasoning_content": "思考链（累积快照）",
  "timing": {"generated_at_ms":..., "tool_call_started_at_ms":..., "tool_call_finished_at_ms":...},
  "tool_call_info": {
    "id": "pi_...",
    "name": "Glob|Shell|finish|EnvironmentSetup|...",
    "params": {"path":"/workspace","pattern":"*"},
    "result": {
      "status": "success|failure",
      "error_message": "",
      "data": {"files":[]},
      "images": null, "render": null, "is_truncated": false, "interrupt": null
    },
    "meta": {"llm_toolcall_id": "call_..."}
  },
  "agent_status": {"status":"running|completed","run_mode":"foreground"},
  "__packet_seq__": 19
}
```

解析规则（实测）：

- 同一 `transaction id` 下 `__packet_seq__` 单调递增，必须按序合并
- `reasoning_content` 是**累积全文**，不是增量；新 `plan_item` 开始时清零
- 文本回复从 `thought` 取（也是累积全文）
- `tool_call_info.name` 先为空（纯思考），随后填名，再填 `params`，最后填 `result`
- 流结束标志：`event: done`
- `token_usage`：`prompt_tokens` / `completion_tokens` / `reasoning_tokens` / `total_tokens`，
  每次工具调用后各推一次；`cache_read_input_tokens` 实测恒 0

### 2.6 工具调用

上游 agent 自带工具并在**服务端**执行，server 无法注入自己的工具定义。
实测工具名：`Glob`、`Shell`、`EnvironmentSetup`、`finish`。
工具结果由上游执行后随 `plan_item.result` 下发。

结论：**无法做到「让模型调用我方的工具」**。若需要工具能力，只能在服务端
模拟 OpenAI 的 `tool_calls`——但上游不接受 tools 参数（未验证：未见 tools 字段，
请求体无 tools 位置）。

## 3. 模型列表

```
GET https://www.trae.cn/api/remote/v1/models?functions=solo_agent_remote,solo_work_remote,solo_design_remote&show_custom_model=true
Authorization: Cloud-IDE-JWT <token>
```

响应：

```json
{"code":0,"data":{"list":[{"function":"solo_agent_remote","models":[
  {"name":"glm-5.3-flash","multimodal":true,"is_default":false,
   "display_name":"GLM-5.3-Flash","is_new":false,"is_beta":false,
   "icon":{"dark":"...","light":"..."},
   "features":"{\"access\":{...},\"consumption_rate\":{...}}",
   "config_source":1,"is_preset":true,"max_mode":true,
   "context_window_tokens":{"dev":200000,"max":1000000},
   "reasoning_effort_config":{"support_thinking":true,
     "options":["light","high","extra_high"],"default_level":"high"}}
]}]},"message":"success"}
```

要点：

- `context_window_tokens.dev` = 常规窗口，`.max` = Max 模式窗口（0 表示不支持）
- `max_mode: true` 才可请求 1M；请求参数未验证
- `reasoning_effort_config.options` 为档位白名单，`default_level` 为默认档
- `features` 是字符串化 JSON，含计费倍率 `consumption_rate.data.rate`
- 实测三个 function 的模型数：`solo_agent_remote` 18、`solo_work_remote` 16、`solo_design_remote` 2
- 同模型在不同 function 下的能力**不同**（如 `deepseek-v4.1-flash` 在 work 下 `support_thinking=false`）

## 4. 签到

Host：`https://api.trae.cn`
认证：`Authorization: Cloud-IDE-JWT <token>`（纯 JWT 即可，值来自 Cookie 换取的 token）

### 4.1 状态

```
POST /trae/api/v2/ug/checkin_credits/status
{"req_source": 3}
```

```json
{"checked_in":true,"code":0,"credits":100,"did_checked_in":false,"enable":true,"extra_credits":100,"message":"success"}
```

字段含义：`checked_in` 今日已签；`enable` 活动是否开放；`credits` 本次基础积分；
`extra_credits` 额外积分。

### 4.2 领取

```
POST /trae/api/v2/ug/checkin_credits/claim
{"req_source": 3}
```

实测返回 `{"code":9004,"message":"The submitted order parameters are incorrect..."}`。
同时 `status` 显示 `checked_in: true`，说明今日已签（9004 是「已领取」的错误码表现，
但错误文案不指向该含义）。

代码中的错误码表（`1693.41f23348.js`）：

| code | 含义 |
|-|-|
| 1002 | 暂时无法验证账号，请重新登录 |
| 2001 | 服务异常 |
| 9004 | 请求参数异常 |
| 9090 | 活动暂不可用 |
| 9093 | 活动配置异常 |
| 9095 | 该设备今日已参与签到 |

未验证：9004 是否等价于「今日已签」。需在未签到账号上复测。

### 4.3 兼容端点

- `POST /trae/api/v2/ug/activity/info` `{"req_source":3}`：活动清单，含 `checkin_credits`
- `POST /trae/api/v2/ug/activity/action` `{"activity_id":"checkin_credits","req_source":3}`：实测 `9090`
- `POST /trae/api/v2/ug/checkin_credits/test/mock_history`：测试用
- `POST /trae/api/v2/ug/checkin_credits/test/rollback`：测试用

### 4.4 积分余额

```
POST /trae/api/v2/pay/web_user_ent_usage  {"require_usage":true}
POST /trae/api/v2/pay/user_current_entitlement_list  {"require_usage":true,"full_data":true}
```

响应含 `total_balance`、`usage_summary`、`user_entitlement_pack_list[]`。
`total_balance` 即剩余积分。

## 5. 其它端点（按需）

| 端点 | 说明 |
|-|-|
| `POST /cloudide/api/v3/trae/CheckLogin` | 登录态 + `WID`；body `{"GetAIPayHost":true,"ReqSource":"Web"}` |
| `POST /cloudide/api/v3/trae/GetUserInfo` | 用户信息；需 Cookie，body `{}` |
| `POST /cloudide/api/v3/common/GetUserToken` | Cookie 换 JWT；body 空 |
| `POST /cloudide/api/v3/trae/GetThirdPartyToken` | 第三方 token；body `{"Types":["feishu"],"ForceRefresh":true}` |
| `GET /api/remote/v1/chat_sessions?page_size=100` | 会话列表 |
| `POST /api/remote/v1/chat_sessions/{sid}/wakeup_sandbox` | 唤醒沙箱 |
| `POST /api/remote/v1/chat_sessions/{sid}/commit` | 改名/提交 `{"title":"..."}` |
| `POST /api/remote/v1/chat_sessions/{sid}/read` | 标记已读 `{}` |
| `POST /api/remote/v1/chat_sessions/{sid}/stop` | 停止 |
| `GET /api/remote/v1/frontier/access_key` | 前端密钥（用途未分析） |

## 6. 与参考实现（dsh channel-pack）的差异

| 项 | 参考实现 | Web 实测 |
|-|-|-|
| 对话主机 | `trae-api-cn.mchost.guru` | `www.trae.cn` |
| 对话路径 | `/api/agent/v3/llm_utils_chat` | `/api/remote/v1/chat_sessions*` |
| 请求体 | OpenAI 兼容 `{model,messages,tools}` | SOLO `{initial_message:{query,agent_type,custom_model,...}}` |
| 响应 | 自定义 SSE `event: output` | 自定义 SSE `event: plan_item` |
| 鉴权 | 仅 JWT | Cookie 换 JWT，JWT 用于后续 |
| 工具 | 支持 OpenAI tools | 上游自管工具，无法注入 |
| 签到 | `trae-mate` 的设备头方案 | 仅需 JWT + `{"req_source":3}` |
| Max 模式 | `max_mode` 字段 | 模型列表有 `max_mode`，请求参数未验证 |

## 7. 未验证项

1. 登录回调完整流程
2. 9004 是否等于「今日已签到」
3. Max 模式（1M 上下文）的请求参数
4. JWT 过期后的自动续期（只有 Cookie 能换新 JWT）
5. 多轮对话的工具结果回传格式（我方向上游传 assistant/tool 消息的方式）
6. 图片/文件消息的 `query` 类型与上传流程
7. 并发与限流
