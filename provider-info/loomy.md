# Loomy 客户端协议分析

> 分析来源: 解包 Electron 应用 `resources/app.asar.unpacked/electron/` 下 801 个 .js 源码文件
> 客户端版本: 0.9.39
> 分析方式: 静态代码分析 + 生产网关实测（2026-11-20）

## 0. 总结


* Loomy 是讯飞（iFlyTek.Inc）出品的 Electron 办公助手，客户端版本 0.9.39。
* **对话侧是标准 OpenAI 兼容协议**：`/chat/completions` 收发均为 OpenAI 语义，
  流式为纯 `data:` 帧，`messages`/`tools`/`tool_choice`/`reasoning_effort` 全部逐项生效。
* **登录不走 IM 渠道**，而是讯飞统一账号网关（手机号 + 短信验证码，见 §1）。
* 本协议不支持、但 Chat Completions 有的功能：无。

### 域名

| 域名 | 用途 |
|------|------|
| `https://account.xfinfr.com` | 讯飞统一账号网关（登录、发短信验证码、查用户信息） |
| `https://loomyad.xunfei.cn/api/v1` | **模型 API**（对话 / 模型目录 / 积分 / 宠物打工） |
| `https://api.loomy.ai/api/v1` | 仅客户端升级检查（`/version/check`），**不是**模型 API |
| `https://ossptest.voicecloud.cn/loomy/integration/api/v1` | 测试环境（规模同生产，登录态无效会回 `100002`） |
| `spark-api-open.xf-yun.com` | 讯飞星火 API（客户端内备用，本项目不用） |

## 1. 登录方式


客户端 UI（`LoginPage`）只有两个 Tab：**手机号** 和 **微信**。二者最终都落到
讯飞统一账号网关 `https://account.xfinfr.com`，一套 `SDK-HMAC-SHA1` 签名。

### 1.1 手机号 + 短信验证码（本项目采用）

```text
POST /login/phone/sendMsgCode   {"mobile":"<11 位手机号>"}
  → {"code":"000000","desc":"成功","data":{"msgid":"<32 hex>"}}
POST /login/phone/checkCode     {"mobile":"...","msgid":"...","code":"<验证码>"}
  → {"code":"000000","data":{"session":"<32 hex>","userid":"...","expireTime":<ms>}}
GET  /userinfo/query/baseInfo   （带 session）
  → {"code":"000000","data":{...}}   登录态有效
  → {"code":"020002","desc":"session无效"}   登录态失效
```

签名细节（`sign.js` 复刻，见 `src/providers/loomy/sign.rs`）：

* 算法 `SDK-HMAC-SHA1`，`Authorization` 头形如
  `SDK-HMAC-SHA1 Access=<AK>, SignedHeaders=<h1;h2;...>, Signature=<base64>`
* 待签串 9 行，`\n` 分隔：
  `METHOD / escapedPath / escapedQuery / Content-MD5 / Content-Type / Date / Nonce / (空) / (空)`
* 方法大写；路径与 query 按 RFC3986 转义（额外把 `! ' ( ) *` 转成 `%21 %27 %28 %29 %2A`）
* `Date` 用 RFC1123 GMT（`Fri, 20 Nov 2026 00:44:14 GMT`），`Nonce` 为 32 hex
* 参与签名的头必须同时出现在 `SignedHeaders` 与请求头里
* AK/SK 来自客户端 `.env.prod`（AES-256-GCM 加密），目前**硬编码**在
  `src/providers/loomy/account.rs`

`session` 有效期 **14 天**，无续期机制。

### 1.2 微信扫码

`/login/qr/start` → `/login/qr/wait` 轮询，成功同样换回一个 `session`。
本项目未实现（需要用户扫码，无法在服务器上自动化）。

### 1.3 IM 渠道（飞书 / 钉钉 / 企业微信 / QQ）

这些是**机器人通道**配置（`appId`/`appSecret` 等存在 `electron-store` 配置文件里），
与模型对话无关，本项目不涉及。

## 2. 令牌续期方式


**无**。讯飞账号 `session` 固定 14 天有效，过期只能重新登录
（`ofm-server login loomy`）。故 `Provider::refresh` 返回 `Unsupported`。

