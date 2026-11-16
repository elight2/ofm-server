# codearts 客户端协议（华为云 CodeArts Agent / 码道）

**来源**：`~/tools/docs/agent-clients/codearts-agent-linux-arm64-26.11.100-39f41d5.deb`
解包至 `tmp/codearts-extract/`，关键文件：

| 文件 | 大小 | 说明 |
|-|-|-|
| `resources/app/extensions/huaweicloud.codearts-snap/out/extension.js` | 10.5MB | VSCode 扩展（IDE 侧全部业务逻辑） |
| `resources/app/extensions/huaweicloud.codearts-snap/product.json` | — | 域名表 |
| `resources/app/agentkernelServer-linux-arm64` | 175MB | Bun 单文件 ELF，内嵌 `/$bunfs/root/chunk-*.js`（332 chunk），agent kernel（TUI/CLI 侧） |

kernel 内嵌 JS 已导出为 `tmp/codearts-kernel-js.txt`（262MB，grep 需 `-a`）。

**标注方式**：本文全部结论来自**直接阅读打包 JS**，标 `[打包JS]` 并附 grep 可复现的 offset。
**本机 x86_64，安装包为 arm64，未运行客户端**，故没有 `[抓包]` / `[curl]` 证据，一切运行时行为标 `[未验证]`。

---

## 0. 总结

* 客户端版本：app `26.11.100`（distro `d7ca751ba65ff48109f0f0a86178d1d659b4e399`），扩展 `huaweicloud.codearts-snap` `26.11.100`，kernel 内部 `VERSION="126.11.1-20261110192452"` `[打包JS]`
* 分析方式：仅静态读 JS（`grep`/`python3` 切片），未执行二进制，未抓包
* 两套代码并存：**IDE 扩展**（extension.js，PromptCenter 体系）与 **agent kernel**（Bun chunk，AgentCenter/inferhub 体系）。域名、端点、认证参数集**不同**，实现时须选定一侧，不可混用

### UA / 客户端标识 `[打包JS]`

不同进程发不同头，**不可混用**：

| 发起进程 | 头 | 值 |
|-|-|-|
| kernel → 内部 HTTP | `User-Agent` | `AgentKernel/1.0`（offset 102046206 附近，node `d.request` 探测本机服务） |
| kernel → inferhub | `x-ot-client-type` / `x-ot-client-version` / `x-ot-function` | CLI 模式固定 `CLI` / `126.11.1-20261110192452` / `agent-tui`；非 CLI 取调用方传入或空串 |
| 华为 SDK（IAM/AKSK 签名） | `User-Agent` | `huaweicloud-usdk-nodejs/3.0`（offset 8892390，SDK 内 `_request` 默认，若调用方已设则**前缀拼接**为 `huaweicloud-usdk-nodejs/3.0 <原值>`） |
| extension → 华为云 API | `User-Agent` | `DevKit-VSCode:<n>\|<r>:D1` |
| OTel 上报 | `User-Agent` | `OTel-OTLP-Exporter-JavaScript/0.213.0` |

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
| 权益/余额（非生产或内网） | `https://snapengine-apig.cn-north-7.myhuaweicloud.com` | `/api/v1/benefit/claim`、`/api/v1/user/tokens/balance`、`/api/v1/gateway/config` |
| 权益/余额（生产商用） | `https://opengw.developer.huaweicloud.com` | 同上；选择式 `MFn() = H()&&IS_PROD!=="true" ? 内网 : 生产`（offset 9940xxxx，`DFn()`） |
| 内网兜底 | `http://snapengine.codemate.cce.prod-kwe-g.dragon.tools.huawei.com/api/v2/` | `py.inferhubBaseUrl` 默认值（offset 103554750） |
| OTel | `collector-agents-access.{cn-north-4,ap-southeast-1}.myhuaweicloud.com` | `/collector/v1/traces`、`/collector/v1/logs` |

`isCommercialVersion:true`、`isNewFramework:true`、`externalBrandName:"CodeArts Agent"`、`isSupportWebUrl:false` `[product.json]`。

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

---

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

### 2.1 主动续期 `[打包JS]`

主路径：`refresh_token` 重放 §1.3（`grant_type=refresh_token`）。

