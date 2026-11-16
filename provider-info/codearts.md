# codearts 客户端协议（华为云 CodeArts Agent / 码道）

**来源**：`~/tools/docs/agent-clients/codearts-agent-linux-arm64-26.11.100-39f41d5.deb`
解包至 `tmp/codearts-extract/`，关键文件：

| 文件 | 大小 | 说明 |
|-|-|-|
| `resources/app/extensions/huaweicloud.codearts-snap/out/extension.js` | 10.5MB | VSCode 扩展（IDE 侧全部业务逻辑） |
| `resources/app/extensions/huaweicloud.codearts-snap/product.json` | — | 域名表 |
| `resources/app/agentkernelServer-linux-arm64` | 175MB | Bun 单文件 ELF，内嵌 `/$bunfs/root/chunk-*.js`（332 chunk），agent kernel（TUI/CLI 侧） |

kernel 内嵌 JS 已导出为 `tmp/codearts-kernel-js.txt`（262MB，grep 需 `-a`）。

**标注方式**：

| 标注 | 含义 |
|-|-|
| `[打包JS]` | 直接阅读解包后的可读 JS（附 grep 可复现的 offset） |
| `[curl]` | 我方直接构造请求实测（2026-11-16 起，用真实 AKSK 凭证） |
| `[未验证]` | 无直接证据，仅为推断 |

**本机 x86_64、安装包为 arm64，从未运行官方客户端**，故没有 `[抓包]` 证据。

## UA / 客户端标识 `[打包JS]`

不同进程发不同头，**不可混用**：

| 发起进程 | 头 | 值 |
|-|-|-|
| kernel → 内部 HTTP | `User-Agent` | `AgentKernel/1.0`（offset 102046206 附近，node `d.request` 探测本机服务） |
| kernel → inferhub | `x-ot-client-type` / `x-ot-client-version` / `x-ot-function` | CLI 模式固定 `CLI` / `126.11.1-20261110192452` / `agent-tui`；非 CLI 取调用方传入或空串 |
| 华为 SDK（IAM/AKSK 签名） | `User-Agent` | `huaweicloud-usdk-nodejs/3.0`（offset 8892390，SDK 内 `_request` 默认，若调用方已设则**前缀拼接**为 `huaweicloud-usdk-nodejs/3.0 <原值>`） |
| extension → 华为云 API | `User-Agent` | `DevKit-VSCode:<n>\|<r>:D1` |
| OTel 上报 | `User-Agent` | `OTel-OTLP-Exporter-JavaScript/0.213.0` |

## 0. 总结

* 客户端版本：app `26.11.100`（distro `d7ca751ba65ff48109f0f0a86178d1d659b4e399`），扩展 `huaweicloud.codearts-snap` `26.11.100`，kernel 内部 `VERSION="126.11.1-20261110192452"` `[打包JS]`
* 分析方式：仅静态读 JS（`grep`/`python3` 切片），未执行二进制，未抓包
* 两套代码并存：**IDE 扩展**（extension.js，PromptCenter 体系）与 **agent kernel**（Bun chunk，AgentCenter/inferhub 体系）。域名、端点、认证参数集**不同**，实现时须选定一侧，不可混用

### 本协议不支持, 但 Chat Completions 有的功能

| 功能 | 结论 |
|-|-|
| `response_format.json_schema` / `json_object` | kernel 侧有映射（`aU.transformRequestBody`）`[打包JS]`，上游是否真支持 `[未验证]` |
| `logprobs` / `top_logprobs` | 未见处理，`[未验证]` |
| `n>1` | 未见处理，`[未验证]` |
| `stop` | kernel `getArgs` 有透传 `[打包JS]` |
| `seed` | 同上 `[打包JS]` |
| `user` | 同上 `[打包JS]`，但会被 `user-msg-id` 等自有头替代语义 |
| `prompt_cache_*` | 无；上游以 `wl()` 返回 `cacheRead`/`cacheWrite` 统计（见 §3.4） |
| 音频/视频输入 | 无 |
| `--` 图片输入 | 有，但走多模态模型白名单（见 §4.4） |

### 本协议与官方客户端的出入

1. **加密**：官方 chat 请求体/响应体经 **SM4-GCM** 加密（§3.2），我方若不复刻必须依赖服务端"回退明文"分支，官方仅在异常时回退，**正常路径会失败** `[未验证]`。
2. **两套认证 URL 参数集不同**（§1.2）：IDE 版带 `port`+`code_challenge`，kernel 版带 `ticket_id`+`auth_callback_url`+`plugin-name`。
3. **两套模型列表来源不同**（§4）：kernel 走 AgentCenter `useragents→detail`，IDE 走 PromptCenter `/v1/model/builtin`。
4. **AKSK 双模式**：有 AK/SK 时用 SDK-HMAC-SHA256 签名，无 AK/SK 时用 `x-auth-token`（security token）。官方两者可能同时存在。
5. **续期有两套、签到只有一套**：续期 IDE 带 `code_verifier`、kernel 不带且加
   `x-agent-user-account`（§2.1）；端到端只有 `refresh_token` 重放一条链路，
   `snap-manager/v1/token/renew` 是死声明（§2.3）。
   签到只有 `/v1/ops/*` 一套（§7.2），与官方的差异仅是额外要求 `Agent-Type` 头。
6. **签到不复刻 UI 面板**：官方在综合栏「限时福利」面板手动点「每日领取」；server 由
   `claim` 子命令直接调 `/v1/ops/{delivery,claim,confirm}`，幂等键自生成。

---

### 域名 `[打包JS，product.json + 全量 grep]`

| 键 | 域名 | 用途 |
|-|-|-|
| `chatDomain` / `policyDomain` / `agentModeDomain` / `codeGenDomain` / `snapApigDomain` | `https://snap-access.cn-north-4.myhuaweicloud.com` | 主业务（inferhub `/api/v2/`、policy `/v1/`、snap-manager `/snap-manager/v1/`） |
| `snapApigBackupDomain` | 同上但 `.cn`（`https://snap-access.cn-north-4.myhuaweicloud.cn`） | 备用 |
| `grayUserDomain` | `https://ide.cn-north-4.myhuaweicloud.com` | 灰度 |
| `toolkitDomain` | `https://iam.myhuaweicloud.com` | IAM：`/v3/auth/tokens`、`/v3.0/OS-CREDENTIAL/securitytokens`、`/v3/auth/domains` |
| `iamStsOpenDomain` | `https://sts.cn-north-4.myhuaweicloud.com` | 登录换 token：`/v1/oauth2/tokens`、`/v5/caller-identity` |
| `codeartsWebNewUrl`（=`portalHcAuthorizeUrl`） | `https://codearts.huaweicloud.com` | OAuth 授权页 `/portal/authorize`、跳转 `/portal/login` |
| `codeartsWebUrl`（=`toolkitHcOpenWebUrl`） | `https://devcloud.cn-north-4.huaweicloud.com` | 旧门户 |
| **账户积分余额** | `https://snap-access.cn-north-4.myhuaweicloud.com` | `/snap-manager/v1/statistics/plugin`（见 §5.1） |
| 福利模型额度（内网） | `https://snapengine-apig.cn-north-7.myhuaweicloud.com` | `/api/v1/benefit/claim`、`/api/v1/user/tokens/balance`、`/api/v1/gateway/config` |
| 福利模型额度（生产商用） | `https://opengw.developer.huaweicloud.com` | 同上；选择式 `MFn() = H()&&IS_PROD!=="true" ? 内网 : 生产`（`DFn()`）。**不是余额端点** |
| **限时福利 / 每日签到** | `https://snap-access.cn-north-4.myhuaweicloud.com` | `/v1/ops/delivery`、`/v1/ops/claim`、`/v1/ops/confirm`、`/v1/ops/invite/generate`、`/v1/ops/tasks/conversation`（见 §7.2）。**与上面 benefit-service 完全无关**：workbench 用 `productService.snapEngineUrl`，扩展用 `policyCenterDomain`，两者在生产环境都指向本域名 |
| 内网兜底 | `http://snapengine.codemate.cce.prod-kwe-g.dragon.tools.huawei.com/api/v2/` | `py.inferhubBaseUrl` 默认值（offset 103554750） |
| OTel | `collector-agents-access.{cn-north-4,ap-southeast-1}.myhuaweicloud.com` | `/collector/v1/traces`、`/collector/v1/logs` |

`isCommercialVersion:true`、`isNewFramework:true`、`externalBrandName:"CodeArts Agent"`、`isSupportWebUrl:false` `[product.json]`。

## 1. 登录方式

OAuth2 + PKCE(SHA-256) + **DPoP**(ES256)。`client_id` = `QH()` = 扩展 package.json 的 `name`，
**本包实际值为 `codearts-snap`**（26.11.100；`"vscode-codebot"` 仅是 fallback 常量，见 §8-10）。

### 1.1 授权 URL（IDE 扩展版，offset 6757053 区）`[打包JS]`

官方实际拼接的是 §1.1 + §1.2 参数的**并集**（一次拼全，`openAuthorizeUrl`）：

```
GET {portalHcAuthorizeUrl}/portal/authorize
  ?theme=<vscode activeColorTheme.kind 或 空>
  &locale=<vscode env.language>
  &uri_scheme=<encodeURIComponent(QH())>          # 即 codearts-snap
  &client_id=<encodeURIComponent(QH())>
  &port=<callback 端口>
  &code_challenge=<encodeURIComponent(PKCE challenge)>
  &code_challenge_method=<PKCE.CODE_CHALLENGE_METHOD>
  &ticket_id=<crypto.randomUUID()>
  &auth_callback_url=<encodeURIComponent(callbackBase + CALLBACK_PATH)>
  &plugin-name=<Lf()>_vscode
  &plugin-version=<Fr(!1)>
```
回调地址 `${callbackBase}${CALLBACK_PATH}`，本地起 `127.0.0.1:<随机端口>`；
`CALLBACK_PATH = "/oauth/callback"`。国际站额外追加 `&quickcompfalg=1`。

`PKCE.CODE_CHALLENGE_METHOD` 常量原文为 **`"SHA-256"`**（不是 `S256`）；
`VERIFIER_LENGTH = 128`，verifier = `WordArray.random(128/2).toString(Hex)` = **128 个 hex 字符（64 字节）**。
（kernel 的 `UQ(128)` 同样是 `randomBytes(64).toString("hex")`。）