## 3. Chat Completions 映射


### 3.1 请求格式

```text
POST https://loomyad.xunfei.cn/api/v1/chat/completions
Authorization: Bearer <session>
token: <session>
Content-Type: application/json
```

`Authorization` 与 `token` 两者**各自单独都能生效**（实测只带 Bearer 也返回 200），
但官方客户端（`useSessionAuth: true`）两个都发，照抄最稳。

客户端还会带 `traceparent` / `ChatId` / `MsgId` / `loomy-version`。实测**都非必需**
（缺失时正常返回），本项目仍带上以对齐链路。

### 3.2 请求体

```json
{
  "model": "<modelID>",
  "messages": [{"role": "user", "content": "..."}],
  "stream": true,
  "reasoning_effort": "low",
  "tools": [{"type": "function", "function": {}}],
  "tool_choice": "required"
}
```

* `stream` 恒发 `true`：上游 SSE 分帧正常，非流式由下游聚合。
* `messages` 纯文本给字符串，含图片给内容数组
  （`{"type":"image_url","image_url":{"url":"data:image/png;base64,..."}}`）。
  **图片只对 `capabilities.input_modalities` 含 `image` 的模型有效**；
  `spark-x2.6` 无视觉能力，发图片回 HTTP 400「请求参数错误」。
* tool 消息的 `content` 是工具结果字符串，并带 `tool_call_id`。

### 3.3 模型标识

模型名直接用 `/models` 返回的 `id`（如 `spark-x2.6`、`GLM-5.3-Flash`）。
本项目对外再加 `loomy/` 前缀路由。

另有路由模型 `auto`：实测会转发到具体模型（如 `GLM-5.3-Flash`），
响应里的 `model` 是**真实模型名**而非 `auto`。

### 3.4 思考强度

目录里每个 chat 模型带
`reasoning_efforts: ["none","low","medium","high","xhigh"]` 与
`default_reasoning_effort: "low"`。上游 `reasoning_effort` 字段逐档透传生效。

**本协议不发送该字段**：请求体不带 `reasoning_effort`，保持上游默认思考行为
（即目录声明的默认档位）。

### 3.5 工具调用

四种 `tool_choice` 均**实测生效**（`none` 回文本、`required` 与指定函数均回
`finish_reason: tool_calls`），无需像 trae 那样降级。`Auto` 时不发送该字段。

### 3.6 响应格式

纯 `data:` 帧（无 `event:` 行），标准 OpenAI chunk：

```text
data: {"choices":[{"index":0,"delta":{"reasoning_content":"..."}}],...}
data: {"choices":[{"index":0,"delta":{"content":"..."}}],...}
data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_x",
        "function":{"name":"get_weather","arguments":""}}]}}]}
data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"",
        "function":{"name":"","arguments":"{\"city\":"}}]}}]}
data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],
       "usage":{"prompt_tokens":28,"completion_tokens":32,"total_tokens":60,
                "completion_tokens_details":{"reasoning_tokens":29},
                "points_consumed":1}}
data: [DONE]
```

工具调用后续分片里 `id` / `name` 是**空串**，等价于缺失，解析时不可覆盖已累积值。

`usage` 额外带 `points_consumed`（本次扣分），非 OpenAI 标准，本项目忽略。
`finish_reason` 实测出现 `stop` / `length` / `tool_calls`。

### 3.9 思考强度（上游能力参考，本协议不发送）

上游支持 `none`/`low`/`medium`/`high`/`xhigh` 五档，逐档透传生效。

**本协议不发送该字段**：请求体不带 `reasoning_effort`，保持上游默认思考行为
（即目录声明的 `default_reasoning_effort`）。

## 4. models 端点映射


### 4.1 获取模型列表

```text
GET https://loomyad.xunfei.cn/api/v1/models
Authorization: Bearer <session>
token: <session>
```

### 4.2 响应格式

