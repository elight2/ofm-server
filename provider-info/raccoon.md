# 商汤小浣熊(Raccoon Work)客户端协议

> 客户端版本: 桌面 1.2.2 / 移动 1.1.0 ｜ 分析方式: 静态读解包 JS(主进程 `build/electron/main/` + 渲染层 chunk) + 抓包实测

## 0. 总结

* 对话走 **标准 OpenAI Chat Completions**, 上游 `POST /api/web/llm/v2/chat/completions`。
* 身份靠请求头而非 UA(上述端点不校验 UA)。
* 登录可用 **loopback 兼容登录**(1.2.0 新增), 服务器可复用。
* 令牌可刷新: `POST /api/web/auth/v1/refresh` 存在, 但服务端对 IAM token **原样返还**, 不延长期限(见 §2)。

### 本协议不支持, 但Chat Completions有的功能

* `n` / `logprobs` / `logit_bias` / `seed` 等采样细节取决于上游网关, 官方客户端不显式传。
* 思考强度: 上游 `reasoning_effort` 的 `low`/`medium`/`high` 是**真实枚举**
  (`low` 可完全抑制思考), 其余取值被当默认。**本项目不发送该字段**, 保持上游默认。
* `tool_choice` **不**属于缺失项: 四种取值均生效, 原样透传。

### 本协议与官方客户端的出入

* 登录 → 只用 loopback 兼容登录(深链需 OS 注册协议, 服务器收不到); 短信需滑块验证码, 不实现。
* 对话发起方 → 官方由 `box-agent-acp`(Python)发起, 主进程只下发 `api_base`/`api_key`;
  服务器直接按 OpenAI 格式打上游即可。
* 本地 relay(`dist/electron/main.cjs`)是受限版本(单一 model、`tools` 必须为空), **不可照抄**。
* 思考强度 → 官方无该 UI 也不发送, 本项目同样不发送。
* 签到 → 官方无传统签到, 只有登录领取端点(见 §6)。

### 域名

|域名和端点|功能|
|-|-|
|`https://xiaohuanxiong.com`|主站与全部 API 基址|
|`/api/web/auth/v1/*`|登录、刷新、用户信息|
|`/api/iam/v1/*`|IAM 会话(`iam/confirm`、`iam/logout`、`account`), 均不下发新 token|
|`/api/web/llm/v2/chat/completions`|对话(OpenAI 兼容)|
|`/api/web/llm/v2/model_catalog`|模型目录|
|`/api/web/points/v1/balance`|积分余额|
|`/api/web/points/v2/bills`|积分流水|
|`/api/web/mobile/v1/login/points/grant`|移动端登录领每日积分(本项目签到用)|
|`/api/web/desktop/v1/login/points/grant`|桌面端登录赠送积分(需 `X-Client-Platform`)|

## 1. 登录方式

采用 **loopback 兼容登录**: 本地起一次性 HTTP 服务收回调 code, 再换 token。

```text
1. listen(0,'127.0.0.1')，回调路径 /auth/callback，state = 32 字节随机数的 64 位小写 hex
2. 浏览器打开授权页：
   https://xiaohuanxiong.com/authorize?login_source=desktop&appname=<appName>
     &desktop_callback=loopback
     &redirect=http://127.0.0.1:<port>/auth/callback
     &state=<64位小写hex>
3. 浏览器回连 127.0.0.1:<port>/auth/callback?code=xxx&state=xxx
4. POST https://xiaohuanxiong.com/api/web/auth/v1/login_with_authorization_code
   body {"authorization_code":"<code>"}
   → {code:0, data:{access_token, refresh_token, office_identity, office_org_name, office_org_role}}
```

* `desktop_callback=loopback` **必需**, 否则网页走深链。
* 回调服务端严格校验: `Host == 127.0.0.1:<port>`、`remoteAddress == 127.0.0.1`、仅 `GET`、
  `Origin` 若存在须等于授权页 origin、`Sec-Fetch-Dest` 若存在须为 `document`、
  query 只允许 `code`/`state` 各一个、`state` 须 `^[a-f0-9]{64}$` 且 timing-safe 相等、
  code 非空且 ≤4096 字节、单次消费(重复 → 409)、超时 300s。
* 授权码失效回 `code:200035`(`AUTHORIZATION_CODE_NOT_FOUND`), 且是 **HTTP 400** 而非 200 信封,
  故不能只看 HTTP 状态。

令牌: `access_token` / `refresh_token` / `office_identity`。
`office_identity` 为 `personal` 或组织码; **非 personal 时所有请求须带 `X-Org-Code`**。
实测两者是**同一 JWT**(同 `jti`、同 `exp`), 有效期 30 天。