回调请求可带 `code` 或旧版 `secret`：
* `code` → §1.3 换 token；
* `secret` → §1.6 `snap-manager/v1/login/ticket` 换票（扩展 `queryAuth` 为 90×3s 轮询，kernel 为 10×1s）。

`portalHcAuthorizeUrl = productConfigs.codeartsWebNewUrl`；本包 product.json
（isCommercialVersion=true，非 HK）实际值：

| 字段 | 值 |
|-|-|
| `codeartsWebNewUrl`（授权门户） | `https://codearts.huaweicloud.com` |
| `iamStsOpenDomain`（token/userinfo） | `https://sts.cn-north-4.myhuaweicloud.com` |
| `chatDomain` / `snapApigDomain` | `https://snap-access.cn-north-4.myhuaweicloud.com` |
| `snapApigBackupDomain` | `https://snap-access.cn-north-4.myhuaweicloud.cn` |
| `toolkitDomain` | `https://iam.myhuaweicloud.com` |

授权成功后门户跳 `${portalHcAuthorizeUrl}/portal/login?login_succeed=<bool>&locale=<…>&uri_scheme=<…>`。

### 1.2 授权 URL（agent kernel 版）`[打包JS]`

```
GET {l}/authorize
  ?locale=<language>
  &client_id=CodeArts_Tui
  &port=<callback 端口>
  &code_challenge=<PKCE challenge>
  &code_challenge_method=SHA-256
  &ticket_id=<crypto.randomUUID()>
```
`l` 是 kernel 配置里的门户基址（扩展版对应 `${portalHcAuthorizeUrl}/portal`）。
kernel **不带** `auth_callback_url` / `plugin-name` / `plugin-version`，
与扩展版 §1.1 不同，不能把两套参数混为一谈。

`Bun.serve({port:0, hostname:"127.0.0.1"})`，`getCallbackUrl() = http://127.0.0.1:${port}${XQ}`。

回调处理分支：
* 无 `code`/`secret` → 返回 `OK`
* 有 `secret` → `kQ(ticketId, secret)` 换票，最多 10 次，日志 `Secret exchange failed, attempt ${X}/10`
* 有 `code` → `_Q(code, codeVerifier)`
* 成功后 `307` 跳 `${portalHcAuthorizeUrl}/login?login_succeed=<…>&locale=<language||zh-cn>`

超时：callback 等待 `KQ=300000`(5min)、整体 `yQ=600000`(10min)。超时日志 `OAuth callback timeout`。

### 1.3 换 token `[打包JS]`

```
POST https://sts.cn-north-4.myhuaweicloud.com/v1/oauth2/tokens
DPoP: <ES256 dpop+jwt>
Content-Type: application/x-www-form-urlencoded
timeout: 60s, proxy: false
```

body（`x-www-form-urlencoded`，`requestToken` 内拼装）：

| 客户端 | grant_type | 字段 |
|-|-|-|
| 扩展 | `authorization_code` | `client_id=codearts-snap`、`code`、`code_verifier=<pkce verifier>`、`grant_type`、`redirect_uri=<callbackBase>${CALLBACK_PATH}` |
| 扩展 | `refresh_token` | `client_id=codearts-snap`、`code_verifier`、`grant_type`、`refresh_token` |
| kernel | `authorization_code` | `client_id=CodeArts_Tui`、`code`、`code_verifier`、`grant_type`、`redirect_uri=<callbackUrl>` |
| kernel | `refresh_token` | `client_id=CodeArts_Tui`、`grant_type`、`refresh_token`（**不带** `code_verifier`，另带 `x-agent-user-account` 头） |

DPoP（`Kti()`/`zti()`）：
* 密钥：`crypto.subtle.generateKey({name:"ECDSA", namedCurve:"P-256"})`，`kty=EC`、`crv=P-256`
* header `{typ:"dpop+jwt", alg:"ES256", jwk:publicKeyJwk}`
* claims `{htm, htu, iat, jti}`，`jti = WordArray.random(32).toString(Hex)` = **64 个 hex 字符**
* PKCE：verifier 见 §1.1（128 个 hex 字符）；`code_challenge = base64url(sha256(verifier))`，`+→-`、`/→_`、去 `=`
* **DPoP 密钥对与 PKCE verifier 随登录上下文持久化，续期复用**（`refreshToken(e,r)` 直接读 `e.pkcePair.codeVerifier`）

代理重试：无代理失败后读 `getProxyUrl()`，用 `HttpsProxyAgent` 重试一次。日志：
`【ApiService】Token request without proxy failed: {0}, retrying with proxy: {1}` / `【ApiService】Token request failed: {0}`（置 `flag="iamLoginError"`）。超时日志 `Token request timed out, aborting request after 60s`。

响应校验（`validateTokenResponse`）：
* 空 → `Empty response from getTokenByAuthenticationCode`
* `status !== 200` → `GetTokenByAuthorizationCode failed with status: …`
* 无 `data.credentials` → `Invalid token response: missing credentials`

### 1.4 响应结构 `[打包JS]`

`data.credentials` 即 IAM 临时凭证：

```
access_key_id, secret_access_key, security_token, expiration
```

另返回 `refresh_token`。之后：
* `X-Security-token` / `x-auth-token` = `security_token`（代码里叫 `accessToken`）
* AKSK 签名用 `access_key_id` / `secret_access_key`

### 1.5 落盘 `[打包JS]`

> 下面是**官方客户端**的 secrets 结构（供对照）。本 server 的
> `providers/codearts/credential.toml` 字段不同，见
> `src/providers/codearts/credential.rs`（保存 AKSK、refresh_token、PKCE
> verifier、DPoP 密钥对）。

```toml
# 官方客户端 secrets（非本 server 格式）
type = "oauth"
access = '<credentials JSON 原样>'
refresh = "<refresh_token>"
expires = "<ISO8601>"
account_id = "<accountId>"
enterprise_url = "<enterpriseUrl>"
```
同时 `G.update({userInfo:{accessKeyId, secretAccessKey, accessToken, …}})`。
JWT `user_profile`（第 2 段 base64url，`Hri()`）可解析出用户名等展示信息。

### 1.6 ticket 换 token（IDE ↔ kernel 互通）`[打包JS]`

```
GET {chatDomain}/snap-manager/v1/login/ticket?ticket_id=<…>&secret=<…>
Content-Type: application/json;charset=UTF-8
plugin-name: <Lf()>_vscode
plugin-version: <Fr(!1)>
```
响应直接为：

```
{domain_id, user_id, user_name,
 credential: {access, secret, securitytoken, expires_at}}
```

扩展端将该响应交给 `jri()` 组装登录用户，不走 §1.3 的 OAuth token 端点；
`jri()` 不读取 `refresh_token`，该响应通常也不含它。用于 IDE 已登录时把凭证交给 kernel。

本 server 实现：`code` 优先；只有 `secret` 时按上表走 ticket 端点，
采用 kernel 的 10×1s 重试。换票成功后写入 `credential.toml`（`refresh_token` 为空，
无法用 `refresh` 子命令续期）。

---

## 2. 令牌续期方式

> 续期**只有一条**真实链路：`refresh_token` 重放 `POST /v1/oauth2/tokens`。
> 客户端不存在服务端续期接口的调用（见 §2.3）。

### 2.1 续期请求（IDE 与 kernel 两种形态）`[打包JS]`

| 客户端 | 位置 | body（`x-www-form-urlencoded`） | 额外头 |
|-|-|-|-|
| IDE 扩展 | offset 8902310 区 `refreshToken(e,r)` | `client_id=codearts-snap`、`code_verifier=<loginContext.pkcePair.codeVerifier>`、`grant_type=refresh_token`、`refresh_token` | `DPoP`、`Content-Type`（**无** `Accept`） |
| kernel | offset 106452732 `fQ()` | `client_id=CodeArts_Tui`、`grant_type=refresh_token`、`refresh_token`（**无** `code_verifier`） | `DPoP`、`Content-Type`、`Accept: application/json`、`x-agent-user-account: <auth.accountId，缺失则为空串>` |

共同点：
* DPoP proof 用**登录时同一密钥对**；`htu` = `https://sts.cn-north-4.myhuaweicloud.com/v1/oauth2/tokens`；claims `{htm:"POST", htu, iat, jti}`。
  * IDE：`jti = CryptoJS.WordArray.random(32).toString(Hex)` = 64 hex；签名 `jose`，header `{alg:ES256, typ:"dpop+jwt", jwk}`。
  * kernel：`jti = crypto.randomBytes(16).toString("hex")` = **32 hex**（与 IDE 不同）。
* 响应处理与登录完全一致（IDE 走 `validateTokenResponse`，kernel 走 `$.credentials`），取 `credentials.access_key_id/secret_access_key/security_token/expiration` 与顶层 `refresh_token`。
* kernel 只在 `Error.message` 含 `401` 时清 auth store（视为 refresh token 已吊销）；IDE 无此分支。

**实测（2026-11-16，`testdata` 令牌）**：用 IDE 形态（`codearts-snap` + `code_verifier` + DPoP）
一次成功，返回新 AKSK + 轮换后的 `refresh_token`（JWT `iat` 更新、`counter` 1→2、`family_id` 不变），
新 AKSK 可正常拉模型目录。

**关键结论：`refresh_token` 与签发它的 `client_id` 绑定，且一次性轮换。**

* 用 `CodeArts_Tui` 携带 `codearts-snap` 签发的令牌 → `400 STS5.1806 invalid refresh token: 'invalid client id: codearts-snap'`；
* 用 `codearts-snap` 携带已被用过的令牌 → `400 STS5.1806 invalid refresh token: 'the refresh token has been used'`。

因此 `client_id` **绝不能跨客户端借用**（kernel 形态只适用于 kernel 签发的令牌，且那种令牌续期后
`refresh_token` 返回空串）；服务端拒绝 `client_id` 不匹配时**不会消耗**令牌，但一旦成功就必须立即
用响应里的新 `refresh_token` 覆盖旧值并落盘。

### 2.2 续期决策与调度 `[打包JS]`

阈值：

| 客户端 | 提前量 |
|-|-|
| IDE | `renewalWindowMs = 6e5`（10 分钟） |
| kernel | `yQ = 600000`（10 分钟） |

IDE `HcLoginService.ensureTokenValid`（offset 8754119 区）分支：

| 结果 | 条件 |
|-|-|
| 直接 `true` | `now < tokenExpiredTime` |
| 直接 `false` | `isRetrying`（重试周期进行中） |
| `waitLock` | 另一窗口持 `renewalLockPath` 文件锁 → 等 30s 再判断 |
| `renew` | 过期且取到锁 → 走 §2.1 |
| `cancel` | 无 `refresh_token` 或无 `loginContext` |

