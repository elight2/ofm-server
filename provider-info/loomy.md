# Loomy 客户端协议分析

> 客户端版本: 0.9.39 ｜ 分析方式: 静态读解包 JS + 生产网关实测

## 0. 总结

* 对话侧是**标准 OpenAI 兼容协议**: `/chat/completions` 收发均为 OpenAI 语义,
  流式为纯 `data:` 帧, `messages`/`tools`/`tool_choice` 逐项生效。
* 登录走讯飞统一账号网关(手机号 + 短信验证码), 不走 IM 渠道。

### 本协议不支持, 但Chat Completions有的功能

* 无。思考强度(`reasoning_effort`): 上游支持 `none/low/medium/high/xhigh` 五档并逐档生效,
  本项目不转发该字段, 保持上游默认档位。

### 本协议与官方客户端的出入

* 登录方式 → 仅实现手机号 + 短信验证码; 微信扫码需人工扫码, 不实现。
* 图片输入 → 仅对 `capabilities.input_modalities` 含 `image` 的模型有效;
  `spark-x2.6` 无视觉能力, 发图回 HTTP 400, 不做前置拦截。
* `reasoning_effort` → 官方客户端按档位发送, 本项目不发送(见上)。
* 倍率 → 上游模型目录无倍率字段, `ModelInfo::rate` 恒为 `None`。

### 域名

|域名和端点|功能|
|-|-|
|`https://account.xfinfr.com`|讯飞统一账号网关(登录、短信验证码、用户信息)|
|`https://loomyad.xunfei.cn/api/v1`|模型 API(对话 / 模型目录 / 积分 / 宠物打工)|
|`https://api.loomy.ai/api/v1`|仅客户端升级检查, 不是模型 API|
|`https://ossptest.voicecloud.cn/loomy/integration/api/v1`|测试环境(登录态无效回 `100002`)|

## 1. 登录方式

短信验证码登录, 三步:

```text
POST /login/phone/sendMsgCode   {"base":{...},"param":{"ccode":"86","phone":"...","expire":300}}
  → {"code":"000000","data":{"msgid":"<32 hex>"}}
POST /login/phone/checkCode     {"base":{...},"param":{"ccode":"86","phone":"...","mcode":"<验证码>","msgid":"...","expire":1209600}}
  → {"code":"000000","data":{"session":"<32 hex>","userid":"...","expireTime":<ms>}}
POST /userinfo/query/baseInfo   (带 session)
  → {"code":"000000",...} 有效 ｜ {"code":"020002","desc":"session无效"} 失效
```

`base` 固定为 `{appid, modelid:"Web", version:"1.0.0", devid:"web", ua:"Loomy|Desktop|Electron|macOS", traceid:<32 hex>}`。
失效码为 `020002` 与 `100002` 两种(客户端 `AUTH_ERROR_CODES`), 均需重新登录。

签名: `SDK-HMAC-SHA1`, `Authorization: SDK-HMAC-SHA1 Access=<AK>, SignedHeaders=<h1;h2;...>, Signature=<base64>`。

* 待签串 9 行, `\n` 分隔: `METHOD / escapedPath / escapedQuery / Content-MD5 / Content-Type / Date / Nonce / (空) / (空)`
* 方法大写; 路径与 query 按 RFC3986 转义, 并额外把 `! ' ( ) *` 转成 `%21 %27 %28 %29 %2A`
* `Date` 为 RFC1123 GMT, `Nonce` 为 32 hex
* 参与签名的头必须同时出现在 `SignedHeaders` 与请求头里

令牌: `session` 明文串, 有效期 **14 天**。AK/SK 自客户端 `.env.prod` 解出。

## 2. 令牌续期方式

**无**。`session` 固定 14 天有效, 过期只能重新登录。`Provider::refresh` 返回 `Unsupported`。

### 依据(客户端 0.9.39 静态核对)

账号网关的**全部** 18 个端点(`app.asar` 内 `electron/xfyun/account-service.js`)里没有任何
刷新/续期/延期接口; 会话变更只有三个入口, 全部需要用户参与:

| 入口 | 端点 | 拿到的 session |
|-|-|-|
| 短信登录 | `/login/phone/checkCode` | 新 session(`expire: 1209600`) |
| 密码登录 | `/login/account/byPwd` | 新 session(`expire: 1209600`) |
| 微信登录 | `/login/thirdAccount/bind/{auth,skip,checkCode}` | 新 session(`expire: 1209600`) |

* `expire` 只在**登录**时作为「请求多久的会话」传给上游, 不存在用旧 session 换新 session 的调用。
* 客户端里对 session 只有三种操作: 登录写入(`auth-session-controller.update`)、
  启动读回(`restore`)、收到 `100002` 后清空(`clear` + 广播 `auth:session-invalidated`)。无定时续期逻辑。
* 客户端对失效的**唯一**处理是清 session 并跳回登录页(文案「登录已过期，请重新登录」),
  佐证服务端不提供续期能力。
* 模型侧 `/api/v1` 的 59 个端点亦无 token/session 刷新接口;
  唯一带 refresh 语义的是 `athena-connect` 的 `device_token`(设备绑定凭证), 与账号 session 无关。

结论: 本项目对 loomy 只能做「失效探测 + 提示重新登录」, 不能做自动续期。

## 3. Chat Completions映射