`HcLoginService.ensureTokenValid`（offset 8753379 区）决策四分支：

| 结果 | 条件 |
|-|-|
| `valid` | 未到阈值 |
| `waitLock` | 另一进程持 `fileLock` 正在续期 |
| `cancel` | 续期被取消 |
| `renew` | 需要续期 → 调 renew |

定时器（`startRenewalTimers`）：
* 启动后 `12e4`ms 首次
* 间隔 `kri` = clamp(计算值, `1800000`~`7200000`)
* 提前量缓冲 `3e5`ms，下限 `6e4`ms
* 凭证无 `expiration` 时 fallback TTL `mZe = 864e5`ms

### 2.2 服务端续期端点 `[打包JS]`

```
POST/GET {chatDomain}/snap-manager/v1/token/renew
```
（`_hcSessionRenewalUrl`，offset 2375900 端点表）。响应体结构 `[未验证]`。

### 2.3 IDE 命令通道 `[打包JS]`

extension 执行 `huaweicloud.authentication.command.renewSession`（offset 10162527），
kernel 侧通过 VSCode 命令桥触发；命令不可用时日志 `CodeArts IDE renewSession command not available: {0}`。

### 2.4 被动失效 `[打包JS]`

remote-session 轮询 `GET {chatDomain}/v1/sessions`（1h 刷新 / 120s 轮询），`status !== 0` → `FORCED_LOGOUT`，需重新走 §1。

> 注：**`/snap-manager/v1/token/renew` 与 `/snap-manager/v1/login/ticket` 是端点表中仅有的 token 相关项**。kernel 实际调用的 snap-manager 路径只有 `chat-session/heartbeat`、`current/user`、`privacy`、`skill/*`、`statistics/plugin`，摘要中曾疑有 `token` 相关实为端点表静态声明 `[打包JS 已 grep 核实]`。

---

## 3. Chat Completions 协议转换

上游为 **OpenAI Chat Completions 兼容**，端点：

```
POST {inferhubBaseUrl}chat/completions
inferhubBaseUrl = `${agentChatDomain}/api/v2/`   →  https://snap-access.cn-north-4.myhuaweicloud.com/api/v2/
```
即实际 `https://snap-access.cn-north-4.myhuaweicloud.com/api/v2/chat/completions` `[打包JS]`。
旧框架另有 `chatUrl = /v1/chat/chat`（`construtNewFrameWorkAllUrl`，offset 2375900），新框架用 `/api/v2/chat/completions`。

`inferhubBaseUrl` 来源优先级：`XCENTER_BASE_URLS`(env) → `W.get().inferhubBaseUrl` 去 `/api/v2/` 尾 → 空。环境变量 `INFERHUB_BASE_URL`（逗号分隔取首个，`Am()`/`JHn()`）。缺失抛 `InferhubBaseUrlMissing`（`error_code: MP`、`statusCode: 555`、`isRetryable: false`）`[打包JS]`。

### 3.1 请求头 `[打包JS]`

`getInferhubHcHeaders(n, u)`（offset 99296691 区）分两模式。

**公共**：

| 头 | 值 |
|-|-|
| `Content-Type` | `application/json` |
| `Accept` | `text/event-stream`（流式） |
| `x-ot-client-type` | CLI 固定 `CLI`；否则 `u["x-ot-client-type"] \|\| t.clientType \|\| ""` |
| `x-ot-client-version` | CLI 固定 `126.11.1-20261110192452`；否则取入参 |
| `x-ot-function` | CLI 固定 `agent-tui`；否则 `u["x-ot-function"] ?? ""` |
| `x-codearts-doer-scenario-type` | `u["x-codearts-doer-scenario-type"] ?? ""` |
| `X-Language` | `zh` / `en`（`jwn()`：`language` 以 `zh` 开头→`zh`，否则 `en`） |
| 透传 | `plugin-name`、`plugin-version`、`ide-name`、`ide-version` |

**trace 组**：`x-ot-trace-id`、`x-ot-span-id`、`x-snap-traceid`、`x-ot-session-id`、`x-ot-parent-session-id`、`user-session-id`（缺省 `randomUUID().replace(/-/g,"")`）、`user-msg-id`、`created-time`。