IDE 定时器：
* `startRenewalTimers`：启动后 `12e4`ms 首次触发 `refreshSession()`，之后接 `scheduleRenewal`。
* `scheduleRenewal` 间隔 `kri(expiresAt, now)`：
  `r = expiresAt - now`；`n = clamp(max(r/2, 0), 1800000, 7200000)`；
  返回 `max(min(n, r - 300000), 60000)`（结果恒落在 `60000`~`7200000`ms）。
* `scheduleExpiryCheck`：间隔 `clamp(expiresAt + 1000 - now, 60000, 600000)`；重试中固定 `600000`。
* 凭证无 `expiration` 时 fallback TTL `mZe = 864e5`ms。

IDE 失败重试 `executeRetryCycle`：网络错误每 `6e4`ms 重试，非网络错误每 `6e5`ms；
连续 5xx（`500/502/503/504`）达 30 次 → `softLogout`；非网络错误且已过 `tokenExpiredTime` → `softLogout`。
网络错误判定集 `A5a`：`ECONNABORTED/ENOTFOUND/ETIMEDOUT/ECONNREFUSED/ECONNRESET/EAI_AGAIN` 或无 `response`。

kernel `mW()`（启动引导，offset 106454000 区）：
1. 环境变量 `CODEARTS_CLI_AK`+`CODEARTS_CLI_SK` → 直写 GlobalConfig（无 securityToken）。
2. 环境变量 `HW_ACCESS_KEY`+`HW_SECRET_KEY`+`HW_SECURITY_TOKEN` → 直写 GlobalConfig。
3. GlobalConfig 的 AKSK 三元组齐且 `expiresAt > now+600000` → 跳过。
4. 否则读 auth store：有效（`expires > now+600000`）→ 同步进 GlobalConfig。
5. 否则 `fQ()` 续期；失败抛 `AUTH_FAILED`。

### 2.3 服务端续期端点（仅端点表声明，无调用方）`[打包JS]`

```
{chatDomain}/snap-manager/v1/token/renew
```
（IDE 端点表字段 `_hcSessionRenewalUrl`，offset 2378069；仅声明与 getter `get hcSessionRenewalUrl()`，
**全量 grep 无任何调用点**）。kernel 包内不存在该路径字符串。响应体结构无从确认，**不应实现**。

### 2.4 IDE 命令通道 `[打包JS]`

extension `ensureTokenValid()` 在 `kr`（IDE 宿主模式）下改为执行命令
`huaweicloud.authentication.command.renewSession`，成功后再 `waitForTokenRefresh(3, 500)`
（3 次 × 500ms 等待）。命令不可用日志：
`CodeArts IDE renewSession command not available: {0}`。

### 2.5 被动失效 `[打包JS]`

| 客户端 | 端点 | 行为 |
|-|-|-|
| IDE | `GET {chatDomain}/v1/sessions?client_type=CodeArts Agent CLI&page=1&page_size=10…` | 见下 |
| kernel | `GET {chatDomain}/v1/sessions?client_type=…&page=1&page_size=10` | `data.sessions[0].status !== 0` → 清 auth → `FORCED_LOGOUT` |
| kernel | `POST {chatDomain}/v1/sessions` body `{client_type, device_name(hostname), device_type("Linux"), ip_address(首个非内网 IPv4), client_version}` | 创建会话，失败仅 log |

kernel 轮询 `n = 120000`ms（120s）；`T()` 另有限流：仅当带 `CODEARTS_CLI_AK/SK` 或
`HW_ACCESS_KEY/SECRET_KEY/SECURITY_TOKEN` 时按 `xQ = 3600000`ms（1h）节流重建会话。
检测失败 → `LQ()` 打印/派发 `FORCED_LOGOUT` 并 `process.exit(0)`。

### 2.6 server 实现状态（2026-11-16）

`src/providers/codearts/refresh.rs` + `mod.rs`：

| 项 | 做法 |
|-|-|
| 请求 | IDE 形态（`client_id=codearts-snap` + `code_verifier` + `DPoP` + `Accept`） |
| 回退 | 仅在非 `invalid_grant` 类失败（5xx/网络/其它 4xx）时试一次 kernel 形态（`CodeArts_Tui`，不带 `code_verifier`） |
| 轮换 | 响应 `refresh_token` 为空时沿用旧值；否则覆盖落盘 |
| 串行 | `refresh_lock` 保护，避免 `refresh_token` 轮换冲突 |
| 被动 | `valid_credential()`：`expire_at < now+60s` 即续期（比官方 10min 更保守） |
| 主动 | `refresh` 子命令不检查是否过期，直接续期 |
| 错误 | 401 / `invalid_grant` / `invalid_token` / 包含 `expired` → `Unauthorized`（提示重新登录）；其余保留上游报文 |
| 未做 | IDE 的 120s 首次定时器与 `scheduleRenewal` 节奏（被动续期在请求前按需触发）、失败重试周期、`softLogout`、代理重试 |

实测：`ofm-server refresh codearts` 对 `testdata` 令牌成功；新凭证 `status` 显示 12 个模型。

---

---

## 3. Chat Completions 映射

上游为 **OpenAI Chat Completions 兼容**，chat provider（`IP()` → `aU`）端点：

```
POST {base}/chat/completions
```

当前生产 base 为 `https://snap-access.cn-north-4.myhuaweicloud.com/api/v2`，即实际
`https://snap-access.cn-north-4.myhuaweicloud.com/api/v2/chat/completions` `[打包JS]`。
旧框架另有 `chatUrl = /v1/chat/chat`（`construtNewFrameWorkAllUrl`，offset 2375900），新框架用 `/api/v2/chat/completions`。

**base URL 解析（更正）**：chat provider 只取 `options.baseURL` 或全局
`W.get().inferhubBaseUrl`；两者都为空才抛 `InferhubBaseUrlMissing`
（`error_code: "CodeAgent.Config.InferhubBaseUrlMissing"`、`statusCode: 555`、
`isRetryable: false`，offset 150700745 区）。
`XCENTER_BASE_URLS` **不参与 chat base**，只用于 AgentCenter/模型详情（§4.1）。
`INFERHUB_BASE_URL`（逗号分隔取首个）是全局配置/启动初始化的来源之一。
`Am()`/`oZ()`（offset 151466780）是 SnapEngine 的解析器：显式值 →
`globalThis.__codeagentGlobalConfig.inferhubBaseUrl` → `INFERHUB_BASE_URL`，并把 URL
截成 `origin` 后拼 `/api/v2/`；不要把它当成 chat provider 的 base 解析。

### 3.1 请求头 `[打包JS]`

`getInferhubHcHeaders(n, u)`（offset 150672502）只在 `H()`（商业版/AKSK 路径）为真时
走 AKSK 分支；否则走 `t.userToken` 的 `x-auth-token` 分支。

**HTTP 层补充**：`P6` 设置 `Content-Type: application/json`；代码**未显式设置**
`Accept` 头（不要依赖 `Accept: text/event-stream`）。`getInferhubHcHeaders` 只返回业务头。

**两模式共有的业务头**：

| 头 | 值 |
|-|-|
| `x-ot-client-type` | CLI 固定 `CLI`；否则 `u["x-ot-client-type"] \|\| t.clientType \|\| ""` |
| `x-ot-client-version` | CLI 固定 `126.11.1-20261110192452`；否则取入参 |
| `x-ot-function` | CLI 固定 `agent-tui`；否则 `u["x-ot-function"] ?? ""` |
| `x-codearts-doer-scenario-type` | 仅当 `u["x-codearts-doer-scenario-type"]` 非空才加 |
| 透传 | `plugin-name`、`plugin-version`、`ide-name`、`ide-version`（仅当入参存在） |
| trace | `x-ot-trace-id`、`x-ot-span-id`、`x-snap-traceid`、`x-ot-session-id`、`x-ot-parent-session-id`、`user-session-id`、`user-msg-id`、`created-time`；缺失时随机 UUID 或空串 |
| 模型 | `model-id`；`model-name` 在 AKSK 分支用 `VP(...)` 归一，token 分支原样 |
| `maas_type` | 仅当入参提供 |

**AKSK 分支追加**：

| 头 | 值 |
|-|-|
| `X-Security-token` | `userInfo.accessToken`（进入签名头） |
| `X-Language` | `me()`：`zh` / `en` |
| `X-Remote-Origin-Url` | 入参存在才加 |
| `client-ip` | `LP()` |
| `model-name` | `VP((Xu.getStore()?.modelName \|\| u["model-name"]) ?? "")` |

然后 `G6.sign(req, new V6(ak, sk))` 生成 SDK-HMAC-SHA256 签名；签名器补
`Content-Type`、`X-Sdk-Content-Sha256`、`Host`、`X-Sdk-Date`、`Authorization`。

**token 分支**：只返回 `x-auth-token` + 上表共有头；**不加** `X-Language`、
`client-ip`、`X-Remote-Origin-Url`，也不做 `model-name` 的 `VP` 归一。

调试日志：`[header-debug] _existingHeaders=…, alsModelName=…, finalModelName=…`、
`[header-debug] hc headers model-id=…, model-name=…`。

### 3.2 传输加密（SM4-GCM，官方客户端路径）`[打包JS]`

> **server 实现现状（2026-11-16）**：`src/providers/codearts/chat.rs` 走明文分支，
> 不发送 `X-Encrypt-Capable`、不实现 SM4-GCM；实测明文 POST 能正常返回 SSE。
> 本节保留官方加密链路，供后续上游强制加密时补全。

配置 `S1`（offset 151428000 区）：

```js
{ enabled:true,
  sm4:{ enabled:true, keyExchangePath:"/api/v1/security/key-exchange",
        keyRefreshInterval:3600, fallbackOnFailure:true },
  certPin:{ enabled:false, pins:[] } }
```
可由 env `TRANSPORT_SECURITY_CONFIG` 覆盖（`pN()`）。算法标识 `LN = "sm4-gcm"`。
加密路径白名单：`/api/v2/chat/completions`（`KLn`，`startsWith` 匹配）；
key-exchange 路径自身不加密（`zLn`）。算法 `LN="sm4-gcm"`，加密请求体走
`application/octet-stream`。

传输安全 hook 还会在 chat 请求头上加 `X-Encrypt-Capable: sm4-gcm`（`RN`，
`[hn.ChatHeaders]` 回调），服务端据此决定是否返回加密响应。

**密钥协商**：

