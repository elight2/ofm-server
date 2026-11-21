# codearts 客户端协议（华为云 CodeArts Agent / 码道）

> 客户端版本: app `26.11.100`, 扩展 `huaweicloud.codearts-snap` `26.11.100`,
> kernel `126.11.1-20261110192452` ｜ 分析方式: 解包读打包 JS + 明文 curl 实测(本机 x86_64,
> 安装包 arm64, **从未运行官方客户端**, 无抓包)

## 0. 总结

* 上游**就是 OpenAI Chat Completions 兼容**: `POST {base}/api/v2/chat/completions`,
  只需做适配与裁剪, 不需转换语义。
* 官方打包里有**两套并存的代码**: IDE 扩展(`extension.js`, PromptCenter 体系)与
  agent kernel(Bun chunk, AgentCenter/inferhub 体系)。域名、端点、认证参数集**不同**,
  实现须选定一侧。本实现以 kernel + 商业版可用路径为准。
* 认证是 **AKSK(SDK-HMAC-SHA256 签名) + security_token 双轨**: 有 AK/SK 用签名,
  无 AK/SK 才用 `x-auth-token`。商业版 `x-auth-token` 实际是空串, 故**签名是主路径**。
* 登录 = OAuth2 + PKCE(SHA-256) + **DPoP**(ES256), `client_id = codearts-snap`。

### 本协议不支持, 但 Chat Completions 有的功能

* `response_format` / `logprobs` / `top_logprobs` / `n>1`: 上游无处理, 本实现不转发。
* `stop` / `seed` / `user`: 客户端会透传, 本实现不转发。
* 音频/视频输入: 无。图片输入: 有, 但**仅限多模态白名单模型**
  (CLI 默认 `Qwen3.6-27B-VL`、`Qwen3.5-397B-A17B-VL`), 否则报「未获取到可用的多模态模型」。
* `prompt_cache_*`: 无。上游以 `usage.prompt_tokens_details.cached_tokens` 返回缓存命中
  (见 §3.2), 请求侧用 `prompt_cache_key` 归组。

### 本协议与官方客户端的出入

* **传输加密不复刻**: 官方 chat 请求/响应体走 SM4-GCM(ECDH P-256 协商 + 每 chunk 独立
  加密), 由头 `X-Encrypt-Capable: sm4-gcm` 触发; 失败时**明文回退**。本实现走明文分支,
  不发 `X-Encrypt-Capable`, 实测明文 POST 正常返回 SSE。
* **思考强度不转发**: 官方档位由服务端 `POST /v1/model/thinking-variants` 按模型下发
  (`strengths`), 用户选择原样写进 `reasoning_effort`。本实现**不发送**该字段,
  保持上游默认思考行为。
* **`maas_type: benefit` 不发送**: 该头是网关路由开关(不是「声明福利模型」), 本账号
  无条件 404, 见 §3.4。
* 首帧租户冻结拦截、内容安全扫描旁路、客户端断开取消、SM4 加密链、官方定时续期节奏
  均未复刻。
* 签到不复刻 UI 面板: 直接调 `/v1/ops/{delivery,claim,confirm}`, 幂等键自生成。

### 域名

| 键(product.json) | 域名 | 用途 |
|-|-|-|
| `chatDomain` / `policyDomain` / `snapApigDomain` | `https://snap-access.cn-north-4.myhuaweicloud.com` | 主业务: `/api/v2/chat/completions`、`/v1/model/builtin`、`/snap-manager/v1/*`、`/v1/ops/*` |
| `iamStsOpenDomain` | `https://sts.cn-north-4.myhuaweicloud.com` | 登录换令牌 `/v1/oauth2/tokens` |
| `toolkitDomain` | `https://iam.myhuaweicloud.com` | IAM 凭证 |
| `codeartsWebNewUrl` | `https://codearts.huaweicloud.com` | OAuth 授权页 `/portal/authorize` |
| 备用 | `snap-access.cn-north-4.myhuaweicloud.cn` | 主域 `.cn` 变体 |

其余域名(`opengw.developer.huaweicloud.com` 福利网关、`ide.cn-north-4` 灰度、
`snapengine-apig.cn-north-7` 内网)本实现不用。

### UA / 客户端标识

不同进程不同头, **不可混用**:

| 发起进程 | 头 | 值 |
|-|-|-|
| kernel → inferhub | `x-ot-client-type` / `x-ot-client-version` / `x-ot-function` | CLI 固定 `CLI` / `126.11.1-20261110192452` / `agent-tui` |
| 华为 SDK(IAM/AKSK 签名) | `User-Agent` | `huaweicloud-usdk-nodejs/3.0`(调用方已设则**前缀拼接**) |
| extension | `User-Agent` | `DevKit-VSCode:<n>\|<r>:D1` |