```json
{
  "reasoning_enabled": true,
  "reasoning_catalog_version": 3,
  "object": "list",
  "data": [{
    "id": "spark-x2.6", "name": "Spark X2.6", "object": "model",
    "owned_by": "loomy", "created": 1790065212,
    "type": "chat", "protocol": "openai_chat",
    "context_length": 128000, "max_output_tokens": 131072,
    "reasoning_efforts": ["none","low","medium","high","xhigh"],
    "default_reasoning_effort": "low",
    "is_default_model": true, "is_default_small_model": false,
    "capabilities": {"reasoning": true, "vision": false,
                     "function_calling": true, "streaming": true,
                     "system_message": true,
                     "input_modalities": ["text"], "output_modalities": ["text"]}
  }]
}
```

实测目录（2026-11-20）：

| id | type | 视觉 | 工具 |
|----|------|------|------|
| `spark-x2.6` | chat | ✗ | ✓ |
| `DeepSeek-V4.1-Flash` | chat | ✓ | ✓ |
| `deepseek-v4-flash-1017` | chat | ✓ | ✓ |
| `GLM-5.3-Flash` | chat | ✓（含 video） | ✓ |
| `qwen3.8-flash` | chat | ✓（含 video） | ✓ |
| `auto` | chat（路由） | — | — |
| `Hy-Image-3.5-preview` | image | ✓ | ✗ |
| `doubao-seedream-5-lite` | image | ✓ | ✗ |
| `qwen-image-3.0-pro` | image | ✓ | ✗ |

**没有倍率字段**，故本项目 `ModelInfo::rate` 恒为 `None`。

### 4.3 过滤规则

* 只保留 `type == "chat"`：`type == "image"` 的文生图模型经 `/chat/completions`
  转发必然 404「该模型暂未开放，请切换其他模型」。
* `auto` 保留（它是合法 chat 路由模型，只是没有 `capabilities`）。

## 5. 余额获取


```text
GET https://loomyad.xunfei.cn/api/v1/points/balance
  → {"code":"000000","desc":"成功","data":{"balance":6000,
     "dailyBalance":4981,"availableBalance":10981}}
```

* `balance` = 永久积分
* `dailyBalance` = 当日剩余额度
* `availableBalance` = 两者之和（**对外展示用这个**）

账单明细：`GET /points/records?pageNo=1&pageSize=20`，`data.list[]` 每项含
`ledgerId/modelName/direction(credit|debit)/description/createdAt/
pointsActual/consumeSource/balanceBefore/balanceAfter/dailyCycleDate`。

其它积分端点（实测存在但本项目未用）：`/points/activation`、
`/points/first-login`、`/invitation-codes`、`/points/redemption-codes/redeem`、
`/points/by-chat`、`/points/by-turn`、`/points/records`（v2 形态）、
`/team-points/*`（需先切团队，否则回 `100001 当前未切换到团队`）。

## 6. 签到协议（宠物打工奖励）


Loomy **没有**「每日签到」字样的接口，但每日福利落在「宠物打工」这套接口上，
语义与签到等价：

```text
GET  /pet-work
  → data: {enabled, businessDate:"YYYY-MM-DD",
           config:{version:"pet-work-v1", dailyMaxPoints:1000, sampleWindowDays:28,
                   minimumActiveDays:7, sessionWeight:0.4, depthWeight:0.6,
                   tiers:[{tierId:"tier_1",score:60},{tierId:"tier_2",score:100},
                          {tierId:"tier_3",score:150}], ...},
           status:{earnedPointsToday, rewardedTierIds, dailyRewardCapped, lastGrant}}

POST /pet-work/rewards
     {"businessDate":"...","configVersion":"pet-work-v1","reachedTierId":"tier_1"}
  → data: {granted, duplicate, grantedPoints, grantedTierIds, status{...}}
```

**实测结论（2026-11-20）**：

* 服务端**不校验**本地算出的打工分数，直接申请任意合法 `tierId` 都会发分；
* 每档每天只能领一次，重复申请回 `duplicate: true` + `grantedPoints: 0`；
* 档位奖励 `tier_1/2/3 → 200/300/500`，累加受 `dailyMaxPoints`（1000）封顶，
  封顶后 `status.dailyRewardCapped = true` 且不再发分；