```
POST {base}/api/v1/security/key-exchange    （ECDH P-256，crypto.createECDH）
请求头：优先 b_(url,"POST") 的 AKSK 签名头；若有 userToken 再加 x-auth-token
请求 body: { clientPublicKey:<base64>, sessionId:<16B随机→hex>, timestamp:<Date.now()> }
           clientPublicKey = createECDH("prime256v1").generateKeys().toString("base64")
→ 响应: { serverPublicKey:<base64 DER 裸，非 PEM/JWK>, keyId, expiresIn }
           服务端公钥直接 Buffer.from(serverPublicKey,"base64") 喂 computeSecret
shared = ECDH(clientPriv, serverPub)
key    = sha256(shared) 前 16 字节          （FLn(n)，IV 长度常量 FN=12）
```

服务端 `expiresIn` 存在时按 `expiresIn*1000` 作为 key TTL，否则用
`keyRefreshInterval*1000`（默认 3600s）；提前 `TLn=300000`ms 触发重新协商。

**请求报文格式**（iv/tag/ts 走 header，**不在 body 内联**）：
* `AAD = ${method}|${pathname}|${ts}`（`MLn`；method 默认 `POST`，pathname 不含 query）
* body = base64(密文)，密文为 SM4-GCM 输出（不含 tag）
* headers 追加：`X-Encrypt-Algo: sm4-gcm`、`X-Encrypt-Key-Id: <keyId>`、
  `X-Encrypt-Iv: base64(iv)`、`X-Encrypt-Tag: base64(tag)`、`X-Encrypt-Ts: <Date.now()>`、
  `Content-Type: application/octet-stream`
* 加密后须对 AKSK 头**重新签名**（`b_(url, method, "application/octet-stream")`，跳过 `host`）；
  无 AKSK 则保留原 `Authorization`

**响应报文格式**：
* 响应侧 `aadPrefix = keyId`（`new s5(m.key, m.keyId)`），逐行解密
* `AAD_chunk = ${aadPrefix}|${chunkIndex}` = `${keyId}|${chunkIndex}`，计数从 0 开始
* 每个 chunk 文本以 `:` 分 3 段：`iv:tag:ciphertext`，均 base64；IV 12B、key 16B、tag 独立
* 解密后原样补回换行，删除 `X-Encrypt-Algo` / `X-Encrypt-Key-Id`，设置
  `Content-Type: text/event-stream`
* 判定是否加密响应：`X-Encrypt-Algo === "sm4-gcm"`，否则明文透传

**回退**：任何失败转明文，日志：
`Encrypting fetch bypass (URL not in encrypt paths), sending plaintext`、
`Encrypted request failed (network error), falling back to plaintext`、
`Encrypted request failed (non-2xx), falling back to plaintext`；
`ENCRYPT_KEY_EXPIRED` → 清除 key、重新协商后重试一次。
`fallbackOnFailure:true` 时，协商失败但旧 key 未过期则沿用旧 key；否则抛错并由上层回退明文。
日志：`Session key negotiated`（含 `keyId`、`ttlSec`）、`Session key negotiation failed`、
`Falling back to existing key`、`Session key cleared`。

### 3.3 请求映射（Chat Completions → 上游）`[打包JS]`

上游即 CC 形状，主要做**适配与裁剪**（`aU.getArgs`，offset 150678000 区）：

| 下游字段 | 上游处理 |
|-|-|
| `messages` | 直通；`tool_calls` / `tool` role 直通 |
| `tools[].function.parameters` | 直通（JSON Schema 对象） |
| `max_tokens` | 仅 `cU(model)`：`o1`/`o3`/`o4`/`gpt-5`/`gpt-6` 系列 → 重命名为 `max_completion_tokens`（`BP()`），日志 `[openai-reasoning-model] adaptMaxTokensParam applied` |
| `reasoning_effort` | 见 §3.3.1。**客户端不校验、不改写**非 OpenAI 模型的值，原样透传 |
| `response_format` | 仅当 AI SDK `responseFormat.type === "json"`：有 schema 且 `supportsStructuredOutputs` → `json_schema`；否则 `json_object`；`text` 不发该字段 |
| `strictJsonSchema` | 默认 `true`，用作 `json_schema.strict`（不是顶层字段） |
| `user` / `stop` / `seed` / `verbosity` | `user`、`stop`、`seed` 透传；`verbosity` 取 `providerOptions.textVerbosity` |
| `stream` | `doStream` 传 `true`，`doGenerate` 传 `false` |
| `stream_options.include_usage` | 由 inferhub provider 配置 `includeUsage` 决定，**不是**下游 CC 字段透传 |

`openai-compatible` 模式有弃用警告。`transformRequestBody` 两个调用点分别是 `{...u, stream:false}` 与 `{...u, stream:true}`。

#### 3.3.1 `reasoning_effort` 的实际处理 `[打包JS]`

模型名判定（`D6` = `openai-reasoning-model` 服务，offset 99395952 区）：

```js
function l1(n,u){ return new RegExp(`^${u}([.\\-]|$)`).test(n) }
const $Vn = ["gpt-6-astra","gpt-6.1-sol"];
function fU(n){                       // 是否走 OpenAI 推理改写
  let u=n.toLowerCase();
  if(l1(u,"gpt-5")) return true;
  if(l1(u,"gpt-6")) return !$Vn.some(t=>l1(u,t));
  return false;
}
function AP(b){                       // 唯一改写：带 tools 的 OpenAI 推理模型强制关思考
  if(typeof b.model!=="string"||!fU(b.model)) return b;   // ← 非 gpt-5/gpt-6 直接返回
  if(!Array.isArray(b.tools)||b.tools.length===0) return b;
  if(b.reasoning_effort==="none") return b;
  b.reasoning_effort = "none";        // 日志 adaptReasoningEffort applied
  return b;
}
```

结论：

* **只有** `gpt-5` 全系、`gpt-6`（除 `gpt-6-astra`/`gpt-6.1-sol`）在 `tools` 非空时被强改为 `"none"`。
* `glm` / `sn-*` / 其它模型**不匹配 `fU`**，`reasoning_effort` 原样透传给上游，客户端不做合法性校验。
* 改写在 request body 层（`AP(body)`），只影响该次请求；`stream`/非流式两条路径共用。

### 3.4 响应映射（上游 SSE → CC chunk）`[打包JS]`

SSE 逐帧 `data: {...}`，`data: [DONE]` 结束；`xl(chunkSchema)` 解析，chunk 形状是
标准 OpenAI CC（`choices[0].delta` / `finish_reason` / `usage`）或错误对象。

**成功判定（更正）**：chat 上游**没有** `error_code === "0000"` 校验；`0000` 只用于
benefit-service 响应（§5/§7）。`aU.doStream` 的判定是：

* `H()` 为真：chunk 含 `error` / `error_code` / `error_msg` 任一键即按错误处理；
* `H()` 为假：仅 `error` 键按错误处理。

**内容分流**：`reasoning_content ?? reasoning` → reasoning；`content` → text。
`aU`（inferhub provider）**不解析** `<think>…</think>`；`gP()` 的 `<think>` 分流只属于
通用 `openai-compatible` provider wrapper，不要套到 chat 上游。

**finish_reason**（`rU`）：`stop`→`stop`、`length`→`length`、
`content_filter`→`content-filter`、`function_call`/`tool_calls`→`tool-calls`，其他→`other`。

**工具增量**：按 `tool-input-delta` 批处理，阈值 50 个 delta / 4096 个 JS 字符
（`text.length`，不是字节）；遇换行立即 flush。文本/推理 delta 在 agent-team
非 leader 会话中也会走同样的 50/4096 批处理。

**收尾**：`finish_reason === "length"` 时会额外插入一条 `text-delta` 提示
（`nU()`，输出 token 耗尽）；若只有 reasoning、没有 text/tool call，会转成可重试的
空响应错误（最多 3 次）。

usage（`wl(n)`）：

```js
inputTokens.total   = prompt_tokens ?? 0
inputTokens.noCache = total - (prompt_tokens_details.cached_tokens ?? 0)
inputTokens.cacheRead = cached_tokens ?? 0
inputTokens.cacheWrite = undefined
outputTokens.total  = completion_tokens ?? 0
outputTokens.reasoning = completion_tokens_details.reasoning_tokens ?? 0
outputTokens.text   = total - reasoning
raw = 原始 usage
```

### 3.5 排队（queue）`[打包JS]`

上游可返回排队态，三套队列机制并存。**注意**：trace 队列轮询只走
`H()` 为假的 token 分支；该分支发请求前会额外加 `app-id: CodeAgent3.0`、
`is-need-queue: true`。触发条件是 429 且 `queue-status: queued`；
`queue-status: rejected` 则走拒绝/错误分支。

1. **响应内 queueInfo**（`QueueRetryError`，`statusCode:429`、`isRetryable:true`）
   字段 `queue_length`、`queue_wait_time`、`retry_after`（秒，默认 5）；
   队列总等待上限 `uU = 43200000`ms（12h），超时返回 429 不可重试。
2. **trace 队列轮询**
   ```
   GET  {origin}/api/v1/concurrency/trace-queues/{traceId}
   DELETE {origin}/api/v1/concurrency/trace-queues/{traceId}   # 取消，best-effort
   ```
   `origin` 取 chat URL 的 `new URL(q).origin`。响应
   `{status, queue_length, queue_wait_time, retry_after}`；`status` ∈ `queued` / `promoted` / `not_found`。
   轮询失败计数 ≥3 放弃；`promoted` 后用原 body 重新 POST，若又返回 429-queued 则继续轮询。
   日志 `poll queue GET failed (attempt N)`、`queue delete sent for traceId=…`
3. **IDE 侧轮询**（extension.js，offset 10106446 区）
   ```
   GET {chatDomain}/api/v1/queue/status?task_id=<…>&model=<…>
   ```
   headers `X-Language: zh-cn`；解构 `{queue_position, httpStatusCode, status, message, buttons, links, error_msg}`；
   `MAX_POLLING_ERROR_COUNT=3`；`TM.00001045` → 弹错误提示；
   `pendingPollingResolvers[sessionId]` 注册/结算，`clearAllQueue()` 批量 DELETE。
   日志 `[Queue] polling registered, sessionId: {0}, pending: {1}`、`[Queue] start Polling params`、`[Queue] batch clear Polling params`

排队状态需向下游体现为等待（我方建议：不发 chunk，仅延长超时；`[未验证]` 官方是否发 notice 帧）。