## 1. 登录方式

OAuth2 + PKCE(`code_challenge_method` 原文是 **`"SHA-256"`**, 不是 `S256`) + DPoP。

```text
GET {codeartsWebNewUrl}/portal/authorize
  ?locale=<lang>&uri_scheme=codearts-snap&client_id=codearts-snap
  &port=<callback 端口>&code_challenge=<b64url(sha256(verifier))>&code_challenge_method=SHA-256
  &ticket_id=<uuid>&auth_callback_url=<encode(http://127.0.0.1:<port>/oauth/callback)>
  &plugin-name=<name>_vscode&plugin-version=<ver>&theme=<…>
```

* `CALLBACK_PATH = /oauth/callback`, 本地起 `127.0.0.1:<随机端口>`。
* verifier = 64 字节随机 → **128 个 hex 字符**。
* 回调可带 `code`(→ §1.1 换 token)或旧版 `secret`(→ ticket 端点换票, 见 §1.3)。
* kernel 版授权 URL 是 `{portal}/authorize?client_id=CodeArts_Tui&…`, **不带**
  `auth_callback_url`/`plugin-name`, 与扩展版不能混用。

### 1.1 换 token

```text
POST https://sts.cn-north-4.myhuaweicloud.com/v1/oauth2/tokens
DPoP: <ES256 dpop+jwt>
Content-Type: application/x-www-form-urlencoded
timeout: 60s

扩展: grant_type=authorization_code&client_id=codearts-snap&code=<code>
      &code_verifier=<verifier>&redirect_uri=http://127.0.0.1:<port>/oauth/callback
kernel: 同上, client_id=CodeArts_Tui, redirect_uri=<callbackUrl>
```

**DPoP**: 密钥 `ECDSA P-256`(`crv=P-256`, `kty=EC`);
header `{typ:"dpop+jwt", alg:"ES256", jwk:<公钥>}`; claims `{htm, htu, iat, jti}`,
`jti` = 64 个 hex 字符(扩展侧; kernel 侧是 32 hex)。
⚠️ **DPoP 密钥对与 PKCE verifier 必须随凭证持久化**, 续期要复用同一密钥。

响应 `data.credentials` = IAM 临时凭证:

```text
access_key_id, secret_access_key, security_token, expiration
```

另返回顶层 `refresh_token`。之后: `X-Security-Token` / `x-auth-token` = `security_token`;
签名用 `access_key_id` / `secret_access_key`。
校验: 空响应 / `status != 200` / 缺 `data.credentials` 均视为失败。

### 1.2 落盘

本实现凭证文件字段见 `config.example.toml`: AKSK、`security_token`、`refresh_token`、
PKCE verifier、DPoP 密钥对、过期时间。

### 1.3 ticket 换 token(IDE ↔ kernel 互通)

```text
GET {chatDomain}/snap-manager/v1/login/ticket?ticket_id=<…>&secret=<…>
plugin-name: <name>_vscode
plugin-version: <ver>
→ {domain_id, user_id, user_name,
   credential:{access, secret, securitytoken, expires_at}}
```

响应**不含 `refresh_token`** → 此路径登录的凭证无法续期, 只能重新登录。
本实现: 回调有 `code` 优先走 §1.1; 只有 `secret` 时才走本端点(10×1s 重试)。

## 2. 令牌续期方式

**只有一条**真实链路: `refresh_token` 重放 `POST /v1/oauth2/tokens`。
(`snap-manager/v1/token/renew` 在官方代码里只有端点声明、无任何调用点, **不应实现**。)

| 客户端 | body(`x-www-form-urlencoded`) | 额外头 |
|-|-|-|
| IDE 扩展 | `client_id=codearts-snap`、`code_verifier=<登录时 verifier>`、`grant_type=refresh_token`、`refresh_token` | `DPoP`、`Content-Type` |
| kernel | `client_id=CodeArts_Tui`、`grant_type=refresh_token`、`refresh_token`(**无** `code_verifier`) | `DPoP`、`Content-Type`、`Accept: application/json`、`x-agent-user-account: <accountId 或空串>` |

* DPoP 用**登录时同一密钥对**, `htu` = token 端点 URL, claims `{htm:"POST", htu, iat, jti}`。
* 响应处理与登录一致, 取新 AKSK 与顶层 `refresh_token`。