**模型**：`model-id`、`model-name`（`VP(...)` 归一）、`maas_type`。

**AKSK 模式追加**：`X-Security-token`、`X-Remote-Origin-Url`、`client-ip`（`LP()`），
再 `G6.sign(req, new V6(ak, sk))` 生成华为云 SDK-HMAC-SHA256 签名（见 §3.2）。

调试日志：`[header-debug] _existingHeaders=…, alsModelName=…, finalModelName=…`、`[header-debug] hc headers model-id=…, model-name=…`。

### 3.2 传输加密（SM4-GCM，**实现必须复刻**）`[打包JS]`

配置 `S1`：

```js
{ enabled:true,
  sm4:{ enabled:true, keyExchangePath:"/api/v1/security/key-exchange",
        keyRefreshInterval:3600, fallbackOnFailure:true },
  certPin:{ enabled:false, pins:[] } }
```
可由 env `TRANSPORT_SECURITY_CONFIG` 覆盖（`pN()`）。算法标识 `dl = "sm4-gcm"`。
加密路径白名单：`/api/v2/chat/completions`（`KLn`）；key-exchange 路径自身不加密（`zLn`）。
算法 `LN="sm4-gcm"`，密文走 `application/octet-stream`。

**密钥协商**：

```
POST {base}/api/v1/security/key-exchange    （ECDH P-256，crypto.createECDH）
请求 body: { clientPublicKey:<base64>, sessionId:<16B随机→hex>, timestamp:<Date.now()> }
           clientPublicKey = createECDH("prime256v1").generateKeys().toString("base64")
→ 响应: { serverPublicKey:<base64 DER 裸，非 PEM/JWK>, keyId, expiresIn }
           服务端公钥直接 Buffer.from(serverPublicKey,"base64") 喂 computeSecret
shared = ECDH(clientPriv, serverPub)
key    = sha256(shared) 前 16 字节          （FLn(n)，IV 长度常量 FN=12）
```

**请求报文格式**（iv/tag/ts 走 header，**不在 body 内联**）：
* `AAD = ${method}|${pathname}|${ts}`（`MLn`，ts 参与）
* body = base64(密文)
* headers 追加：`X-Encrypt-Algo: sm4-gcm`、`X-Encrypt-Key-Id: <keyId>`、
  `X-Encrypt-Iv: base64(iv)`、`X-Encrypt-Tag: base64(tag)`、`X-Encrypt-Ts: <ts>`、
  `Content-Type: application/octet-stream`
* 加密后须对 AKSK 头**重新签名**（`b_(url,method,"application/octet-stream")`，跳过 host）；无 AKSK 则保留原 Authorization

**响应报文格式**：
* 响应侧 `aadPrefix = keyId`（`new s5(m.key, m.keyId)`），逐 chunk 解密
* `AAD_chunk = ${aadPrefix}|${chunkIndex}` = `${keyId}|${chunkIndex}`
* chunk 文本以 `:` 分 3 段：`iv:tag:ciphertext`，均 base64；IV 12B、key 16B、tag 独立
* 判定是否加密响应：`X-Encrypt-Algo === "sm4-gcm"`，否则明文透传

**回退**：任何失败转明文，日志：
`Encrypting fetch bypass (URL not in encrypt paths), sending plaintext`、
`Encrypted request failed (network error), falling back to plaintext`、
`Encrypted request failed (non-2xx), falling back to plaintext`；
`ENCRYPT_KEY_EXPIRED` → 重新协商后重试一次。
会话密钥刷新：`keyRefreshInterval=3600`s，提前 `TLn=300000`ms 触发；失败 `fallbackOnFailure:true` 沿用旧 key。
日志：`Session key negotiated`（含 `keyId`、`ttlSec`）、`Session key negotiation failed`、`Falling back to existing key`、`Session key cleared`。

### 3.3 请求映射（Chat Completions → 上游）`[打包JS]`

上游即 CC 形状，主要做**适配与裁剪**：