### 3.6 首帧/首 token 超时与租户冻结 `[打包JS]`

* 首 token 超时 `w=300000`ms；首 chunk 后清掉该定时器，改由 chunk 空闲计时
  `b=600000`ms 控制；每次解析到 chunk 都会重置。
* 首 chunk 拦截（`createTenantFrozenInterceptFetch`，offset 150675437）只在 `H()` 分支、
  且响应 `status===200` 时读第一块并检查前 512 字节：
  * 含 `TM.00020005` → 日志 `intercept TM.00020005 tenant frozen error in first chunk`，
    返回 `new Response(body,{status:400, headers:原响应头})`；
  * 含 `ModelArts.81101` / `81111` / `81114` / `BusyException`：
    - 若 `lw()` 判定为上下文超长（如 `prompt length ... maximum input length`）→ **保留原响应**；
    - 否则日志 `… BusyException in first chunk`，返回 status `429`。
* 正常日志 `[${sessionId}] response stream passthrough (no-tee), probeFailed=${f}, hasFirstChunk=${!!c}`，取消 `passthrough stream cancelled by downstream`。

### 3.7 内容安全扫描（仅提示，不裁剪）`[打包JS]`

`createContentSecurityInterceptFetch(n,u,t)`：仅 `status===200 && body`，且
`u = n.headers["x-session-user-message"] === "true"` 时才扫描；否则原样返回。
按 `\n` 切分、跳过空行和 `data: [DONE]`：

| 命中 | 处理 |
|-|-|
| `error_code === "TM.00001054"` 且有 `error_msg` | 日志 `[${n}] Content security intercepted` + `Uw(a.error_msg, "warning")` |
| `desensitized === true` 且有 `desensitizeHint` | 日志 `[${n}] Content desensitized` + `Uw(a.desensitizeHint, "info")` |

兜底正则：`/"error_msg"\s*:\s*"([^"]+)"/`、`/"desensitizeHint"\s*:\s*"([^"]+)"/`。
这里只是**旁路扫描并弹提示**，不会删除/替换内容，也不终止流；最后重新包
`new Response(d, {status, headers})`，body 内容保持一致。

### 3.8 server 实现状态（2026-11-16）

已落地 `src/providers/codearts/chat.rs` / `stream.rs`：

* `POST {SNAP_GATEWAY}/api/v2/chat/completions`，`stream:true`；非流式由 server 聚合。
* AKSK 签名头：`content-type`、`x-sdk-content-sha256`（body SHA256）、
  `x-security-token`、可选 `maas_type:benefit`；trace/model 头在签名后追加。
* 请求体：`model`/`messages`/`stream`/`max_tokens:65536`/`tool_stream`/`tools`/
  `tool_choice`/`prompt_cache_key`；assistant 历史恒带 `reasoning_content`。
* 响应：标准 CC SSE，解析 `content`、`reasoning_content`/`reasoning`、`tool_calls`、
  `finish_reason`、`usage`；错误帧兼容 `error` 与顶层 `error_code/error_msg`。
* `TM.00001041` 并发会话满：HTTP 400 时每 10s 重试，最多 180 次（30 分钟）。

**未实现/已知限制**：

* SM4-GCM 加密链路（§3.2）未实现；当前依赖上游明文回退，实测可用。
* SSE 流内 `TM.00001041` 等排队错误不会自动重连（HTTP 400 会重试）。
* 首帧租户冻结/内容安全扫描/客户端断开取消未复刻。
* `glm-5.3-flash`、`deepseek-v4.1-flash` 等 benefit 模型：本账号带
  `maas_type:benefit` 实测返回 `InferHub.APIG.0101.404`（疑似账号无 benefit 权益），
  普通模型 `GLM-5.2`、`glm-5.3` 正常。

**已实测**：`GLM-5.2` 流式/非流式、工具调用（`get_time`）、工具结果回传均正常；
`test` 用户的 pi 通过本地 `ofm` provider 完成纯文本对话和工具获取系统信息测试。

---

## 4. models 端点映射

> **实现结论（2026-02-18，已实测并落地 `catalog.rs`）**：商业版（非 gamma、无权益）真实可用的
> 模型目录路径是内核 `$D`（provider-models-plugin）：
>
> ```http
> GET https://snap-access.cn-north-4.myhuaweicloud.com/v1/model/builtin
> Content-Type: application/json
> Agent-Type: PromptCenter
> X-Language: zh-cn
> X-Security-Token: <security_token>   # 参与 AKSK 签名（SDK-HMAC-SHA256，同 §4.5 算法族）
> X-Sdk-Content-Sha256: UNSIGNED-PAYLOAD   # payload hash 固定值，不是空体 SHA256
> ```
>
> 签名器还会补 `Host`、`X-Sdk-Date`、`Authorization`；`SignedHeaders` 必须包含
> `content-type`、`agent-type`、`x-language`、`x-security-token`、`x-sdk-content-sha256`、
> `host`、`x-sdk-date`。
>
> 响应 `{count, builtinModels:[{model_id, model_name, enable, credit[{ratio, ratio_display}],
> supports_images, max_tokens, context_window, channel_source:"codearts", provider:"internal", …}]}`。
> 实测返回 12 个模型（GLM-5.2、openpangu-2.0-flash/pro、glm-5.3 系列、deepseek-v4.1 系列、
> kimi-k3 系列、speed 等），倍率取 `credit[0].ratio`（如 GLM-5.2 → x0.05）。
>
> **排除记录**：
> - benefit-service `opengw…/api/v1/gateway/config`：AKSK 签名被服务端接受（HTTP 200），
>   但业务恒报 `{"error_code":"4000","error_msg":"invalid channel, group not found"}`——账号未
>   开通权益（extension 先查 `{policyCenterDomain}/v1/benefit-gateway-config`，`enabled=false`
>   时不走 benefit 网关），属预期；已开通权益的账号才会从 `gateway/config`（§5.4）拿到福利模型。
> - xCenter/agent-center（§4.1/4.2）：商业版内核 `XCENTER_BASE_URLS=[]`、`CODEAGENT_ID=""`
>   硬编码为空，`BD()` 直接短路，整条路径禁用。
> - extension PromptCenter（§4.3）：商业版 `x-auth-token` 为空串（`Tr()`：`isDefaultToken=false`
>   且非内部版 → `""`），不可复刻。
> - 内部域名 `snapengine-apig.cn-north-7.myhuaweicloud.com`：仅 `IS_PROD!="true"` 的 HC 内部
>   环境生效，公网 DNS 不解析。

两套来源，按进程区分。

### 4.1 kernel：AgentCenter（inferhub provider）`[打包JS]`

```
GET {xCenterBase}/v1/agent-center/agents/useragents?offset=0&limit=100
GET {xCenterBase}/v1/agent-center/agents/detail?agent_id=<id>
```

`xCenterBase` 由 `Asu()` 决定：`XCENTER_BASE_URLS`(env，数组) → `W.get().inferhubBaseUrl` 去尾 `/api/v2/` → 空。
日志 `resolveUrls using XCENTER_BASE_URLS` / `resolveUrls fallback to inferhubUrl` / `resolveUrls no url available`。
多 URL 逐个 fallback（`UD()`，retries 2，指数退避，`429`/`5xx`/`ECONNABORTED` 重试），全失败日志 `requestWithFallback all urls exhausted` 返回 `null`。

headers（`SD(n,ak,sk,token)`）：
```
Content-Type: application/json
Agent-Type: AgentCenter
X-Language: zh|en
X-Security-token: <security_token>     # 可选
（AKSK 模式追加 SDK-HMAC-SHA256 签名）
```

`detail` 响应取 `data.gpts.models`，每项字段：
`model_id`、`model_name`、`model_alias`、`context_window`、`max_tokens`、`enable_queue`、`credit[].ratio_display`、
`model_type`（`"third"` → `model_parameters.third_party_model`）。

`resource id` 缺失即跳过：日志 `agent_id is empty, skipping xCenter models fetch`。

**缓存**：`{cacheDir}/models-cache.json`，TTL `86400000`ms（`nTn` / `lD`）。
`ensureValidToken` 失败仅 warn `ensureValidToken failed, will attempt cache fallback`。
ACP 模式下失败仅 warn `loadXCenterModels failed in ACP mode, continuing with local config models`。

### 4.2 thinking variants `[打包JS]`

```
POST {xCenterBase}/v1/model/thinking-variants      （hD）
body {"model_names":[<前 50 个>]}                  （VFn=50）
→ data.variants
```
`XCENTER_BASE_URLS` 为空时日志 `XCENTER_BASE_URLS is empty` 直接返回。用于给每个模型补 `variants`（思考强度档位）。

#### 4.2.1 思考档位完整链路 `[打包JS]`

CodeArts 的思考档位**不来自本地规则**，全部由服务端下发。证据：本地档位生成函数
`kS.variants`（`q(L)`，offset 111131407 区）对 GLM 等模型明确返回空：

```js
function q(L){
  if(!L.capabilities.reasoning) return {};
  let F = L.id.toLowerCase(), ...;
  if(F.includes("deepseek")||F.includes("minimax")||F.includes("glm")||
     F.includes("mistral")||F.includes("kimi")||F.includes("k2p5")) return {};   // ← glm 排除
  ...
}
```

于是 `glm-*` 的档位只能由 `thinking-variants` 提供，链路如下：

1. **模型能力**：`GET …/gpts/models` → `model_parameters.thinking_type != null` → `capabilities.reasoning`；
   同时 `variants_key = model_type === "third" ? model_parameters.third_party_model : model_id`。
   注意 `variants_key` **不必等于模型 id**，第三方模型用 `third_party_model`。
2. **拉档位**（`CJ()` = `fetchThinkingVariants`，offset 99272736 区）：
   ```js
   POST {XCENTER_BASE}/v1/model/thinking-variants
   { "model_names": [variants_key, ...] }        // 只发前 50 个（VFn=50）
   → data.variants[variants_key].strengths        // string[]，缺省视为 []
   ```
   重试：`retries:2`，遇网络错/`ECONNABORTED`/429/≥500 重试；多 base URL 轮询。
3. **档位 → AI SDK variants**（`QJ()`）：
   ```js
   function QJ(n){ let u={}; for(let t of n) u[t]={reasoningEffort:t}; return u }
   ```
   即 `strengths` 里的字符串**本身就是** `reasoning_effort` 的取值，一字不改。
4. **挂到模型**：
   ```js
   variants: (()=>{ let $=y?.variants_key, E=$?f[$]:void 0;
                    return E?.length>0 ? QJ(E) : void 0 })()
   ```
   无 `strengths` 的模型 `variants` 为 `undefined`（UI 不显示滑杆）。

