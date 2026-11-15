# 商汤小浣熊(Raccoon Work)客户端协议

## 0. 总结
* 客户端版本: `1.2.0`(`@raccoon/office-desktop`，包名`商汤小浣熊-1.2.0-amd64.deb`)
* 分析方式: 解包 deb(`tmp/raccoon-extract`) + 解析 `resources/app.asar` 内 `build/`(未混淆的编译 JS) + 渲染层 webpack chunk
  (`out/_next/static/chunks/`)。**未抓包**，未反编译 `box-agent-acp`(PyInstaller)
* UA: 无固定 UA。身份靠请求头(见下)。`GET /api/web/llm/v2/model_catalog` 等端点**不校验 UA**

### 可用的现成源码
`app.asar` 里有两层可读源码，**协议逻辑全在主进程 `build/electron/main/`**：
|路径|内容|
|-|-|
|`.env.electron`|全部服务地址、各 API 前缀|
|`build/electron/main/api/config.js`|`BASE_URL`(内网测试地址，勿用)|
|`build/electron/main/account/desktopLogin.js`|桌面深链登录 + code 换 token|
|`build/electron/main/client/account/webLogin.js`|登录状态机(深链 / loopback 兼容登录)|
|`build/electron/main/client/account/webLoginLoopback.js`|本地回连登录服务(127.0.0.1 一次性)|
|`build/electron/main/schedules/scheduleAuth.js`|token 刷新(300s 提前窗口)|
|`build/electron/main/agent/boxAgentConfig.js`|把 token 写进 `auth.json`、生成 LLM 配置|
|`build/electron/main/client/models/hostedModels.js`|模型目录、内置模型预设|
|`build/electron/main/client/models/hostedModelPresentation.js`|倍率/折扣字段|
|`build/electron/main/client/account/accountRequests.js`|`/api/web/auth/v1/*` 全部端点|
|`build/src/lib/agent/localAgentModelRoutingCore.js`|模型目录响应结构校验|
|`dist/electron/main.cjs`|本地 relay(`/v1/chat/completions` → 上游)|
|`out/_next/static/chunks/*`|渲染层：积分、验证码、手机号加密|

### 本协议不支持, 但 Chat Completions 有的功能
* `n` / `logprobs` / `logit_bias` / `seed` / `stop` 之外的采样细节取决于上游网关，客户端不显式传
* 思考档位**不能**用 `reasoning_effort` 表达：服务端接受 8 个枚举值但不产生效果；
  唯一有效通道是 `extra_body.thinking.type = enabled|disabled`(两态，见参考插件实测)

### 本协议与官方客户端的出入
* 官方桌面端登录有两条可用路径(深链 / loopback)。深链需 OS 注册协议，服务器收不到；
  **loopback 兼容登录可服务器复用**(见 §1)。旧插件“回调地址写死改不成 localhost”的结论只对旧版网页成立
* 官方对话由 `box-agent-acp`(Python)发起，主进程只负责下发 `api_base`/`api_key`/`auth.json`；
  服务器直接按 OpenAI 格式打 `/api/web/llm/v2/chat/completions` 即可
* `dist/electron/main.cjs` 的本地 relay 是受限版本(只允许单一 model、`tools` 必须为空)，**不能照抄**
* 官方**没有签到端点**：每日积分 0 点服务端自动发放(`daily_grant`)，
  当天登录会把有效期从「当日 24:00」延长到 7 天(见 §7)

### 域名
|域名和端点|功能|
|-|-|
|`https://xiaohuanxiong.com`|主站与全部 API 基址|
|`/api/web/auth/v1/*`|登录、刷新、用户信息|
|`/api/web/llm/v2/chat/completions`|对话(OpenAI 兼容)|
|`/api/web/llm/v2/model_catalog`|模型目录|
|`/api/web/llm/v2/images/gen`|文生图|
|`/api/web/points/v1/balance`|积分余额|
|`/api/web/points/v1/tasks`|任务/邀请积分|
|`/api/web/points/v2/bills`|积分流水|
|`/api/web/desktop/v1/login/points/grant`|桌面登录赠送积分(需 `X-Client-Platform`)|
|`/api/web/desktop/v1/sessions/{id}/points-consumed`|会话消耗积分|
|`/api/web/office/v3/*`|Office 业务(文档/表格等，与本协议无关)|
|`/api/web/org/*`|企业版(私有化时启用)|
|`wss://collab-server.xiaohuanxiong.com`|协作(无关)|
|`https://sta-volc.xiaohuanxiong.com`|更新与 runtime 下载 CDN|

## 1. 登录方式
共四种，服务器侧可用「loopback 兼容登录」或自行取「授权码」：

