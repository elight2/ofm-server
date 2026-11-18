# Loomy 客户端协议分析

> 分析来源: 解包 Electron 应用 `resources/app.asar.unpacked/electron/` 下 801 个 .js 源码文件
> 客户端版本: 0.9.39
> 分析方式: 静态代码分析

## 0. 总结

* 客户端版本: 0.9.39
* 分析方式: 解包 Electron ASAR，静态分析源码
* 开发者: iFlyTek.Inc

### 本协议不支持, 但 Chat Completions 有的功能
* 无 (Loomy 使用 OpenAI 兼容协议)

### 本协议与官方客户端的出入
* 无 (Loomy 是客户端，不是上游 API)

### 域名

| 域名和端点 | 功能 |
|-----------|------|
| `https://api.sgroup.qq.com` | QQ 机器人 API |
| `https://sandbox.api.sgroup.qq.com` | QQ 沙箱环境 |
| `https://bots.qq.com/app/getAppAccessToken` | QQ 获取 access_token |
| `https://oapi.dingtalk.com` | 钉钉 OAPI |
| `https://api.dingtalk.com` | 钉钉开放 API |
| `https://novac2c.cdn.weixin.qq.com/c2c` | 微信 CDN 下载 |
| `https://work.weixin.qq.com/ai/qc/generate` | 企业微信生成二维码 |
| `https://work.weixin.qq.com/ai/qc/query_result` | 企业微信查询扫码结果 |
| `spark-api-open.xf-yun.com` | 讯飞星火 API |

## 1. 登录方式

Loomy 支持 6 种 IM 渠道的登录，各自独立：

### 1.1 飞书 (Feishu)
- 存储: `electron-store` 配置文件，路径 `getUserDataPath()/feishu-config.json`
- 凭证: `appId`, `appSecret`
- 认证: 使用 `@larksuiteoapi/node-sdk`，通过 `lark.Client` 初始化
- WebSocket: 使用 `lark.WSClient` 长连接接收消息
- 事件: 订阅 `im.message.receive_v1` 等事件

### 1.2 钉钉 (Dingtalk)
- 存储: `electron-store` 配置文件
- 凭证: `clientId`, `clientSecret`
- 认证: 使用 `dingtalk-stream` 包的 `DWClient`
- WebSocket: Stream 长连接，订阅 `TOPIC_ROBOT` 主题
- API: 通过 `oapi.dingtalk.com` 获取 access_token

### 1.3 企业微信 (WeCom)
- 存储: `electron-store` 配置文件
- 凭证: `botid`, `secret`
- 认证: 使用 `@wecom/aibot-node-sdk` 的 `AiBot.WSClient`
- WebSocket: 长连接，事件包括 `message.text`, `message.image`, `message.file` 等

### 1.4 QQ
- 存储: `electron-store` 配置文件
- 凭证: `appId`, `appSecret`
- 认证: 通过 `https://bots.qq.com/app/getAppAccessToken` 获取 access_token
- WebSocket: 自定义 WebSocket 客户端，使用 OpCode 协议（IDENTIFY, HEARTBEAT, RESUME 等）
- API: `https://api.sgroup.qq.com` 或沙箱环境

### 1.5 微信 (Weixin)
- 存储: 通过 `WeixinGatewayBridge` 管理的 utility 进程
- 凭证: `token`, `accountId`, `userId`, `name`
- 认证: 通过二维码扫码登录（`/login/qr/start`, `/login/qr/wait`）
- 通信: 长轮询 (Long Polling) `/getupdates` 端点
- 文件下载: CDN `https://novac2c.cdn.weixin.qq.com/c2c/download`，AES-ECB 解密

### 1.6 Nexus
- 配置: `nexus-config.js`
- 通过 `NexusDispatchClient` 连接远程网关

## 2. 令牌续期方式