**UI 档位文案**（通用 i18n 表，非 GLM 专有）：
`thinkingLevelMinimal/Low/Medium/High/Xhigh/Max/None/Auto` → 最小/低/中/高/超高/最大/默认/自动。
滑杆只在 `variants` 存在时出现，故只有服务端下发了 `strengths` 的模型（如两个 GLM 5.2）可控。

**发往上游**：选中档位 → `providerOptions.reasoningEffort` → request body `reasoning_effort`（原值）。
与 §3.3.1 一致：GLM 不被 `fU` 匹配，故即使带 `tools` 也不会被改成 `none`。

**推论（未实测）**：`ofm-server` 若要支持 CodeArts 思考档位，正确做法是自行调用
`POST /v1/model/thinking-variants` 取 `strengths`，再把用户档位作为 `reasoning_effort` 透传；
不应硬编码 `low`/`high`，因为档位集合是逐模型、服务端下发的。

### 4.3 IDE 扩展：PromptCenter `[打包JS]`

| 端点 | 用途 |
|-|-|
| `GET {policyDomain}/v1/model/builtin` | 内置模型列表（`getModelBuiltinList`，返回 `builtinModels[]`） |
| `GET {policyDomain}/v1/third-party-model/model/list?page_num=1&page_size=100` | 第三方模型（`getThirdPartyModelList`，返回 `list[]`） |
| `GET {policyDomain}/v1/llm/vendors` | 供应商 |
| `GET {policyDomain}/v1/llm/tenant/settings` | 读 `third_party_model_enabled`（自定义模型开关） |
| `GET {policyDomain}/v1/default/models` | `_getAllmodels` |
| `GET {policyDomain}/v1/model/credit-ratio` | `creditRatioDisplay`（`credit[].ratio_display` join `" "`） |

共同 headers：
```
x-snap-traceid: <uuid 去 ->
Content-Type: application/json
x-auth-token: <accessToken>        # 注意不是 AKSK
Agent-Type: PromptCenter           # 第三方模型列表用小写 agent-type
X-Language: zh-cn
```
缓存 TTL `SETTING_PAGE_CACHE_TTL`；日志 `getModelBuiltinList cache hit, count:{0}`、`getThirdPartyModelList skipped: packageType is {0}`。

`checkCustomModelPermission`（`NV()`）：
token = `n.userToken || n.userInfo?.accessToken`；`Asu()` 为空 → `{allowCustomModel:false}`；
逐 URL fallback，headers `Osu(...)`；异常 → 日志 `tenant settings API error, treating as free version` + `{allowCustomModel:true}`。
日志 `checkCustomModelPermission start`、`trying baseUrl`、`all fallback urls exhausted`。

### 4.4 多模态模型白名单 `[打包JS]`

CLI 模式默认：`["Qwen3.6-27B-VL","Qwen3.5-397B-A17B-VL"]`；IDE 模式再加 `"Qwen3-VL-235B"`。
过滤函数 `hw(_)`。缓存 `wD=600000`ms。
图片输入仅在模型命中白名单时可用，否则报 `未获取到可用的多模态模型` / `No multimodal model is available`。

### 4.5 AKSK 签名（模型详情请求用）`[打包JS]`

`signSimple`（`YLn(n,u,t,i)`）：

```
X-Sdk-Date:            YYYYMMDDTHHmmssZ (ISO8601 无分隔)
Host:                  <host>
X-Sdk-Content-Sha256:  UNSIGNED-PAYLOAD
X-Security-Token:      <security_token>
canonical = [ method, pathname + "/", "", canonicalHeaders, signedHeaders, "UNSIGNED-PAYLOAD" ]
Authorization: SDK-HMAC-SHA256 Access=<ak>, SignedHeaders=<signedHeaders>, Signature=<hex>
```
日志 `signSimple start` / `signSimple: parsed URL` / `signSimple: final signed headers`（Authorization 打码）。

---

## 5. 余额获取

### 5.1 唯一的账户余额端点 `[打包JS + curl 实测 2026-11-16]`

UI 上「总已用积分: 2.78/9600」的数据源**不是** benefit-service，也不是 `/v1/ops/credit/usage`：

```
GET {chatDomain}/snap-manager/v1/statistics/plugin
    （chatDomain = snap-access.cn-north-4.myhuaweicloud.com）
    headers: x-auth-token: await Tr()   ← 商用版本应为空，AKSK 签名亦可
```

响应**没有** `{code,data}` 壳，直接是对象：

```json
{"metrics":[
  {"name":"usageTokenChatMessages","usage_token_num":25209,"package_token_amount":1},
  {"name":"usageTotalPackageCredit","package_credit_amount":9600.00,
   "package_credit_used":2.78,"package_credit_remain":9597.22,
   "package_credit_expiring_amount":7097.22},
  {"name":"usageBasicPackageCredit","package_credit_amount":500.00,
   "package_credit_used":0.00,"package_credit_remain":500.00},
  {"name":"usageBonusPackageCredit","package_credit_amount":9100.0,
   "package_credit_used":2.78,"package_credit_remain":9097.22,
   "package_credit_expiring_amount":7097.22}
 ],"model_metrics":[...]}
```

* 总积分看 `usageTotalPackageCredit`：`package_credit_amount`（总量）/
  `package_credit_used`（已用）/ `package_credit_remain`（剩余）。
* 分档：`usageBasicPackageCredit`（基础包 500）+ `usageBonusPackageCredit`（赠送包 9100，
  含签到/活动所得）= 9600。`package_credit_expiring_amount` 为即将过期额度。
* `model_metrics[]` 同时给出各模型的 `enable` / `status` / `usage_token_num`，
  是「模型是否激活」的真源（`SubscriptionService.returnSubscriptionInfo`）。

`/v1/ops/credit/usage` 的响应形如 `{totalTokenUsed,promptTokenUsed,completionTokenUsed,
cachedTokenUsed,cacheHitRatio,creditCost}`。

### 5.2 已被证伪的三个来源（勿再使用）

| 来源 | 结论 |
|-|-|
| `GET opengw.developer.huaweicloud.com/api/v1/user/tokens/balance` | 本账号恒 `{"error_code":"4000","error_msg":"invalid channel, group not found"}`，是**福利模型额度**渠道，非账户积分 |
| `GET /v1/ops/credit/usage?type=&id=` | 单条消息/会话的成本明细，`type` 只收 `msg`/`session` |
| `claim`/`confirm` 的 `totalAmount`/`remainingAmount` | 单个活动积分包的量（100、1800…），非账户总额 |

> 早期本文档把 `tokens/balance` 当作余额、`benefit/claim` 当作签到，均属误判，
> 已于 2026-11-16 更正。benefit-service 的真实语义见 §5.4。

### 5.3 server 实现

`ops::fetch_balance` → `fetch_credit_usage`，取 `usageTotalPackageCredit` 的
`package_credit_amount − package_credit_used`。

`send_raw` 返回原始 JSON、`send` 再解 `{code,message,data}` 壳——因为 statistics
端点**没有**壳。

实测 `ofm-server status codearts` → `余额: 9597.22`（= 9600 − 2.78，与 UI 一致）。

### 5.4 benefit-service（福利模型额度，不是余额）

> 代码位于 **agent kernel** 的 `benefit-service`（`_D`/`PFn`/`DFn`/`MFn`），非 IDE 扩展。
> 独立网关 `opengw.developer.huaweicloud.com`（生产）/ `snapengine-apig.cn-north-7`（内网），与 chat 域名不同。

真实语义（`tTn()` 加载福利模型）：

```
mD()  GET  /api/v1/gateway/config    → 有 models 才继续
aD()  POST /api/v1/benefit/claim     → 失败则「跳过全部福利模型」并 log
                                        claimBenefit failed, skip loading benefit models
      成功则把 config.models 并入模型表，标记 is_free_benefit
```

即 `claim` 是「福利模型渠道能否加载」的前置开关，**与积分 / 签到无关**。

`benefitBase` 选择：`MFn() ? "https://snapengine-apig.cn-north-7.myhuaweicloud.com" : "https://opengw.developer.huaweicloud.com"`，
`MFn() = H() && process.env.IS_PROD !== "true"`。

headers（`PFn()`）：`Content-Type: application/json` + `x-snap-traceid: <uuid 去 ->` +
`X-Security-token: <security_token>`（若有）+ SDK-HMAC-SHA256 签名。超时 `10s`。
成功判定 `error_code === "0000"`，取 `result`；错误字段名是 `error_msg`。

> **签名两大坑（实测）**：
> 1. **POST body 的 payload hash 强校验**：发 `{}` body 必须签 `sha256("{}")`，
>    签空体 hash（`e3b0c4…`）会被拒 `APIG.0301`。SDK 语义：有 body 哈希 body，
>    无 body 才用空体 hash。
> 2. **不得重复发送已参与签名的头**：签名头已含 `content-type`，发请求时再追加一次
>    会变成重复头，服务端重算签名值不符 → 同样 `APIG.0301`。
> 3. **query 不参与签名**（与 §7 的 `/v1/ops/*` **相反**）：官方 `y2i` 的 `HttpRequest`
>    不解析 query，canonical query 恒为空；实测两种写法服务端都接受。

`gateway/config` 返回：

```js
{ endpoint: "<base_url>", models: [{ model_id, model_name, model_alias }] }
```

`endpoint || base_url` 作该批福利模型的 chat 基址；`hw(modelId)` 判断「是否福利模型」。

`provider-models-plugin` 注册两个 provider：`inferhub-provider` 与 `benefit-model`。
`uTn(n)` 按 `{modelName, maasType}` 解析：`maasType === "benefit"` 时在 `gateway/config`
的模型表里查，命中则仍走 `inferhub-provider` 并带 `traceMaasType`，否则 `providerID="benefit-model"`。
**福利模型和能力模型走不同计费通道**，不区分会导致双计费（代码注释原文 `potential dual-billing risk`）。

`server 实现`：`benefit.rs` 仅保留 `fetch_config`（当前无调用点）。福利模型接入 chat
仍受 `InferHub.APIG.0101.404` 阻塞（需 `maas_type: benefit` 头与额外签名）。

## 6. 上游异常响应

> 本节（错误码表 `hVn`/`RFn`、流内错误 `_P`、4291→402 映射）代码位于 **agent kernel**（`benefit-service` 等），非 IDE 扩展。