1. **桌面深链**(官方主路径)
   * 浏览器打开 `https://xiaohuanxiong.com/authorize?login_source=desktop&appname=办公小浣熊客户端`
     (`NEXT_PUBLIC_LOGIN_URL=/login`，`DESKTOP_AUTH_PATH=/authorize`)
   * 登录成功后回调 `office-raccoon://auth/callback?code=xxx`，客户端取 `code`
   * `POST https://xiaohuanxiong.com/api/web/auth/v1/login_with_authorization_code`
     body `{"authorization_code": "<code>"}`
   * 响应 `{code:0, data:{access_token, refresh_token, office_identity, office_org_name, office_org_role}}`
   * 错误码 `200035` = 授权码不存在/过期/已消费
2. **短信**：`send_sms` / `login_with_sms`，手机号必须 AES-128-CFB 加密
   (key=UTF8(`senseraccoon2023`)，iv=随机16B，输出 `base64(iv‖ct)`)，并需阿里云滑块验证码
3. **IAM**(SSO)：`access_token` 为 JWT 且 `iss` 以 `/iam` 结尾时走 `iam/confirm`、`iam/logout`，
   无 `refresh_token`，只能重新登录

### loopback 兼容登录(1.2.0 新增，服务器可复用)
实现：`client/account/webLoginLoopback.js` + `webLogin.js`
1. 本地起一次性 HTTP 服务：`listen(0, '127.0.0.1')`，回调路径 `/auth/callback`
   (OAuth 模式则为 `/oauth/callback/<state>`)，`state = randomBytes(32).hex`(64 位小写 hex)
2. 浏览器打开授权页，在 `desktopLoginUrl` 基础上追加三项后再打开：
   ```
   https://xiaohuanxiong.com/authorize?login_source=desktop&appname=<appName>
     &desktop_callback=loopback
     &redirect=http://127.0.0.1:<port>/auth/callback
     &state=<64位小写hex>
   ```
   (`appname` 取自 `webLoginResources[lang].appName`；`desktop_callback=loopback` **必需**，否则网页走深链)
3. 浏览器回连 `127.0.0.1:<port>/auth/callback?code=xxx&state=xxx`
4. 校验后调 `login_with_authorization_code` 换 token

服务端严格校验(以下任一不符即拒)：
* `Host` 必须等于 `127.0.0.1:<port>`；`remoteAddress` 必须为 `127.0.0.1`
* 仅 `GET`；`Origin` 若存在必须等于授权页 origin；`Sec-Fetch-Dest` 若存在必须为 `document`
* 路径必须为 `/auth/callback`，query 只允许 `code` 和 `state` 且各仅一个，`state` 必须 `^[a-f0-9]{64}$` 且与本地值 timing-safe 相等
* `code` 非空、长度 ≤ 4096、不含控制字符；单次消费(重复→409)
* 超时 300s，`maxConnections=8`

⚠️ **实测结论(2025-11，已真实登录成功)**：loopback 登录**完全可用**。
* 授权页 URL 返回 HTTP 200，`desktop_callback` / `redirect` / `state` 均被受理
* 浏览器完成授权后**确实回连 `127.0.0.1:<port>/auth/callback?code=...&state=...`**
* 用该 code 调 `login_with_authorization_code` 成功换到 token

⇒ 服务器可直接复用此方式，**无需**手动粘 code。

存储(官方)：`<BOX_AGENT_CONFIG_DIR>/config/auth.json`，权限 0600
```json
{"access_token":"...","refresh_token":"...","office_identity":"personal","office_org_name":"","office_org_role":""}
```
`office_identity` 为 `personal` 或组织码；**非 personal 时请求须带 `X-Org-Code`**。

## 2. 令牌续期方式
* `POST https://xiaohuanxiong.com/api/web/auth/v1/refresh`，body `{"refresh_token":"<rt>"}`
* 响应 `{code:0, data:{access_token, refresh_token}}`
* 提前量 `TOKEN_REFRESH_WINDOW_SECONDS = 300`(官方源码)
* `refresh_token` 自身也需检查 `exp`，过期只能重新登录

### 实测(2025-11，个人身份，`login_with_authorization_code` 登入)
登入得到的 token 形态与官方源码假设**不同**：
* `iss` = `xiaohuanxiong.com/iam`，按官方 `isIamToken()` 判定**属 IAM token**
* `access_token` 与 `refresh_token` **字符串完全相同**(同一 `jti`、同一 `exp`)
* 有效期 **30 天**(非文档所述的“约 3h”)：`exp - iat = 2592000`
* **IAM token 可正常调 `/refresh`**：回 `code:0` 且带两个 token，
  故“IAM token 不刷新”的说法**不成立**，但响应是**原样返还同一 token**
  (未临近过期时 `exp`/`jti` 均不变)

⇒ 采用任意一个 token 作 `refresh_token` 均可续期；只需按 `exp` 判断
(提前 300s 窗口)。由于服务端可能原样返还，不要用“token 是否变化”
判断续期是否成功。