| 下游字段 | 上游处理 |
|-|-|
| `messages` | 直通；`tool_calls` / `tool` role 直通 |
| `tools[].function.parameters` | 直通（JSON Schema 对象） |
| `max_tokens` | 推理模型（`cU(model)`）→ 重命名为 `max_completion_tokens`（`BP()`），日志 `[openai-reasoning-model] adaptMaxTokensParam applied` |
| `reasoning_effort` | 白名单模型（`fU(model)`）且 `tools` 非空 → **强制 `"none"`**（`AP()`），日志 `[openai-reasoning-model] adaptReasoningEffort applied: … -> "none"` |
| `response_format` | `json_schema` / `json_object` 透传 |
| `strictJsonSchema` | 默认 `true` |
| `user` / `stop` / `seed` / `verbosity` | 透传（`getArgs`） |
| `stream` | `doStream` 传 `true`，`doGenerate` 传 `false` |
| `stream_options.include_usage` | `[未验证]` |

`openai-compatible` 模式有弃用警告。`transformRequestBody` 两个调用点分别是 `{...u, stream:false}` 与 `{...u, stream:true}`。

### 3.4 响应映射（上游 SSE → CC chunk）`[打包JS]`

SSE 逐帧 `data: {...}`，`data: [DONE]` 结束。`processChunk` / `createChatStreamMessageFromJson` 逻辑：
* 成功判定 `error_code === "0000"`
* `<think>…</think>` 从正文分流到 reasoning（`gP`）
* `finish_reason` 映射 `rU`（`stop` / `length` / `tool_calls` / `content_filter`）
* tool 调用增量按 `tool-input-delta` 批处理，阈值 50 字符 / 4096 字节，日志 `[tool-delta-batch] flushToolDelta: …`

usage（`wl(n)`）：

```js
{ inputTokens:{ total, noCache, cacheRead, cacheWrite },
  outputTokens:{ total, text, reasoning },
  raw }
```

### 3.5 排队（queue）`[打包JS]`

上游可返回排队态，两条队列机制并存：

1. **响应内 queueInfo**（`QueueRetryError`，`statusCode:429`、`isRetryable:true`）
   字段 `queue_length`、`queue_wait_time`、`retry_after`（秒，默认 5）
2. **trace 队列轮询**
   ```
   GET  {base}/api/v1/concurrency/trace-queues/{traceId}
   DELETE {base}/api/v1/concurrency/trace-queues/{traceId}   # 取消，best-effort
   ```
   响应 `{status, queue_length, queue_wait_time, retry_after}`；`status` ∈ `queued` / `promoted` / `not_found`。
   轮询失败计数 ≥3 放弃。日志 `poll queue GET failed (attempt N)`、`queue delete sent for traceId=…`
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

* 首 token 超时 `300000`ms，无数据 `600000`ms
* 首个 chunk 若含 `ModelArts.81101 BusyException` → 日志 `… BusyException in first chunk`，返回 `new Response(body,{status:429, headers:原响应头})`（即租户被冻结）
* 正常日志 `[${sessionId}] response stream passthrough (no-tee), probeFailed=${f}, hasFirstChunk=${!!c}`，取消 `passthrough stream cancelled by downstream`

### 3.7 内容安全拦截 `[打包JS]`

`createContentSecurityInterceptFetch(n,u,t)`：仅 `status===200 && body` 时逐行扫描
（按 `\n` 切分，跳过 `data: [DONE]`）：

| 命中 | 处理 |
|-|-|
| 含 `TM.00001054` | 日志 `[${n}] Content security intercepted` + `Uw(a.error_msg, "warning")` |
| `desensitized === true` 且有 `desensitizeHint` | 日志 `[${n}] Content desensitized` + `Uw(a.desensitizeHint, "info")` |

兜底正则：`/"error_msg"\s*:\s*"([^"]+)"/`、`/"desensitizeHint"\s*:\s*"([^"]+)"/`。
处理后重新包 `new Response(d, {status, headers})`。

---

## 4. 模型列表获取

> **实现结论（2026-02-18，已实测并落地 `catalog.rs`）**：商业版（非 gamma、无权益）真实可用的
> 模型目录路径是内核 `$D`（provider-models-plugin）：
>
> ```http
> GET https://snap-access.cn-north-4.myhuaweicloud.com/v1/model/builtin
> Content-Type: application/json
> Agent-Type: PromptCenter
> X-Language: zh-cn
> X-Security-Token: <security_token>   # 参与 AKSK 签名（SDK-HMAC-SHA256，同 §4.5 算法族）
> ```
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
>   时不走 benefit 网关），属预期；已开通权益的账号才会从 §5.1 拿到福利模型。
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