## 2. 令牌续期方式

```text
POST https://xiaohuanxiong.com/api/web/auth/v1/refresh  body {"refresh_token":"<rt>"}
  → {code:0, data:{access_token, refresh_token}}
```

* 提前量 300s(`TOKEN_REFRESH_WINDOW_SECONDS`)。
* **响应可能原样返还同一 token**(未临近过期时 `exp`/`jti` 不变), 故**不可用"token 是否变化"
  判断续期是否成功**, 只按 `exp` 判断。
* `refresh_token` 自身过期只能重新登录。

### 客户端 1.2.2 静态核对的增量结论

客户端里 refresh 的调用点有 **3 处**, 但按 token 类型分了叉:

| 调用方 | 文件 | 对 IAM token 的处理 |
|-|-|-|
| 主进程账号模块 | `build/electron/main/client/account/accountRequests.js` | `isIamToken()` → 直接 `throw unauthorized`, **不请求** |
| schedule / box-agent | `build/electron/main/schedules/scheduleAuth.js` | 照常调 `/refresh`(无 IAM 分支) |
| 渲染层 HTTP 客户端 | `out/_next/.../96813-*.js` | `refreshBeforeExpireSeconds:300`, `refreshEndpoint:"/refresh"`, 不区分类型 |

关键差异: `accountRequests.js::tokens()` 在签发时就把 IAM token 的 `refresh_token` **丢掉**
(`isIamToken(access_token) ? {} : {refreshToken}`), `boxAgentAuthFile.js` 写盘时也同样剥除,
所以走主进程账号模块的 IAM 登录**磁盘上根本没有 `refresh_token`**。

`iam/confirm` 不是续期接口: 它在 `user_info` 回 `code:200901` 时被调一次, 用于确认
「设备/会话仍需二次确认」, 返回 `{code:0}` 但**不返回任何 token**, 会话 token 不变。

### 实测(2026-11-21, 测试凭证)

```text
当前 token: iss=xiaohuanxiong.com/iam, exp=2026-12-21 11:05:54 (剩余 30 天)
POST /api/web/auth/v1/refresh  {"refresh_token":"<同一个 JWT>"}
  → HTTP 200 {code:0, message:"success", data:{access_token, refresh_token}}
  两个返回串与请求时**完全一致**, exp 未变
```

结论: 端点对 IAM token **也不报错**, 但服务端**不会重新签发**(至少 30 天有效期内不会),
与此前实测一致。因此:

* 本项目仍应继续调 `/refresh`(将来若服务端改为真轮换, 能自动受益), 但不要指望它延长期限。
* **不可**推出「续期成功 = 有效期延长」; 只能按 `exp` 判断。
* `Provider::refresh` 返回 `Unsupported`, 不对外暴露独立刷新命令; 内部用前自动续期保留。

## 3. Chat Completions映射

```text
POST https://xiaohuanxiong.com/api/web/llm/v2/chat/completions
Content-Type: application/json
Authorization: Bearer <access_token>
X-Raccoon-Language: zh
X-Org-Code: <office_identity>          // 仅当 office_identity != "personal"
X-Client-Platform: desktop-linux       // 仅 desktop/v1 端点必需
X-Client-Version: v1.2.0
X-Client-Device-ID: <32位hex>
```

请求体: **标准 OpenAI Chat Completions 原样**。

* 模型名用目录里的 `name`(如 `sn-glm-5-3`)
* 纯文本消息 `content` 用字符串; 含图片时为内容数组 + `image_url` + `data:` URL
* 图片按**请求体字节**限制 10MB, 超限 `HTTP 413`
* 视觉能力判定: 目录 `tags` 含 `vision`(上游可能是 `image` / `image-understanding`, 需归一)
* 输出上限取目录 `params.max_tokens`
* `tools[].function.parameters` 保持**对象**(不是字符串)
* `tool_choice` 原样透传(四种取值均生效); 未指定/`"auto"` 时不发送该字段
* 非流式由下游聚合

响应为标准 OpenAI SSE: 纯 `data:` 帧(无 `event:` 行), 末尾 `data: [DONE]`。

* 首帧 `delta:{"role":"assistant"}`; 正文 `delta.content`; 思考 `delta.reasoning_content`
* 工具调用 `delta.tool_calls[{index,id,type,function:{name,arguments}}]`,
  **首帧给 `id`+`name`, 后续分片给 `id:""`/`name:""` + arguments 片段**
  (空串等同缺失, 不可覆盖首帧值)