## 3. Chat Completions 协议转换
实测版本：客户端 `1.2.0`，依据 `hostedModels.js`、`boxAgentConfig.js`、`dist/electron/main.cjs`

* 上游地址：`POST https://xiaohuanxiong.com/api/web/llm/v2/chat/completions`
* 请求头(官方 `createHeaders()`，见渲染层模块 35105 + 50398)：
  ```
  Content-Type: application/json
  Authorization: Bearer <access_token>
  X-Raccoon-Language: zh
  X-Org-Code: <office_identity>        // 仅当 office_identity != "personal" 时
  X-Client-Platform: desktop-linux|desktop-macos|desktop-windows   // 仅 desktop/v1 必需
  X-Client-Version: v1.2.0
  X-Client-Device-ID: <32位hex>
  ```
* 请求体：**OpenAI Chat Completions 原样**。客户端把 `remoteBaseUrl` 设为
  `https://xiaohuanxiong.com/api/web/llm/v2`，`remote_provider=openai`，
  box-agent 自行拼 `/chat/completions`。`remote_api_key` 为**空**或占位
  `box-agent-no-auth`，真实凭据走 `auth.json` 的 `access_token`
* 模型名：直接用目录里的 `name`(如 `sn-glm-5-3`)
* 输出上限：官方默认 `XIAOHUANXIONG_MAX_OUTPUT_TOKENS`(仅 xiaohuanxiong 域名的 base 用此值，
  自定义 base 用 `USER_CONFIGURED_MAX_OUTPUT_TOKENS`)；每模型上限取目录 `params.max_tokens`
* 图片输入：模型 `tags` 含 `vision`(目录里可能是 `image`/`image-understanding`，
  客户端统一归一为 `vision`)时允许；注意按**请求体字节**限制 10MB，超限 `HTTP 413`
* 思考：`extra_body.thinking = {"type":"enabled"|"disabled"}`
* 流式：`stream:true` 走 SSE，与 OpenAI 帧一致(客户端直通转发)

## 4. 模型列表获取
* `GET https://xiaohuanxiong.com/api/web/llm/v2/model_catalog`
* 头：`Authorization: Bearer <token>`，非 personal 加 `X-Org-Code`
* 超时 10s，超时重试 1 次(仅该幂等接口)；401 时刷新 token 重试一次
* 响应结构：
  ```
  {code:0, data:{categories:[
      {type:"chat", default_model:"...", models:[
          {name, description, tags:[...], ability_level, params:{context_window, max_tokens},
           reasoning_efforts,
           points_multiplier, billing_multiplier, billing_effective_multiplier,
           billing_status:"normal"|"discount"|"limited_free", billing_status_note,
           billing_discounts:[{name,multiplier,active,daily_start,daily_end,start_at,end_at}],
           display_description, visible}
      ]},
      {type:"image"|..., ...}
  ]}}
  ```
* 解析规则(`normalizeLocalAgentRoutingCatalog`)：`code` 必须为 0；只取 `categories[].type=="chat"`；
  按 `name` 去重；`visible!=false` 才算展示；`context_window` 兼容 `params.context_window` 与顶层；
  `default_model` 不在列表内则退化为第一个

### 实测(2025-11，个人身份)
`data.categories` 只有 1 个，`type="chat"`，`models` 共 9 个：
|name|visible|备注|
|-|-|-|
|`raccoon-090dd3`(default) / `raccoon-19b265` / `raccoon-405a1c`|`false`|内部路由模型，不展示|
|`sn-sensenova-6-8-flash` / `sn-sensenova-6-8-flash-lite`|`true`|`tags` 含 `vision`|
|`sn-glm-5-3` / `sn-glm-5-3-flash`|`true`|`billing_category: lite`|
|`sn-kimi-k3` / `sn-deepseek-v4-1-flash-1017`|`true`|——|

倍率取 `billing_effective_multiplier`(已计入折扣)，如 `sn-sensenova-6-8-flash` 的
`billing_multiplier: 0.5` 但 `billing_effective_multiplier: 0`(限免到 11-30，`billing_status: limited_free`)。
模型样例：
```json
{"name":"sn-sensenova-6-8-flash","visible":true,"billing_category":"lite",
 "params":{"context_window":256000,"max_tokens":63999},
 "billing_multiplier":0.5,"billing_effective_multiplier":0,
 "billing_status":"limited_free","billing_status_note":"限免到11月30日",
 "billing_discounts":[{"name":"限免到11月30日","multiplier":0,"active":true,
   "start_at":"2026-11-11T16:00:00Z","end_at":"2026-11-30T15:59:59Z"}]}
```
注：文档早期写 `points_multiplier`，**实测响应里没有该字段**，只有 `billing_*` 系列。