* **按档位升序逐个申请**才能拿满 1000；只申请 `tier_3` 只拿 500。

客户端另有 `GET /pet-work` 的本地评分逻辑（`pet-work-service.js` 用
`activeSessionCount` / `extraValidTurnCount` 相对基线算 `petWorkScore`），
但服务端不校验，本项目跳过这一步，直接按档位申请。

## 7. 上游异常响应


### 7.1 两套错误形态

| 接口 | 形态 |
|------|------|
| `/points/*`、`/pet-work*`、`/models` | 恒 HTTP 200 + `{"code","desc","data"}` 信封，`code != "000000"` 即失败 |
| `/chat/completions` 认证失效 | **HTTP 200** + `{"code":"100002","desc":"登录已失效，请重新登录","trace_id":...}`（非 SSE） |
| `/chat/completions` 参数错误 | HTTP 400 + `{"error":{"message":"请求参数错误","type":"invalid_request_error","code":400,"metadata":{"provider_name":"loomy"}}}` |
| `/chat/completions` 模型未开放 | HTTP 404 + `{"error":{"message":"该模型暂未开放，请切换其他模型","code":404}}` |

注意认证失效的 `content-type` 是 `application/json` 而非 `text/event-stream`，
故在 SSE 解码前按 content-type 拦一层。

### 7.2 错误码

| code | 含义 | 本项目归类 |
|------|------|-----------|
| `000000` | 成功 | — |
| `020002` | session 无效 | Authentication（先探测，见下） |
| `100001` | 请求参数错误 | InvalidRequest |
| `100002` | 登录已失效 | Authentication（先探测，见下） |
| `100003` / `100005` | 积分不足 / 请求过于频繁 | RateLimit |
| `210105` | 验证码错误（登录） | — |

**`100002` 存在假阳性**：高频使用（限流 / 风控）时上对话接口也会回
HTTP 200 + `{"code":"100002","desc":"登录已失效，请重新登录"}`，与真正的
登录态失效无法从响应体区分。对话接口收到该码时，再用 **只带 `token` 头**的
`GET /models` 探一次：

* 探测成功（`code == "000000"` 且有 `data`）→ 凭证本身没坏，按**限流**处理
  （下游 429 `rate_limit_error`，文案提示疑似限流、稍后重试）；
* 探测也回 `100002`/`020002`/`200003` → 确实需要重新登录（下游 401）；
* 探测失败/超时/其它码 → 无法判断，保守保持 401。

`desc` 含「缺少」的 `100002`（如只发错认证头时的「缺少 token」）是请求构造问题，
不探测，直接归为上游响应异常（502）。

## 8. 消息处理协议


### 8.1 飞书消息

```javascript
// 事件: im.message.receive_v1
{
  "message": {
    "message_id": "om_xxx",
    "chat_id": "oc_xxx",
    "message_type": "text|file|image|audio|media|post",
    "content": "{\"text\": \"hello\"}"  // JSON string
  },
  "sender": {
    "sender_type": "user|bot|app",
    "sender_id": {"open_id": "ou_xxx"}
  }
}
```

### 8.2 钉钉消息

```javascript
// 回调: TOPIC_ROBOT
{
  "msgId": "msg_xxx",
  "text": {"content": "hello"},
  "conversationId": "cid_xxx",
  "conversationType": "private|group",
  "senderStaffId": "staff_xxx",
  "senderNick": "张三",
  "msgtype": "text|audio|file",
  "content": {"downloadCode": "xxx", "fileName": "xxx"}
}
```

### 8.3 企业微信消息

```javascript
{
  "body": {
    "msgid": "msg_xxx",
    "chattype": "group|single",
    "chatid": "cid_xxx",
    "from": {"userid": "user_xxx"},
    "text": {"content": "hello"},
    "image": {"url": "xxx", "aeskey": "xxx"},
    "file": {"url": "xxx", "aeskey": "xxx"},
    "voice": {"content": "识别文本"},
    "mixed": {"msg_item": [...]},
    "quote": {"text": {"content": "引用内容"}}
  }
}
```

### 8.4 QQ 消息