* `finish_reason`: `stop` / `tool_calls`
* usage 独立一帧: `{prompt_tokens, completion_tokens, total_tokens,
  completion_tokens_details:{reasoning_tokens}, prompt_tokens_details:{cached_tokens}}`

⚠️ **不存在的模型名不报错**: 上游用默认模型兜底作答(HTTP 200 + 正常内容), 无法据模型名校验。

## 4. models端点映射

```text
GET https://xiaohuanxiong.com/api/web/llm/v2/model_catalog
Authorization: Bearer <token>
X-Org-Code: <org>   // 非 personal 时
```

响应:

```json
{code:0, data:{categories:[{type:"chat", default_model:"...", models:[{
  name, description, tags:[...], visible,
  params:{context_window, max_tokens},
  billing_multiplier, billing_effective_multiplier,
  billing_status:"normal"|"discount"|"limited_free", billing_status_note,
  billing_discounts:[{name,multiplier,active,start_at,end_at}]
}]}]}}
```

解析规则:

* `code` 必须为 0
* 只取 `categories[].type == "chat"`, 按 `name` 去重(保留首个)
* `visible == false` 才过滤, **字段缺失视为可见**
* `context_window` 兼容 `params.context_window` 与顶层
* `default_model` 不在列表内则退化为第一个
* 倍率取 **`billing_effective_multiplier`**(已计入折扣), 注意 `Some(0.0)`(限免)不等于 `None`
* 响应里**没有** `points_multiplier` 字段, 只有 `billing_*` 系列

## 5. 余额获取

```text
GET https://xiaohuanxiong.com/api/web/points/v1/balance
Authorization: Bearer <token>
X-Org-Code: <org>   // 非 personal 时
  → {code:0, data:{available_points, daily_points, monthly_points, reward_points,
                   topup_points, topup_frozen, ...}}
```

* 对外展示取 `available_points`
* 消耗顺序: 每日额度 → 每月额度 → 奖励积分 → 加油包
* **余额接口不带任何到期字段**, 到期信息只能从账单 `expire_at` 取

## 6. 签到协议

无传统签到端点, 用**登录领取**端点:

```text
POST https://xiaohuanxiong.com/api/web/mobile/v1/login/points/grant
X-Client-Platform: app-android
X-Client-Version: v1.1.0
(无请求体)
  → {code:0, data:{granted:bool, popup?:{points, display_name, source}}}
```

* 发放每日 300 分并把有效期延到 7 天; 另发移动端首登奖励 3000 分(每号一次)
* 服务端幂等: 同日重复调用回 `granted:false`(这是**正常跳过, 不是错误**)
* `popup.source` 枚举: `daily` / `monthly` / `old_user` / `new_user` / `desktop_login` / `mobile_login`
* 桌面端 token 可直接调移动端端点(服务的 `aud` 仅 `xiaohuanxiong.com`, 无客户端类型绑定,
  只校验 header)
* ⚠️ **版本号必须用 `v1.1.0`**(APK 版本), 不可复用桌面版 `v1.2.0`, 否则自相矛盾

⚠️ **定时刷 token / 拉余额 / 发对话都不会触发每日发放**, 只有真实客户端登录才算「登录」
(已实测)。故不能用"定时刷新"代替签到, 必须直接调领取端点。

⚠️ **账单 `created_at` 不可信**: `bill_id` 前缀为 `daily_makeup:` 的是事后补记账单,
`created_at` 恒为业务日 `00:00:00`; 只有 `bill_uuid-...` 才是真实单据。
不能用 `created_at` 推断发放规律。

## 7. 上游异常响应

通用业务信封 `{code:<int>, message:<str>, data:...}`, **HTTP 200 也可能 `code != 0`**, 必须解析业务码。

| 场景 | 判定 | 下游 |
|-|-|-|
| 授权码失效 | `code == 200035`(`AUTHORIZATION_CODE_NOT_FOUND`), HTTP 400 | InvalidRequest |
| 身份无效 | `code` ∈ `[200022(orgNotExists), 200201(userNotInOrg)]` | 切回 personal 后重试 |
| 登录失效 | HTTP `401` | 刷新 token 后重试一次 |
| 点数不足 | `1000007` | RateLimit |
| 用量超限 | `code` ∈ `USAGE_BLOCKED` 列表 | RateLimit |
| 请求体过大 | HTTP `413`(`request body exceeds 10MB`) | InvalidRequest |

⚠️ 假 code 换 token 回 **HTTP 400** 而非 200 信封, 故不能只看 HTTP 状态判断。