> 本节（`_D`/`PFn`/`DFn`/`MFn`）代码位于 **agent kernel** 的 `benefit-service`，非 IDE 扩展。

**注意：余额/签到走独立网关（`opengw.developer.huaweicloud.com` 生产 / `snapengine-apig` 内网），与 chat 域名不同。**

> **实测记录（2026-02-18，未开通权益账号）**：`gateway/config`、`tokens/balance`、`benefit/claim`
> 三端点在 AKSK 签名被服务端接受的前提下（HTTP 200，非 APIG 签名错误）均返回
> `{"error_code":"4000","error_msg":"invalid channel, group not found"}`。已排除变量：channel
> 参数（无/`benefit`/`education`）、`Content-Type`+`x-snap-traceid` 头有无、claim body
> （`{}`/空）、`X-Security-Token` 头大小写。判定：账号无权益分组时服务端直接拒绝，其余
> 行为无需再验证；注意响应错误字段名是 `error_msg`（无 `error_message`）。

```
GET {benefitBase}/api/v1/user/tokens/balance?channel=<benefit|education|examination>
GET {benefitBase}/api/v1/gateway/config?channel=<…>
```

`benefitBase` 选择：`MFn() ? "https://snapengine-apig.cn-north-7.myhuaweicloud.com" : "https://opengw.developer.huaweicloud.com"`，
`MFn() = H() && process.env.IS_PROD !== "true"`。

headers（`PFn()`，签名版）：
```
Content-Type: application/json
x-snap-traceid: <uuid 去 ->
X-Security-token: <security_token>     # 若有
+ SDK-HMAC-SHA256 签名（Authorization）
```
非签名版 `V5(t,e,r,n)`：GET 参数转 query（`encodeURIComponent`），`Content-Type: application/json`，成功判定 `statusCode===200 && data.error_code==="0000"`，返回 `{success, data: data.result}`。

> **签名两大坑（2026-02-18 实测，实现时必看）**：
> 1. **POST body 的 payload hash 强校验**：发 `{}` body 必须签 `sha256("{}")`，
>    签空体 hash（`e3b0c4…`）会被拒 `APIG.0301 Incorrect IAM authentication
>    information: verify ak sk signature fail`。SDK 语义：有 body 哈希 body，
>    无 body 才用空体 hash（SDK-HMAC-SHA256 后端不支持免签 hash）。
> 2. **不得重复发送已参与签名的头**：签名头已含 `content-type`，发请求时再
>    `header(CONTENT_TYPE, …)` 追加一次会变成重复头，服务端重算签名值不符
>    → 同样 `APIG.0301`。签名的头集合 = 实际发送的头集合，多一个都不行。
> 另：响应错误字段名是 `error_msg`（无 `error_message`）。

超时 `10s`（`AbortController`）。

**成功判定**：`error_code === "0000"`，取 `result`。
**`maas_type` 映射**：`education`→`education`、`examination`→`examination`、其他→`benefit`。

### 5.1 `gateway/config` 返回 `[打包JS]`

```js
{ endpoint: "<base_url>", models: [{ model_id, model_name, model_alias }] }
```
`endpoint || base_url` 作该批福利模型的 chat 基址，写入内存 Map（`T6`）；`hw(modelId)` 用它判断"是否福利模型"。
日志 `benefit models loaded`（列出全部 `model_id`）、`getGatewayConfig: missing endpoint or models`。

### 5.2 与 chat 的关系（`benefit-model` 伪 provider）

`provider-models-plugin` 注册两个 provider：`inferhub-provider` 与 `benefit-model`（同 `npm`）。
`uTn(n)` 按 `{modelName, maasType}` 解析：
* `maasType === "benefit"` → 在 `FJ()`（=`gateway/config` 的模型 Map）里按 `modelId`/`modelName` 找；
  命中且 `provider[inferhub].models[modelId]` 存在 → `{providerID:"inferhub-provider", modelID, traceModelName, traceMaasType}`；
  否则 `providerID="benefit-model"`；未命中 warn `benefit model not found in benefit list, falling through to inferhub lookup`