| 渠道 | 续期方式 |
|------|----------|
| 飞书 | 使用 `@larksuiteoapi/node-sdk` 自动管理 token 生命周期 |
| 钉钉 | `DWClient` 自动管理，通过 `getAccessToken()` 获取 |
| 企业微信 | `@wecom/aibot-node-sdk` 自动管理 |
| QQ | 手动获取，`getAccessToken()` 检查过期（提前 60s 刷新），缓存至 `tokenExpiresAt` |
| 微信 | 长轮询 `getupdates_buf` 维持会话，token 在 `post()` 方法中携带 |
| Nexus | 由 `NexusDispatchClient` 管理 |

## 3. Chat Completions 映射

Loomy 使用 OpenCode 运行时进行 AI 对话，核心文件 `llm-completion.js`：

### 3.1 请求格式

```
POST /chat/completions
Content-Type: application/json
Authorization: Bearer <token>
token: <token>  (useSessionAuth 时)
Accept: application/json
```

### 3.2 请求体

```json
{
  "model": "<modelID>",
  "messages": [
    {"role": "system", "content": "..."},
    {"role": "user", "content": "..."}
  ],
  "temperature": 0.7,
  "max_tokens": 600,
  "stream": false,
  "enable_thinking": false,
  "chat_template_kwargs": {"enable_thinking": false}
}
```

### 3.3 模型标识

格式: `providerID/modelID`
- 解析: `parseModelId(modelId)` 返回 `{providerID, modelID}`
- 配置来源: `config.provider[providerID].models[modelID]`

### 3.4 认证方式

1. **API Key 模式**: `Authorization: Bearer <apiKey>`
2. **Session Auth 模式**: `token: <sessionToken>` 头（用于 iModel 等需要登录态的 provider）

### 3.5 思考强度

在模型配置中通过 `variants` 字段实现：
```javascript
variants: {
  low: { reasoningEffort: 'low' },
  medium: { reasoningEffort: 'medium' },
  high: { reasoningEffort: 'high' }
}
```

### 3.6 工具调用

```json
{
  "tools": [{
    "type": "function",
    "function": {
      "name": "<toolName>",
      "description": "<description>",
      "parameters": { /* JSON Schema */ }
    }
  }],
  "tool_choice": {
    "type": "function",
    "function": {"name": "<toolName>"}
  }
}
```

### 3.7 响应格式

```json
{
  "choices": [{
    "message": {
      "role": "assistant",
      "content": "<response>",
      "tool_calls": [...]
    }
  }]
}
```

## 4. models 端点映射

### 4.1 获取模型列表

```
GET /models
Authorization: Bearer <apiKey>
token: <token>  (session auth 时)
```

### 4.2 响应格式

```json
{
  "data": [
    {
      "id": "<modelId>",
      "name": "<displayName>",
      "is_default_model": true,
      "is_default_small_model": false,
      "protocol": "openai",
      "limit": {"context": 8192, "output": 4096},
      "modalities": {"input": ["text"], "output": ["text"]},
      "reasoning": true,
      "tool_call": true,
      "attachment": true
    }
  ]
}
```

### 4.3 内置 Fallback

当 `/models` 返回 404/500 时，使用内置模型列表：
- 星火 (Spark): 根据 baseURL 路径判断，`/v2` 路径使用 `spark-x` 推理模型

## 5. reasoning_effort 字段映射

Loomy 在模型配置中为所有 OpenAI-compatible 模型自动添加思考强度 variants：

| variant | reasoningEffort 值 | 说明 |
|---------|-------------------|------|
| low | `low` | 低思考强度 |
| medium | `medium` | 中思考强度 |
| high | `high` | 高思考强度 |

**实现方式**: `withThinkingVariants()` 函数在模型配置中自动注入 `variants.low/medium/high` 字段。

**前端调用**: 前端按请求透传 variant，OpenCode 在 `session.prompt` 时用 `model.variants[variant]` 深合并进 provider options。