⚠️ **`refresh_token` 与签发它的 `client_id` 绑定, 且一次性轮换**:

* 用 `CodeArts_Tui` 携带 `codearts-snap` 签发的令牌 →
  `400 STS5.1806 invalid refresh token: 'invalid client id: codearts-snap'`
* 用同 `client_id` 携带已用过的令牌 →
  `400 STS5.1806 invalid refresh token: 'the refresh token has been used'`

故 `client_id` **绝不能跨客户端借用**。服务端 `client_id` 不匹配时**不消耗**令牌,
但一旦成功**必须立即用响应里的新 `refresh_token` 覆盖落盘**。
响应 `refresh_token` 为空串时沿用旧值。

**本实现**: 请求用 IDE 形态(`codearts-snap` + `code_verifier` + DPoP);
仅非 `invalid_grant` 类失败(5xx/网络/其它 4xx)时试一次 kernel 形态;
续期全程持 `refresh_lock` 串行; 被动续期在 `expire_at < now+60s` 触发;
`refresh` 子命令不检查是否过期, 直接续期。

官方节奏(不复刻): 提前 10 分钟续期、启动 120s 首次定时器、
`clamp( max(r/2,0), 1800000, 7200000)` 间隔调度、失败重试周期、连续 5xx 30 次 `softLogout`。

**被动失效检测**(官方): 轮询 `GET {chatDomain}/v1/sessions?client_type=…&page=1&page_size=10`,
`data.sessions[0].status !== 0` → 清凭证并退出。本实现不复刻。

## 3. Chat Completions 映射

```text
POST https://snap-access.cn-north-4.myhuaweicloud.com/api/v2/chat/completions
```

base URL 只来自 `options.baseURL` 或全局 `inferhubBaseUrl`(或 env `INFERHUB_BASE_URL`
逗号分隔取首个); 两者皆空即配置错误(`InferhubBaseUrlMissing`, statusCode 555, 不可重试)。
**`XCENTER_BASE_URLS` 不参与 chat base**, 只用于模型详情。

### 3.1 请求头

AKSK 分支(主路径):

| 头 | 值 |
|-|-|
| `X-Security-token` | `security_token`(参与签名) |
| `X-Language` | `zh` / `en` |
| `model-id` / `model-name` | 模型 id / 名称 |
| `x-ot-client-type` / `x-ot-client-version` / `x-ot-function` | CLI 固定值(见 §0 UA 表) |
| `x-ot-trace-id` / `x-ot-span-id` / `x-snap-traceid` / `x-ot-session-id` / `user-session-id` / `user-msg-id` / `created-time` | 缺失则随机 UUID 或空串 |
| `Content-Type` | `application/json` |

签名器补 `Host`、`X-Sdk-Date`、`X-Sdk-Content-Sha256`、`Authorization`。
代码**未显式设置 `Accept`** —— 不要依赖 `Accept: text/event-stream`。

token 分支: 只发 `x-auth-token` + 共有头, **不加** `X-Language`。商业版该值为空串。

### 3.2 请求/响应映射

上游即 CC 形状, 客户端只做**适配与裁剪**:

* `messages` / `tools[].function.parameters` 直通(`tool_calls`、`role:tool` 直通)。
* `max_tokens`: 仅对 `o1`/`o3`/`o4`/`gpt-5`/`gpt-6` 系列改写为 `max_completion_tokens`。
* `stream`: 本实现恒 `true`, 非流式本地聚合。
* `stream_options.include_usage`: 由 provider 配置决定, **不是** CC 字段透传。

响应是标准 CC SSE, `data: [DONE]` 结束:

* 内容分流: `reasoning_content ?? reasoning` → reasoning; `content` → text。
  上游**不解析** `<think>…</think>`(那是通用 `openai-compatible` wrapper 的行为)。
* `finish_reason`: `stop` / `length` / `content_filter` / `tool_calls`(工具轮次)。
* 工具增量按 **50 个 delta 或 4096 个 JS 字符**批处理(`text.length`, 非字节), 遇换行立即 flush。
* ⚠️ **上游把 usage 帧放在 `finish_reason` 帧之后**。流驱动器见到终止事件就停读会丢 usage;
  必须放行 `Finish` 之后的 `Usage` 事件。
* ⚠️ 上游**每帧都带 `usage: null`**, 全 0 的 usage 不得下发, 否则会盖掉后面的真值
  (流式与非流式聚合同理)。
* usage 映射:

```
prompt_tokens          = usage.prompt_tokens ?? 0
cached_tokens          = usage.prompt_tokens_details.cached_tokens ?? 0
completion_tokens      = usage.completion_tokens ?? 0
reasoning_tokens       = usage.completion_tokens_details.reasoning_tokens ?? 0
noCache prompt tokens  = prompt_tokens - cached_tokens
```

⚠️ **前缀缓存有最小长度**: 上游确实支持前缀缓存, 但实测 2512 token 前缀 → 命中 0;
2812 → 命中 2432; 3012 → 命中 2944。即可缓存前缀约需 **≥2560 token**。
同请求连发 10 次约 1~2 次 `cached_tokens: 0`(上游抖动)。
本实现按**对话前缀**派生 `prompt_cache_key`(`ofm-<sha256>`, 取首条 system + 首条 user
内容与模型名哈希), 同会话前缀不变 → key 不变。

### 3.3 排队

触发: HTTP 429 且响应头 `queue-status: queued`(`rejected` 走错误分支)。

```text
GET    {origin}/api/v1/concurrency/trace-queues/{traceId}
DELETE {origin}/api/v1/concurrency/trace-queues/{traceId}   # 取消(best-effort)
→ {status: queued|promoted|not_found, queue_length, queue_wait_time, retry_after}
```

* 轮询失败 ≥3 次放弃; `promoted` 后用原 body 重新 POST, 又 429-queued 则继续轮询。
* 队列总等待上限 12h, 超时返回不可重试的 429。
* 轮询只走 token 分支, 该分支发请求前额外加 `app-id: CodeAgent3.0`、`is-need-queue: true`。
* 本实现不发进度帧, 仅延长超时。

其他超时: 首 token 300s; 首 chunk 后改由 chunk 空闲 600s 控制(每解析到 chunk 重置)。

### 3.4 `maas_type: benefit`(不发送)

⚠️ 该头是**网关路由开关**, 不是「声明这是福利模型」: 带上它连普通模型 `GLM-5.2` 也
**404 `APIG.0101.404`**。官方正确做法是先用 `GET /api/v1/gateway/config` 的
`models[].model_id` 判定是否为福利模型, 命中后才换 chat 基址到该接口下发的 `endpoint`。

本账号 `gateway/config` 的 `result` 为 `null`(无福利模型), 故**永远不加该头、也不换 endpoint**,
12 个模型全部走 `snap-access` 网关。

## 4. models 端点映射

商业版真实可用路径:

```text
GET https://snap-access.cn-north-4.myhuaweicloud.com/v1/model/builtin
Content-Type: application/json
Agent-Type: PromptCenter
X-Language: zh-cn
X-Security-Token: <security_token>
X-Sdk-Content-Sha256: UNSIGNED-PAYLOAD     # 固定值, 不是空体 SHA256
```

`SignedHeaders` 须含 `content-type`、`agent-type`、`x-language`、`x-security-token`、
`x-sdk-content-sha256`、`host`、`x-sdk-date`。

响应 `{count, builtinModels:[{model_id, model_name, enable, credit:[{ratio, ratio_display}],
supports_images, max_tokens, context_window, …}]}`。实测 12 个模型,
**倍率取 `credit[0].ratio`**(如 `GLM-5.2` → x0.05)。

被排除的来源(不要再试):

* xCenter / agent-center(`useragents` + `detail`): 商业版 `XCENTER_BASE_URLS=[]`、
  `CODEAGENT_ID=""` 硬编码为空, 整条路径禁用。
* extension PromptCenter: 商业版 `x-auth-token` 为空串, 不可复刻。
* benefit 网关 `opengw…/api/v1/gateway/config`: 签名被接受, 但业务恒回
  `4000 invalid channel, group not found`(账号未开通权益)。
* 内网 `snapengine-apig.cn-north-7`: 仅内部环境, 公网 DNS 不解析。

多模态白名单: 模型必须命中 `hw()` 白名单才接受图片输入。

## 5. 余额获取

```text
GET {chatDomain}/snap-manager/v1/statistics/plugin
```

响应**没有** `{code,data}` 壳, 直接是对象:

```json
{"metrics":[
  {"name":"usageTotalPackageCredit","package_credit_amount":9600.00,
   "package_credit_used":2.78,"package_credit_remain":9597.22,
   "package_credit_expiring_amount":7097.22},
  {"name":"usageBasicPackageCredit",…},{"name":"usageBonusPackageCredit",…}], …}
```

余额取 `usageTotalPackageCredit` 的 `package_credit_amount − package_credit_used`
(= 9600 − 2.78 = 9597.22, 与 UI 一致)。分档: 基础包 + 赠送包。

