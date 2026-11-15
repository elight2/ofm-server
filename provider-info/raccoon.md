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
* 官方桌面端登录靠深链 `office-raccoon://auth/callback?code=`，服务器无法接收回调；
  须自行构造 code(网页授权页回调地址写死，改不成 localhost)
* 官方对话由 `box-agent-acp`(Python)发起，主进程只负责下发 `api_base`/`api_key`/`auth.json`；
  服务器直接按 OpenAI 格式打 `/api/web/llm/v2/chat/completions` 即可
* `dist/electron/main.cjs` 的本地 relay 是受限版本(只允许单一 model、`tools` 必须为空)，**不能照抄**
* 官方**没有签到**：每日积分由服务端 `daily_grant` 自动发放，无端点

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
三种，服务器侧建议用「授权码」方式：

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

存储(官方)：`<BOX_AGENT_CONFIG_DIR>/config/auth.json`，权限 0600
```json
{"access_token":"...","refresh_token":"...","office_identity":"personal","office_org_name":"","office_org_role":""}
```
`office_identity` 为 `personal` 或组织码；**非 personal 时请求须带 `X-Org-Code`**。

## 2. 令牌续期方式
* `POST https://xiaohuanxiong.com/api/web/auth/v1/refresh`，body `{"refresh_token":"<rt>"}`
* 响应 `{code:0, data:{access_token, refresh_token?}}`
* 提前量 `TOKEN_REFRESH_WINDOW_SECONDS = 300`(access_token 约 3h 有效)
* `refresh_token` 自身也需检查 `exp`，过期只能重新登录
* IAM token 不刷新(只校验 `exp`)，过期即重新登录

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

## 5. 余额获取
* 积分余额：`GET https://xiaohuanxiong.com/api/web/points/v1/balance`
  (类 `PointsApi.baseUrl = ${origin}/api/web/points`，头带 `Authorization`，
  非 personal 加 `X-Org-Code`)
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
**无签到**。每日 300 积分由服务端自动发放(`daily_grant`)，无对应端点。
唯一与「登录奖励」相关的是 `POST /api/web/desktop/v1/login/points/grant`
(需 `X-Client-Platform: desktop-linux`、`X-Client-Version: v1.2.0`，
返回 `{code:0, data:{granted:bool, popup?:...}}`)，仅在桌面客户端登录后调用一次，
**不是每日签到**，且不是每日可重复领。