```text
POST https://loomyad.xunfei.cn/api/v1/chat/completions
Authorization: Bearer <session>
token: <session>
Content-Type: application/json
```

`Authorization` 与 `token` 各自单独都能生效, 两者同时发最稳。
`traceparent` / `ChatId` / `MsgId` / `loomy-version` 均非必需。

请求体: `model`(目录 `id` 原值) / `messages` / `stream`(恒 `true`, 非流式由下游聚合) /
`tools` / `tool_choice`。

* 纯文本消息 `content` 为字符串; 含图片时为内容数组
  `{"type":"image_url","image_url":{"url":"data:image/png;base64,..."}}`
* tool 消息的 `content` 是工具结果字符串, 必带 `tool_call_id`
* 四种 `tool_choice` 均生效, 无需降级; `auto` 时不发送该字段

响应为纯 `data:` 帧(无 `event:` 行), 标准 OpenAI chunk:

```text
data: {"choices":[{"index":0,"delta":{"reasoning_content":"..."}}],...}
data: {"choices":[{"index":0,"delta":{"content":"..."}}],...}
data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_x","function":{"name":"get_weather","arguments":""}}]}}]}
data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"","function":{"name":"","arguments":"{\"city\":"}}]}}]}
data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":28,"completion_tokens":32,"total_tokens":60,"completion_tokens_details":{"reasoning_tokens":29},"points_consumed":1}}
data: [DONE]
```

* 工具调用后续分片的 `id` / `name` 是**空串**(等价缺失), 不可覆盖已累积值
* `usage` 额外带 `points_consumed`, 本项目忽略
* `finish_reason` 实测出现 `stop` / `length` / `tool_calls`
* 路由模型 `auto` 会转发到具体模型, 响应里的 `model` 是真实模型名

## 4. models端点映射

```text
GET https://loomyad.xunfei.cn/api/v1/models
Authorization: Bearer <session>
token: <session>
```

响应 `data[]` 每项关键字段: `id` / `type`(`chat` 或 `image`) / `protocol` /
`context_length` / `max_output_tokens` / `capabilities.{vision,function_calling,input_modalities}`。

* 只保留 `type == "chat"`: `image` 模型经 `/chat/completions` 转发必然 404
  「该模型暂未开放, 请切换其他模型」
* `auto` 保留(合法 chat 路由模型, 但没有 `capabilities`)
* 无倍率字段

## 5. 余额获取

```text
GET https://loomyad.xunfei.cn/api/v1/points/balance
  → {"code":"000000","data":{"balance":6000,"dailyBalance":4981,"availableBalance":10981}}
```

`availableBalance` = 永久积分 + 当日剩余额度, 对外展示用这个。

## 6. 签到协议

无「每日签到」接口, 每日福利落在「宠物打工」上, 语义等价:

```text
GET  /pet-work
  → data:{businessDate:"YYYY-MM-DD", config:{configVersion, dailyMaxPoints,
          tiers:[{tierId:"tier_1",score:60},...]}, status:{earnedPointsToday,...}}
POST /pet-work/rewards
     {"businessDate":"...","configVersion":"...","reachedTierId":"tier_1"}
  → data:{granted, duplicate, grantedPoints, grantedTierIds, status{...}}
```

* 服务端**不校验**本地打工分数, 直接申请任意合法 `tierId` 都会发分
* 每档每天只领一次, 重复申请回 `duplicate: true` + `grantedPoints: 0`
* 档位奖励 `tier_1/2/3 → 200/300/500`, 累计受 `dailyMaxPoints`(1000)封顶
* **必须按档位升序逐个申请**才能拿满 1000; 只申请 `tier_3` 只拿 500

## 7. 上游异常响应

| 接口 | 形态 |
|------|------|
| `/points/*`、`/pet-work*`、`/models` | 恒 HTTP 200 + `{"code","desc","data"}` 信封, `code != "000000"` 即失败 |
| `/chat/completions` 认证失效 | **HTTP 200** + `{"code":"100002","desc":"登录已失效，请重新登录"}`(非 SSE) |
| `/chat/completions` 参数错误 | HTTP 400 + `{"error":{"message":"请求参数错误","type":"invalid_request_error"}}` |
| `/chat/completions` 模型未开放 | HTTP 404 + `{"error":{"message":"该模型暂未开放，请切换其他模型"}}` |

注意认证失效的 `content-type` 是 `application/json` 而非 `text/event-stream`,
故须在 SSE 解码前按 content-type 拦一层。

错误码:

| code | 含义 | 归类 |
|------|------|------|
| `020002` | session 无效 | Authentication(先探测) |
| `100001` | 请求参数错误 | InvalidRequest |
| `100002` | 登录已失效 | Authentication(先探测) |
| `100003` / `100005` | 积分不足 / 请求过于频繁 | RateLimit |

**`100002` 存在假阳性**: 限流/风控时对话接口也回该码, 与真正的登录态失效无法从响应体区分。
对话接口收到该码时, 再用**只带 `token` 头**的 `GET /models` 探一次:

* 探测成功 → 凭证没坏, 按**限流**处理(下游 429)
* 探测也回 `100002`/`020002`/`200003` → 确实需重新登录(下游 401)
* 探测失败/超时/其它码 → 保守保持 401

`desc` 含「缺少」的 `100002` 是请求构造问题, 不探测, 直接 502。