### 6.1 业务错误码 `hVn` `[打包JS]`

| code | 含义 |
|-|-|
| `4000` | 请求参数错误 |
| `4003` | 账号被禁用 |
| `4004` | 权益不存在 / 已过期 |
| `4005` | 模型不受支持 |
| `4007` | `Provider not allowed for your group.` |
| `4291` | 配额超限 / 月度 token 超限 |
| `5000` | 服务不可用 |

按语言返回中文/英文（`oP(n)`）；`dVn(n)` 取错误码第 2 段。

`InferHub.4291.200` → 映射 **402**，message：
* zh：`福利模型额度已用尽，请切换其他模型继续使用`
* en：`The free benefit model quota is used up. Please switch to another model to continue.`

### 6.2 流内错误 `_P(n,u,t)`（offset 150640877）`[打包JS]`

`H()` 分支解析 `{error_code, error_msg}`；非 `H()` 为 `{...n.error, rateLimitInfo}`。
判定：
* `lw(mc(n))` 命中 → 上下文超长，转 `al(u,n)`
* `InferHub.4291.200` → 402 不可重试
* 含 `.429` → 结合 `x-session-retry` 计数判定可重试

### 6.3 限流 `fP` + 退避 `[打包JS]`

限流维度键：`RPM` / `RPH` / `RP6H` / `RPD` / `TPM` / `TPH` / `TP6H` / `TPD`。
`Bw(n)` 只判 `RPM` / `TPM`（决定是否算"短时限流"）。

退避：

| 函数 | 公式 | 上限 |
|-|-|-|
| `Pd(n)` | `min(5000 · 2ⁿ, 60000)` | 60s |
| `k6(n)` | `min(2000 · 2ⁿ, 30000)` | 30s |

token 限额告警阈值：90% / 100%。

重试判定（`aU`）：
* `429` 或 `"406"` → `yU()`（可重试）
* `407` → `isRetryable = attempt < 3`
* `x-session-retry >= 3` → 停止
* 响应头含 `need-switch-model` → 建议换模型（日志 `[retriable decision] response header hasRetry: …, needswitch: …`）

### 6.4 权益/签到错误码 `RFn` `[打包JS]`

| 端点 | 错误码 |
|-|-|
| `claim` | `4000` 参数错误、`4001` token 缺失/无效、`4002` token 过期、`4003` 用户禁用、`4004` 权益不存在/已过期、`4006` **daily claim limit exhausted**、`5000` 内部错误 |
| `balance` | `4000` 参数错误/无效 channel、`4001`、`4002`、`4004` benefit/user not found、`5000` |
| `config` | `4000` 参数错误/无效 channel、`4004` group not found、`5000` |

失败统一 `{success:false, error_code, error_message}`；网络异常 `error_code:"network_error"`。

### 6.5 其他 `[打包JS]`

* `InferhubBaseUrlMissing`：`statusCode 555`、`isRetryable:false`（配置缺失，不应重试）
* 租户冻结：首 chunk `TM.00020005` → 400；`ModelArts.81101`/`81111`/`81114`/`BusyException`
  非上下文超长 → 429；上下文超长保留原响应（§3.6）
* 内容安全：`TM.00001054` 只弹提示，不裁剪/不终止流（§3.7）
* 队列：`TM.00001045` → IDE 弹错

---

## 7. 签到协议（限时福利 / 每日签到）

签到走 `https://snap-access.cn-north-4.myhuaweicloud.com/v1/ops/*`，**AKSK 签名即可调通**，
server 已实现并实领成功（§7.2）。

> 客户端里另有一套 benefit-service（`opengw.developer.huaweicloud.com`），只管**福利模型额度**，
> 与签到、积分余额均无关。见 §5.4 与 §7.1。

### 7.1 benefit-service 的 `claim` **不是签到** `[打包JS + curl]`

> ⚠️ 客户端里有两套互不相干的「福利」，早期文档把二者混为一谈。
> benefit-service 的 `claim` 只是**福利模型渠道的加载开关**，完整语义与签名见 **§5.4**。
> 本账号对该端点恒返回 `{"error_code":"4000","error_msg":"invalid channel, group not found"}`
> （福利模型渠道未开通），与 method / channel / body 均无关。

两套官方实现调用方式不同（端点相同）：

| 项 | kernel（`aD`） | IDE 扩展（`SLc`） |
|-|-|-|
| 方法 | `POST /api/v1/benefit/claim`，body `{}` | `GET`，channel 走 query |
| channel | 不带（默认 benefit） | `benefit` / `education` / `examination` 各自独立 |
| 前置条件 | 仅 `gateway/config` 有 models | 三个 `*-gateway-config` 开关（`enabled` / `is_student_certified` / `is_examination_domain`） |
| 后续动作 | 合并福利模型进模型表 | `POST enableBenefitClaim` + 读额度 |

`RFn.claim` 错误码：`4000` 参数错误、`4001`/`4002` token 无效/过期、`4003` 用户禁用、
`4004` 权益不存在、`4006` 每日名额耗尽、`5000` internal。
**kernel 把 `4006` 也当失败处理（无特殊分支）。**

server 侧已删除这套 claim 实现，`benefit.rs` 仅保留 `fetch_config`（无调用点）。

### 7.2 「限时福利 / 每日签到」`/v1/ops/*`（workbench 面板 + 扩展消息通道）`[打包JS]`

这是用户在 UI 上看到的「限时福利 → 每日签到领 100 积分」。代码里叫 **welfare / activity**，
**关键词不是「签到」**（`签到` 只出现在语言包里，见下），搜 `claim` / `benefit` 也搜不到，
必须搜 `welfare` / `ops/claim`。

#### 7.2.1 三个入口（同一套 HTTP 协议）

| 入口 | 文件 / 符号 | 说明 |
|-|-|-|
| workbench 面板（**主入口**） | `out/vs/workbench/workbench.desktop.main.js` · `ActivityWelfarePane` | 命令 `workbench.action.showActivityWelfare`，标题 `localize2(3554, "Limited-Time Offer")`，`Codicon.gift`；显示条件 `shouldShowActivityWelfare()`：`packageInfo.isCreditPackage === true` |
| 福利弹窗 | 同上 · `ActivityWelfarePane.showPopItems` / `popContent` | 服务端 `displayConfig.popTitle/popContent` 下发才弹 |
| webview 侧（聊天窗内） | `extensions/huaweicloud.codearts-snap/dist/assets/SnapChat-CEg8_cfl.js`（@~396090）+ `index-BIyAOOK4.js`（@~384771） | 只发 VSCode 消息（`claimWelfare`/`getWelfareDelivery`/`confirmWelfare`/`generateInviteCode`/`reportCampaignRead`/`setDailyClaimDontRemind`），**不发 HTTP** |

扩展侧真正的 HTTP 实现在 `extensions/huaweicloud.codearts-snap/out/extension.js` @~8587000（`hBi` 对象），
用 `Kx()` 构造头、`Dt.instance.sendRequest()` 发请求，发完 `workbench.action.refreshActivityWelfare` 刷新面板。

#### 7.2.2 端点与请求 `[打包JS]`

Base URL：
* workbench：`this.productService.snapEngineUrl`（= product.json `commercialVersionDomain.newFramework.productDomain.snapEngineDomain`）
* 扩展：`Qe.getInstance().{welfareDelivery,welfareClaim,welfareConfirm,welfareInvite}Url`
  = `${policyCenterDomain}/v1/ops/…`（新框架）/ `${policyCenterDomain}/PromptCenterService/v1/ops/…`（旧框架）
* 生产值均为 `https://snap-access.cn-north-4.myhuaweicloud.com`（香港区 `ap-southeast-1`）

```http
GET  /v1/ops/delivery?channel=IDE           # 拉活动列表 → data.items[]
GET  /v1/ops/tasks/conversation?channel=IDE # 会话任务（失败静默返回 []）
POST /v1/ops/claim                          # 签到/领取
POST /v1/ops/confirm                        # 领取后确认
POST /v1/ops/invite/generate                # 邀请码
POST /v1/ops/events/trigger                 # 事件触发（另有 events/conversation）
```

`POST /v1/ops/claim`：

```json
// 整活动领取（workbench ActivityWelfarePane 主路径）
{"campaignId": <id>, "channel": "IDE"}
// 单项领取（claimActions 分档位）
{"campaignId": <id>, "channel": "IDE", "benefitId": <bid>}
// 扩展/webview 形态：多带幂等键，bench 键格式 claim_<campaign>_[<benefit>|all]_<Date.now()>
{"campaignId": <id>, "idempotentKey": "claim_<id>_all_<ts>", "channel": "IDE"}
```

`POST /v1/ops/confirm`：`{"campaignId": <id>}`（可带 `benefitId`）。
`POST /v1/ops/invite/generate`：`{"campaignId": <id>, "channel": "IDE"}`。

#### 7.2.3 认证头 `[打包JS + 实测]`

**只需 AKSK SDK-HMAC-SHA256 签名，不需要 `x-auth-token`**（此前的结论是错的，见下方勘误）。

```
# workbench（华为云 SDK 签名，getHeaders + getSignedHeaders）
Agent-Type: PromptCenter
X-Language: zh-cn | en-us
(credential 来自 huaweicloud.authentication 的 secret
 HuaweiCloudSession = {accessKey, secretKey, securitytoken}
 → sign() 补 X-Security-Token / X-Sdk-Date / Authorization)

# 扩展（Kx()，同样无 x-auth-token）
x-snap-traceid: <uuid 去 ->
Content-Type: application/json
x-auth-token: await Tr()      ← 商用版恒为 ""，发出即空串
Agent-Type: PromptCenter
X-Language: zh-cn | en-us
```

`Tr()` 三分支（`extension.js` @4474864）：`isDefaultToken`→`developersToken`；
`isInnerVersion`→`userInfo.cloudDragonToken`；商用版→ `""`。
空串意味着「这次请求不带/带空 `x-auth-token`」，而非「必须由 toolkit 注入」。

`product.json` 实测字段（`extensions/huaweicloud.codearts-snap/product.json`）：
`isDefaultToken=false`、`isInnerVersion=false`、`isCommercialVersion=true`、`scenario=null`。
`out/main.js` 的 `getW3AuthHeaders()` 仅在 `scenario === Scenario.HW`（华为内网）时
才补 `x-auth-token = getPassword('huaweicloud.huawei-authentication','CloudDragonToken')`，
商用版该分支直接跳过。

**唯一硬性业务头是 `Agent-Type`**：缺失返回 `400 {"error_code":"TM.00001001","error_msg":"请求头Agent-Type为空"}`。