## 5. 余额获取
* 积分余额：`GET https://xiaohuanxiong.com/api/web/points/v1/balance`
  (类 `PointsApi.baseUrl = ${origin}/api/web/points`，头带 `Authorization`，
  非 personal 加 `X-Org-Code`)
* 实测响应(个人身份)：
  ```json
  {"code":0,"data":{"available_points":7713,"daily_points":300,"monthly_points":0,
   "reward_points":7413,"topup_points":0,"enterprise_topup_points":0,
   "total_chargeable_points":7713,"enterprise_reserved":false,"topup_frozen":false,
   "owner_type":"users","owner_id":7657055}}
  ```
  卡片主数字取 `available_points`。
* 积分流水：`GET /api/web/points/v2/bills?paging.limit=20&type=all[&cursor=]`
* 会话消耗：`GET /api/web/desktop/v1/sessions/{sessionId}/points-consumed`
  → `{code:0, data:{total_points_consumed: number}}`
## 6. 上游异常响应
通用业务信封：`{code: <int>, message: <str>, data: ...}`，**HTTP 200 也可能 `code!=0`**，必须解析业务码。
客户端已识别的错误码：
|场景|判定|
|-|-|
|授权码失效|`code == 200035` (`AUTHORIZATION_CODE_NOT_FOUND`)|
|身份无效|`code` ∈ `[200022(orgNotExists), 200201(userNotInOrg)]` → 客户端切回 personal 后重试|
|用量超限|`code` ∈ `USAGE_BLOCKED` 列表(渲染层模块 91187 `Vs`)|
|登录失效|HTTP `401` → 刷新 token 后重试一次|
|点数不足|`1000007`(渲染层 `points` 模块)|
|模型目录超时|客户端本地错误 `HOSTED_MODEL_CATALOG_TIMEOUT`|
|请求体过大|`HTTP 413`，客户端文案 `request body exceeds 10MB`|

## 7. 签到协议
**无签到端点**。每日积分由服务端在 **每天 0 点自动发放**(账单 `biz_type: 'daily_grant'`)，
客户端无任何可调用的签到接口，故**不可**实现成签到按钮(按钮必然失败)。

### 每日积分的有效期与续期(官方文案原文)
客户端积分明细页文案 `setting.resource.points.dailyGrantRule`：

> **每日积分每天 00:00 发放，默认当日 24:00 清零，当天登录后有效期延长至 7 天。**

英/日文同义(`Daily points are granted at 00:00 each day and expire at 24:00 that day by default. Logging in that day extends their validity to 7 days.`)。

要点：
* 发放：服务端 0 点主动发，**无接口**
* 默认有效期：**当日 24:00 清零**
* 续期：**当天登录**即把该批每日积分的有效期延长至 **7 天**
  * 延期由**登录行为**(`login_with_authorization_code` / `refresh` 成功)触发，非独立接口
  * 故服务器若保持 token 长期有效、多日不重新登录，可能不触发延期
  * ⚠️ **待实测**：延期到底由「登录」还是「任何携带 token 的请求」触发，未抓包验证

### 换 token 端点实测(本机，2025-11)
`POST /api/web/auth/v1/login_with_authorization_code` 以假 code 实测，返回：
```
HTTP 400 {"code":200035,"message":"authorization_code_not_found_error","details":"authorization code not found"}
```
* 端点真实存在且受理该请求体格式 `{"authorization_code":"..."}`
* 错误码 `200035` 与源码一致；**假 code 会回 HTTP 400**（非 200 信封）
* 故客户端不能只看 HTTP 状态，需解析 body 的业务码

账单相关字段与文案(`GET /api/web/points/v2/bills`)：
|字段/类型|含义|
|-|-|
|`biz_type: 'daily_grant'`|每日积分发放；展示 `grant_at · 有效期至 <grant_at+1天> 24:00`|
|`biz_type: 'daily_expire'`|每日积分过期；`当日未登录` 表示当天没登录导致作废|
|`grant_at` / `expire_at`|发放时间 / 失效时间；二者同一天时显示「当日 24:00」|
|`event_name` / `points` / `client_type`|事件名 / 变动积分(带符号) / 消耗来源|

余额分池(`GET /api/web/points/v1/balance`)：`available_points`(总)、`daily_points`、`monthly_points`、
`reward_points`、`topup_points`、`topup_frozen`。消耗顺序：每日额度 → 每月额度 → 奖励积分 → 加油包。

唯一与「登录」相关的写接口是 `POST /api/web/desktop/v1/login/points/grant`
(需 `X-Client-Platform: desktop-linux`、`X-Client-Version: v1.2.0`，
返回 `{code:0, data:{granted:bool, popup?:...}}`)，**幂等一次性**(已领过返回 `granted:false`)，
**不是每日签到**。