**软关闭推理**: 对于 Qwen3 等推理模型，额外添加：
1. `enable_thinking: false` (顶层字段)
2. `chat_template_kwargs.enable_thinking: false` (vLLM/Qwen3 标准参数)
3. user 消息前缀 `/no_think` (Qwen3 软关标记)

## 6. 余额获取

Loomy 本身不直接获取余额，而是通过各 IM 渠道的凭证状态间接体现：

### 6.1 飞书
- `testConnection()` 调用 `client.auth.tenantAccessToken.internal()` 验证凭证

### 6.2 钉钉
- `testConnection()` 调用 `GET oapi.dingtalk.com/gettoken?appkey=<clientId>&appsecret=<clientSecret>`

### 6.3 QQ
- `testConnection()` 调用 `getAccessToken()` 验证凭证

### 6.4 企业微信
- 通过 `@wecom/aibot-node-sdk` 的 `authenticated` 事件确认连接状态

### 6.5 微信
- `testConnection()` 调用 gateway 的 `testConnection` RPC

## 7. 签到协议

**无**

Loomy 是 IM 机器人客户端，不涉及签到功能。

## 8. 上游异常响应

### 8.1 HTTP 错误处理

| 状态码 | 处理方式 |
|--------|----------|
| 401/403 | 返回 "鉴权失败，请检查 token/API Key 是否正确" |
| 404 | 返回 "该供应商不支持模型列表接口，请手动输入模型 ID 添加" |
| 500 | 返回 "该供应商的模型列表接口暂不可用，请手动输入模型 ID 添加" |
| 超时 | 抛出 `LlmCompletionError('请求超时', 'timeout')` |

### 8.2 错误分类

```javascript
class LlmCompletionError extends Error {
  constructor(message, code) {
    this.code = code; // 'no-model', 'bad-model-id', 'protocol-unsupported', 'no-base-url', 'no-session', 'no-api-key', 'timeout', 'http-xxx', 'network', 'no-tool-call', 'bad-arguments', 'wrong-tool'
  }
}
```

### 8.3 上游 API 错误响应

钉钉 OAPI 错误格式:
```json
{
  "errcode": 40001,
  "errmsg": "invalid credential"
}
```

QQ API 错误格式:
```json
{
  "code": 0,
  "message": "ok"
}
```

微信网关错误:
```json
{
  "ret": -14,
  "errmsg": "session expired"
}
```

## 9. 消息处理协议

### 9.1 飞书消息

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

### 9.2 钉钉消息

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

### 9.3 企业微信消息

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

### 9.4 QQ 消息

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

### 9.5 微信消息

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

## 10. 远程通道规则注入

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

## 11. 文件处理

### 11.1 文件下载

| 渠道 | 方式 |
|------|------|
| 飞书 | `client.im.message.getMedia()` 获取下载链接 |
| 钉钉 | `POST /v1.0/robot/messageFiles/download` 获取 downloadUrl |
| 企业微信 | `client.downloadFile(url, aesKey)` 使用 SDK |
| QQ | 直接通过附件 URL 下载，`Authorization: QQBot <token>` |
| 微信 | CDN 下载 + AES-ECB 解密 |

### 11.2 文件上传

| 渠道 | 方式 |
|------|------|
| 飞书 | `uploadImageToFeishu()` / `uploadFileToFeishu()` |
| 钉钉 | 自定义存储空间 API (`/cspace/grant_custom_space`, `/v1.0/storage/spaces/...`) |
| 企业微信 | `client.uploadMedia()` |
| QQ | `POST /v2/users/{openId}/files` base64 上传 |
| 微信 | 通过 gateway RPC |

## 12. 配置存储

所有渠道配置使用 `electron-store`，存储在用户数据目录：

```
getUserDataPath() → ~/.config/loomy (Linux) 或 %APPDATA%/loomy (Windows)
```

配置文件：
- `feishu-config.json` - 飞书配置
- `qq-config.json` - QQ 配置
- `wecom-config.json` - 企业微信配置
- `weixin-gateway/` - 微信网关数据

## 13. 主进程架构

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