**签名差异（与 benefit 不同）**：query **必须**参与签名。
`channel=IDE` 需编码进 canonical query；不签会得
`401 {"error_msg":"Incorrect IAM authentication information: verify ak sk signature fail","error_code":"APIG.0301"}`。

#### 7.2.4 实测（2026-11-16，`testdata` 凭证）

| 请求 | 结果 |
|-|-|
| AKSK 签名 + `Agent-Type`/`X-Language` + **query 参与签名** | **`200 {"code":0,"message":"ok",...}`** |
| AKSK 签名 + `Agent-Type`，但 query **不**参与签名 | `401 APIG.0301 verify ak sk signature fail` |
| AKSK 签名，缺 `Agent-Type` | `400 TM.00001001 请求头Agent-Type为空` |
| **不签名**（仅 `x-auth-token: <security_token>`） | `401 APIG.0301 x-auth-token not found` |

> ⚠️ **勘误**：早前记录「网关强制 `x-auth-token`、AKSK 不被接受、签到无法复刻」是**误判**。
> 当时的两次失败分别是「带 `Agent-Type` 但不签名」和「签名但漏 `Agent-Type`」，
> 被误读为该头不可替代。真实情况：AKSK 签名完全够用，`x-auth-token` 商用版本就是空串。

实领全流程已验证（本账号 2026-11-16 13:31 UTC）：

```
GET  /v1/ops/delivery?channel=IDE
 → items[0] = {campaignId:34, title:"每日签到领100积分", type:"USER_LOGIN",
               benefitAmount:100, benefitUnit:"CREDIT", status:"ELIGIBLE",
               claimable:true, pendingCount:1, pendingTotalAmount:100.0,
               extra:{triggerEvent:"user.login", triggerMode:"MANUAL_CLAIM",
                      startTime:"2026-10-26T16:00:00Z", endTime:"2026-12-31T15:59:59Z"}}
POST /v1/ops/claim {"campaignId":34,"channel":"IDE","idempotentKey":"claim_34_all_<ms>"}
 → {"code":0,"data":{"status":"CLAIMED","totalAmount":100.0,"remainingAmount":100.0,
                     "pointBucket":"GENERAL","bucketLabel":"通用积分",
                     "claimedAt":"2026-11-16T13:31:34.866705458Z",
                     "expireAt":"2026-11-17T13:31:34.866705458Z"}}
POST /v1/ops/confirm {"campaignId":34}
 → {"code":0,"data":{"status":"CONFIRMED","totalAmount":1800.0,"remainingAmount":1763.48,
                     "confirmedAt":"..."}}
GET  /v1/ops/delivery?channel=IDE
 → items[0] = {status:"CONFIRMED", claimable:false, pendingCount:0}
```

即：领取后 `totalAmount` 记的是**账户积分总额**（本次 100 已并入），
`remainingAmount` 是剩余可用；积分**次日 13:31 过期**（`expireAt` = claimedAt + 24h）。
`status` 走完 `ELIGIBLE → CLAIMED → CONFIRMED` 一轮；次日 `claimable` 重新变 `true`。

同一 `delivery` 里还有 `campaignId:4`「新用户注册送 4000 积分」（`status:CLAIMED`、
`triggerMode:AUTO_GRANT`，已自动发放）与 `campaignId:2`「学生认证领积分」（`status:null`，
未认证 → 不显示）。可见「100 积分」是**服务端下发的活动配置**，不是客户端硬编码。

#### 7.2.4b 账户积分余额

见 **§5.3**（`/snap-manager/v1/statistics/plugin`）。`claim`/`confirm` 响应的
`totalAmount`/`remainingAmount` 只是该活动积分包的量，不是账户总额。

#### 7.2.5 业务语义 `[打包JS]`

`request()`（workbench）判定：HTTP 200 **且** `result.code === 0 && result.status === 0`，取 `result.data`；
否则抛 `result.message`。返回 `data.status` 枚举 `ELIGIBLE | CLAIMED | CONFIRMED | CONSUMED`，
非 `ELIGIBLE` 直接 return（不重复领取）。

领取成功后（`benefit.status === "CLAIMED"` 且 `benefit.id !== null`）→ 立即 `POST /v1/ops/confirm`
→ 通知 `localize(3534/3545)` → 执行 `SETTINGS_GET_CONFIG.command.getFreeTokenSuccess` → `load()` 刷新面板。
扩展侧同样：`returnWelfareClaim` 成功 → `confirmWelfare` → `getWelfareDelivery` + `getCreditPackageInfo`。

`delivery` 返回的 `items[]` 关键字段：`campaignId`、`type`（`INVITE_USER` / `STUDENT_CERTIFIED` / …）、
`status`、`claimable`、`unread`、`displayConfig`（**JSON 字符串**，含 `popTitle`/`popContent`/`claimButtonText` /
`claimButtonTip` / `jumpUrl`）、`claimActions[]`（`benefitId`、`benefitText`、`status`）、`access[]`、`description`、
`conversationTask`。

UI 文案在 `extensions/vscode-language-pack-zh-hans/translations/main.i18n.json`：
`vs/workbench/browser/parts/accountInfoPane/activityWelfarePane`（`activityWelfareSignedInTip: "今日已签到，明天再来吧"`、
`activityWelfarePendingCredits: "待领取 {0} 积分"`、`activityWelfareClaimed: "成功领取福利"` …）；
webview 侧在 `index-BIyAOOK4.js` @~322928（`freeBenefit: "限时福利"`、
`freeBenefitTip: "活动期间每日发放 1000万免费 Tokens…"`——**注意这是福利模型 Tokens 文案，不是积分签到**）。

> 「100 积分」这个具体数值**不在客户端任何静态资源里**，由服务端 `displayConfig` 下发，
> 因此无法从代码确认；「每日签到」的判定也完全在服务端（`campaignId` 由 delivery 下发）。

## 8. 未验证清单（实现时优先实测）

1. ~~SM4-GCM 密钥协商报文与 AAD 构成~~ → **已从源码解析**：协商 body=`{clientPublicKey,sessionId,timestamp}`，响应 `{serverPublicKey(base64 DER),keyId,expiresIn}`；key=`sha256(shared)[0..16]`；请求 AAD=`method|pathname|ts`（iv/tag/ts 走 header），响应 AAD=`${keyId}|${chunkIndex}`。仅剩「服务端是否真接受明文回退」需实测
2. ~~`/v1/oauth2/tokens` 响应字段名~~ → **已核实**：`{credentials:{access_key_id, secret_access_key, security_token, expiration}, refresh_token}`，`refresh_token` 与 `credentials` 平级。续期时 IDE 带 `code_verifier`、kernel 不带且加 `x-agent-user-account`（§2.1）
3. ~~`/snap-manager/v1/token/renew` 的 method 与响应结构~~ → **已核实为死端点**：IDE 端点表仅声明 + getter，无调用方；kernel 无此字符串。续期唯一链路是 `refresh_token` 重放（§2.3）
4. ~~`balance` 返回字段~~ → **已解决**：账户余额真正来源是
   `/snap-manager/v1/statistics/plugin`（§5.1），已实测 `9600 − 2.78 = 9597.22`。
   原 `tokens/balance` 的 `used_amount`/`total_quota` 属福利模型额度，非账户积分（§5.2）
5. ~~`claim` 是否需要 `channel` 参数~~ → **与签到无关**：benefit-service 的 `claim`
   是福利模型渠道的加载开关（§5.4），本账号恒 `4000`。
   真正的每日签到走 `/v1/ops/*`（§7.2），已实现并实测成功
6. **上游是否真接受未加密（明文）chat 请求** ← 实现前最高优先级，用一次真实请求定夺，决定是否需复刻 SM4-GCM
7. `response_format` / `logprobs` / `n>1` 上游支持情况
8. 排队态是否发 notice 帧给下游
9. `X-Remote-Origin-Url`、`maas_type`、`client-ip` 是否服务端强校验

10. ~~`client_id` 实际值~~ → **已从 product.json/package.json 取证**：扩展 `name` 为
    `codearts-snap`（26.11.100），`QH()` 返回该值；`"vscode-codebot"` 仅是 fallback 常量。
    kernel 版为 `CodeArts_Tui`。登录实现已按 `codearts-snap` 落地，**已实测接受**
    （登录/刷新均签发成功，见 credential.toml）
11. 授权 URL 的 `port` 参数对服务端是否必需（官方为本地回调端口，本实现已带上）。
    `secret` 路径已按 §1.6 实现（10×1s 重试），但未做真实登录实测。
12. token 响应的 `refresh_token` 在扩展侧读顶层字段（`t.refresh_token`），kernel 亦同；
    与 `credentials` 平级，**已核实**。**响应确有 AKSK/STS 形态与其它形态两种**
    （`jri`/`bZe`），实测时注意区分。
13. 官方扩展的 token 请求在直连失败后会读代理并用 `HttpsProxyAgent` 重试一次；
    本 server 的 `token_client()` 目前固定 `no_proxy`，未复刻该重试。
14. ~~续期协议是否可用~~ → **已实测**（§2.1 实测段）：IDE 形态一次成功并轮换 `refresh_token`；
    `refresh_token` 与签发方 `client_id` 强绑定、一次性使用。

> **加密对本项目的影响**：客户端对 `/api/v2/chat/completions` 强制加密，但有明文回退分支（key 协商失败/网络错误/非 2xx 时 `falling back to plaintext`）。若服务端接受明文 → 无需碰 SM4-GCM，直接明文转发；若强制加密 → 须用 Rust 复刻整条链路（ECDH+SM4-GCM，格式已全暴露）。**实现第一步先用明文 curl 探测，再定夺**。
16. ~~真正的「每日签到领积分」无法复刻~~ → **已证伪并已实现**（§7.2.4）：AKSK 签名即可调通
    `/v1/ops/*`，`x-auth-token` 商用版本就是空串。关键条件是「带 `Agent-Type`」+「query 参与签名」。
    `src/providers/codearts/ops.rs` 已完成 delivery / claim / confirm，`claim` 子命令已切到该实现。
17. ~~「100 积分」需真实 delivery 响应取证~~ → **已取证**：`campaignId:34`「每日签到领100积分」，
    `benefitAmount:100`、`benefitUnit:"CREDIT"`、`triggerMode:"MANUAL_CLAIM"`、
    有效期 `2026-10-26T16:00:00Z ~ 2026-12-31T15:59:59Z`；领取后积分 24h 过期。