* 否则按 `provider[inferhub].models` 的 key 或 `models[k].name` 匹配

即：**福利模型和能力模型走不同计费通道**，实现时若不区分会导致双计费（代码注释原文 `potential dual-billing risk`）。

余额返回字段：webview 渲染层取证（`SnapChat-*.js` `returnTokensBalance` 处理器）：
benefit/examination 用 `used_amount` / `total_quota`（模型卡判「额度用尽」同用此二字段）；
education 由 `monthly_tokens_used` / `monthly_token_limit` 映射为同结构。
**实际数值形态 `[未验证]`**（未开通权益账号恒 4000 拿不到 result），实现按
`剩余 = total − used` 解析，字段缺失返回空。

---

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

### 6.2 流内错误 `_P(n,u,t)`（offset 99381617）`[打包JS]`

CLI 分支解析 `{error_code, error_msg}`；非 CLI 为 `{...n.error, rateLimitInfo}`。
判定：
* `lw(mc(n))` 命中 → 上下文超长，转 `al(u,n)`
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
* 租户冻结：首 chunk `ModelArts.81101 BusyException` → 429（§3.6）
* 内容安全：`TM.00001054`（§3.7）
* 队列：`TM.00001045` → IDE 弹错

---

## 7. 签到

> 本节（`aD()`/`_D`/`RFn`）代码位于 **agent kernel** 的 `benefit-service`，非 IDE 扩展。

```
POST {benefitBase}/api/v1/benefit/claim
body: {}
headers: 同 §5（AKSK 签名 + X-Security-token）
```

`aD()`：
* 成功（`error_code === "0000"`）→ 返回 `true`
* 失败 → 日志 `claimBenefit failed`（带 `error_code` / `error_message`），返回 `false`
* `4006` = 今日已签（`daily claim limit exhausted`）
* 超时 10s

`channel` 参数：`ELc(t,e,r)` 会并发拉 `getGatewayConfig` 的 `benefit` / `education` / `examination` 三个 channel；
**`claim` 是否也需要 channel 参数 `[未验证]`**（代码中 `_D("claim","POST")` 未带 channel，推测默认 `benefit`；未开通权益账号无法实测，恒 4000）。

**实测记录（2026-02-18）**：未开通权益账号 claim 返回同 §5 的 `4000 invalid channel, group not found`（POST body `{}` 与空 body 均试过）。

签到记录建议写 `providers/codearts/claim.log`。

---

## 8. 未验证清单（实现时优先实测）

1. ~~SM4-GCM 密钥协商报文与 AAD 构成~~ → **已从源码解析**：协商 body=`{clientPublicKey,sessionId,timestamp}`，响应 `{serverPublicKey(base64 DER),keyId,expiresIn}`；key=`sha256(shared)[0..16]`；请求 AAD=`method|pathname|ts`（iv/tag/ts 走 header），响应 AAD=`${keyId}|${chunkIndex}`。仅剩「服务端是否真接受明文回退」需实测
2. `/v1/oauth2/tokens` 响应的完整字段名（`data.credentials.*` 已确认，`refresh_token` 位置未确认）
3. `/snap-manager/v1/token/renew` 的 method 与响应结构
4. `balance` 返回字段：字段名已从 webview 取证（`used_amount`/`total_quota`，
   education 为 `monthly_tokens_used`/`monthly_token_limit`，见 §5）；数值形态未验证
5. `claim` 是否需要 `channel` 参数（已核实代码 `_D("claim","POST")` 未带，推测默认 `benefit`）
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

> **加密对本项目的影响**：客户端对 `/api/v2/chat/completions` 强制加密，但有明文回退分支（key 协商失败/网络错误/非 2xx 时 `falling back to plaintext`）。若服务端接受明文 → 无需碰 SM4-GCM，直接明文转发；若强制加密 → 须用 Rust 复刻整条链路（ECDH+SM4-GCM，格式已全暴露）。**实现第一步先用明文 curl 探测，再定夺**。