```javascript
// 群消息: GROUP_AT_MESSAGE_CREATE
{
  "id": "msg_xxx",
  "content": "@bot hello",
  "group_openid": "group_xxx",
  "author": {"member_openid": "user_xxx"},
  "attachments": [{"url": "xxx", "filename": "file.txt", "content_type": "text/plain"}]
}

// 单聊: C2C_MESSAGE_CREATE
{
  "id": "msg_xxx",
  "content": "hello",
  "author": {"user_openid": "user_xxx"}
}
```

### 8.5 微信消息

```javascript
{
  "message_id": "msg_xxx",
  "from_user_id": "user_xxx",
  "to_user_id": "bot_xxx",
  "context_token": "token_xxx",
  "item_list": [
    {"type": 1, "text_item": {"text": "hello"}},
    {"type": 2, "image_item": {"media": {"encrypt_query_param": "xxx"}, "aeskey": "xxx"}},
    {"type": 4, "file_item": {"media": {"encrypt_query_param": "xxx", "aes_key": "xxx"}, "file_name": "file.txt"}}
  ]
}
```

## 9. 远程通道规则注入


Loomy 在发送消息给 AI 时，会注入通道特定的规则：

```javascript
const hiddenInstructions = buildRemoteHiddenInstructions([
  REMOTE_CHANNEL_RULES.wecom,
  ...(useFileAgent ? [FILE_ANALYSIS_AGENT_RULE, buildAttachmentPreludePrompt(attachments)] : [])
])
```

通道规则：
- **飞书**: "当前消息来源：飞书。"
- **QQ**: "当前消息来源：QQ。"
- **钉钉**: "当前消息来源：钉钉。"
- **企业微信**: 包含文件发送工具使用说明
- **微信**: "当前消息来源：微信。"
- **Nexus**: "不要调用 question 工具..."

## 10. 文件处理


### 10.1 文件下载

| 渠道 | 方式 |
|------|------|
| 飞书 | `client.im.message.getMedia()` 获取下载链接 |
| 钉钉 | `POST /v1.0/robot/messageFiles/download` 获取 downloadUrl |
| 企业微信 | `client.downloadFile(url, aesKey)` 使用 SDK |
| QQ | 直接通过附件 URL 下载，`Authorization: QQBot <token>` |
| 微信 | CDN 下载 + AES-ECB 解密 |

### 10.2 文件上传

| 渠道 | 方式 |
|------|------|
| 飞书 | `uploadImageToFeishu()` / `uploadFileToFeishu()` |
| 钉钉 | 自定义存储空间 API (`/cspace/grant_custom_space`, `/v1.0/storage/spaces/...`) |
| 企业微信 | `client.uploadMedia()` |
| QQ | `POST /v2/users/{openId}/files` base64 上传 |
| 微信 | 通过 gateway RPC |

## 11. 配置存储


所有渠道配置使用 `electron-store`，存储在用户数据目录：

```
getUserDataPath() → ~/.config/loomy (Linux) 或 %APPDATA%/loomy (Windows)
```

配置文件：
- `feishu-config.json` - 飞书配置
- `qq-config.json` - QQ 配置
- `wecom-config.json` - 企业微信配置
- `weixin-gateway/` - 微信网关数据

## 12. 主进程架构


```
main.js (入口)
├── opencode-service.js (AI 对话核心)
│   ├── model-service.js (模型管理)
│   ├── llm/llm-completion.js (一次性 completion)
│   └── session-manager.js
├── remote/remote-runtime-manager.js (IM 渠道管理)
│   ├── feishu/feishu-client.js
│   ├── feishu/feishu-service.js
│   ├── dingtalk/dingtalk-client.js
│   ├── dingtalk/dingtalk-service.js
│   ├── wecom/wecom-client.js
│   ├── wecom/wecom-service.js
│   ├── qq/qq-client.js
│   ├── qq/qq-service.js
│   ├── weixin/weixin-client.js
│   ├── weixin/weixin-gateway-bridge.js (utility 进程)
│   └── nexus/nexus-dispatch-client.js
├── skill/skill-service.js
├── mcp-service.js
└── builtin-assist-service.js
```