已被证伪、勿再使用的三个来源:

| 来源 | 为什么不是余额 |
|-|-|
| `opengw…/api/v1/user/tokens/balance` | 福利模型额度渠道, 本账号恒 `4000 invalid channel` |
| `/v1/ops/credit/usage` | 单条消息/会话成本明细 |
| `claim`/`confirm` 的 `totalAmount` | 单个活动积分包的量, 非账户总额 |

## 6. 上游异常响应

### 6.1 业务错误码

| code | 含义 |
|-|-|
| `4000` | 请求参数错误 |
| `4003` | 账号被禁用 |
| `4004` | 权益不存在 / 已过期 |
| `4005` | 模型不受支持 |
| `4007` | `Provider not allowed for your group.` |
| `4291` | 配额超限 / 月度 token 超限 → 映射 **402**, message「福利模型额度已用尽…」 |
| `5000` | 服务不可用 |

### 6.2 流内错误

* 成功判定: 客户端按 `H()`(是否 AKSK 路径) 区分 —— AKSK 分支 chunk 含 `error` /
  `error_code` / `error_msg` 任一键即错误; token 分支仅看 `error` 键。
* ⚠️ chat 上游**没有** `error_code === "0000"` 校验, `0000` 只用于 benefit-service 响应。
* 首 chunk 特殊拦截(前 512 字节): 含 `TM.00020005` → 403/400 租户冻结;
  含 `ModelArts.81101`/`81111`/`81114`/`BusyException` → 上下文超长则**保留原响应**,
  否则 429。
* 内容安全 `TM.00001054` / `desensitized: true`: 官方只**弹提示**, 不裁剪、不终止流。

### 6.3 限流与重试

限流维度 `RPM`/`RPH`/`RP6H`/`RPD`/`TPM`/`TPH`/`TP6H`/`TPD`; 退避
`min(5000·2ⁿ, 60000)` 或 `min(2000·2ⁿ, 30000)`。

重试判定: `429` / `"406"` 可重试; `407` 在 `attempt < 3` 时重试;
`x-session-retry >= 3` 停止; 响应头含 `need-switch-model` → 建议换模型。
本实现额外处理 `TM.00001041`(并发会话满): HTTP 400 时每 10s 重试, 最多 180 次(30 分钟);
流内出现则不自动重连。

## 7. 签到协议（限时福利 / 每日签到）

```text
GET  {base}/v1/ops/delivery?channel=IDE            活动列表 → data.items[]
POST {base}/v1/ops/claim                           领取
POST {base}/v1/ops/confirm                         到账确认
```

`base` = `https://snap-access.cn-north-4.myhuaweicloud.com`(与 chat 同域)。

```json
claim:   {"campaignId":<id>,"channel":"IDE","idempotentKey":"claim_<id>_all_<ms>"}
confirm: {"campaignId":<id>}
→ {"code":0,"data":{"status":"CLAIMED","totalAmount":100.0,"remainingAmount":100.0,
                    "claimedAt":"…","expireAt":"…"}}
```

* `data.status` 枚举 `ELIGIBLE | CLAIMED | CONFIRMED | CONSUMED`; 非 `ELIGIBLE` 直接返回,
  不重复领取。
* 领取后须立即 `POST /v1/ops/confirm`, 再 `GET delivery` 刷新。
* ⚠️ **认证只需 AKSK 签名 + `Agent-Type: PromptCenter`**, 不需要 `x-auth-token`
  (商业版它本来就是空串)。缺 `Agent-Type` → `400 {"error_code":"TM.00001001"}`。
* ⚠️ **与 benefit-service 相反, 这里 query 必须参与签名**: `channel=IDE` 要编码进
  canonical query, 否则 `401 APIG.0301 verify ak sk signature fail`。
  不签名只用 `x-auth-token` → `401 x-auth-token not found`。
* 「100 积分」是**服务端 `displayConfig` 下发的活动配置**, 不在客户端静态资源里。
  `delivery.items[]` 另有 `type`(`INVITE_USER` / `STUDENT_CERTIFIED` …)、`claimable`、
  `displayConfig`(**JSON 字符串**)等。
* 积分**次日同一时刻过期**(`expireAt` = `claimedAt` + 24h); 次日 `claimable` 重新为 `true`。
* benefit-service 的 `/api/v1/benefit/claim` **不是签到**, 是福利模型渠道的加载开关
  (前置 `GET /api/v1/gateway/config` 有 models), 与积分/签到无关。
