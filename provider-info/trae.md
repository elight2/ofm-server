# trae 客户端协议

> 客户端版本: Trae CN 1.107.1(appVersion `3.4.18`) ｜ 分析方式: 抓包 + 明文 curl 实测 + 打包 JS 静态读

## 0. 总结

* 对话走 `POST /api/agent/v3/llm_utils_chat`, 官方该路径为密文请求, **但服务端有明文直通分支**,
  故可用明文 JSON 直达(依据: 认证先于 body 解析, 带 JWT + `app_id` 即返回 200)。
* 上游是 trae **私有协议**, 不是 OpenAI 语义: `messages[].content` 为对象数组,
  `tools[].function.parameters` 需字符串化, `tool_calls` 键名为 `function_call`。
* UA 是**准入标签**: 对话必须用 `TraeClient/TTNet`, 换成 curl 默认 UA 会按限流拒绝。

### 本协议不支持, 但Chat Completions有的功能

请求侧**须忽略**(不转发给上游):

* `temperature` / `top_p` / `top_k` / `min_p`: 上游宽松绑定, 接受但不生效
* `presence_penalty` / `frequency_penalty` / `logit_bias` / `logprobs` / `seed` / `stop` / `n` /
  `response_format` / `modalities` / `audio` / `stream_options`: 上游目录无这些字段名
* `tools[].function.strict`: 上游 tool schema 只有 `name`/`description`/`parameters`
* `tool_choice`: 上游**有**该字段(string 类型), 但 `"none"`/`"required"`/指定函数**全部不生效**,
  行为与不传一致 → 本实现不发送
* `functions` / `function_call`(请求顶层旧形态): 上游顶层只有 `tools`

响应侧作为扩展字段透传:

* `delta.reasoning_content`: CC 无标准位, 按业界通行命名透传

**`stream: false` 上游不支持**: 即使传 `false` 仍回 `text/event-stream` + 同样的 SSE 帧,
故本实现恒发 `stream:true`, 非流式在本地聚合。

思考强度: `llm_utils_chat` 的 Go 结构体**没有**相关字段(类型错配探针证实), 本实现不发送、
下游传入一律忽略。官方档位走的是 Js→Rust IPC, 不是 HTTP。

### 本协议与官方客户端的出入

* 对话端点 → 官方主对话走 Js→Rust IPC(`lite.send_message`), 本实现只能用明文 HTTP
  `llm_utils_chat`。代价: 无法送达思考强度。
* 请求体不加密 → 官方发 base64 密文, 本实现发明文; 服务端容忍明文。
  UA 只作限流分桶标签, 不承担鉴权。
* 不实现会话落库 → 每请求自带完整 messages, 无跨请求会话态。
* `finish_reason` 改写 → 上游**工具轮次也只回 `stop`**, 本实现在本轮产出 tool_calls 时
  改写为 `tool_calls`(符合 CC 规范)。不改写则依赖该字段的客户端会提前结束 agent 循环。
* 响应 `id`/`created` 由我方生成 → 上游响应无 id 与时间戳, 只有 `event:metadata` 的内部 `session_id`。
* `app_id` 取常量 `6eefa01c-1036-4c7e-9ca5-d891f63bfcd8` → 抓包请求体是密文, 无法从流量验证;
  放 body `app_id` 或头 `X-App-Id` 均实测通过。
* `function` 取 `solo_agent` → 该字段是**服务端模型准入闸门**, 不是可选装饰;
  官方值看不到(体密文), 依据见 §4。
* 图片不走官方上传链路 → 官方是「上传 TOS + 多模态模型转文字描述」四步;
  本实现直接用 CC 的 `image_url`(含 `data:` URL)内联直发, 实测有效且能力更强。
* 流内错误映射为 HTTP 状态码 + 裁剪 message → 详见 §7。
* 放弃的上游能力 → 自主 agent 循环(`create_agent_task`)、服务端 PE 与行为开关、
  `access.identity_list` 准入、计费倍率等 CC 无法表达的项。

### 域名

|域名和端点|功能|
|-|-|
|`api5-normal.mchost.guru/api/agent/v3/llm_utils_chat`|对话(本实现主路径)|
|`api5-normal.mchost.guru/api/ide/v1/batch_get_detail_param`|模型目录|
|`api.trae.cn/trae/api/v3/oauth/ExchangeToken`|登录换令牌 / 续期|
|`api.trae.cn/trae/api/v2/ug/*`|签到|
|`api.trae.cn/trae/api/v2/pay/ide_user_ent_usage`|余额|
|`www.trae.cn/authorization`|登录页(浏览器打开)|
|`api.trae.com.cn/icube/api/v1/{user,native/config/query}`|设备登记、远端配置|

其余端点(`create_agent_task`、`workflow/*`、`get_resource_upload_url`、`batchInsert` 等)
本实现不用。注意签到/余额在 `api.trae.cn`, 登录在 `api.trae.com.cn`(签到走 `api.trae.com.cn`
会回 404), 不要统一。

## 1. 登录方式

自动 PKCE(`S256`), `client_id` 固定 `ono9krqynydwx5`。

```text
1) 浏览器打开：
   https://www.trae.cn/authorization?login_version=1&auth_from=trae&login_channel=native_ide
     &plugin_version=2.3.102387&auth_type=local&client_id=ono9krqynydwx5&redirect=0
     &login_trace_id=<uuid>&auth_callback_url=http://127.0.0.1:40909/authorize
     &machine_id=<64hex>&device_id=<num>&x_device_id=<num>&x_device_type=linux
     &x_app_version=3.4.18&x_app_type=stable
     &code_challenge=<b64url43>&code_challenge_method=S256&channel_name=common

2) 授权后浏览器跳回 auth_callback_url, 查询串直接含授权码：
   ?isRedirect=true&scope=trae
    &authCodeInfo={"AuthCode":"<44 字符>","ExpireAt":<ms>,"ExpireDuration":600000}
    &loginTraceID=<uuid>&host=https://api.trae.com.cn&userRegion=cn&userInfo={…}
   → 直接取 authCodeInfo.AuthCode, 无需再调 GetPCAuthCode(那个需要网页 Cookie)。

3) POST <回调返回的 host>/trae/api/v3/oauth/ExchangeToken   （无认证）
   {"ClientID":"ono9krqynydwx5","AuthCode":"…","CodeVerifier":"…",
    "DeviceInfo":{"DeviceID","MachineID","PlatformCode":"IDE_PC","DeviceType":"PC",
      "DeviceName","DeviceModel":"","ClientVersion":"3.4.18",
      "DevicePublicKey":"<PEM EC prime256v1>","DeviceBrand":"","DeviceCPU":"…",
      "OSInfo":"linux","OSVersion":"Arch Linux"},
    "IDEVersion":"3.4.18"}
   → Result.{Token, RefreshToken, RefreshExpireAt, BoundDeviceID, ClientID, DeviceBindStatus}
```

* 回调端口 `40909`(官方硬编码)。
* `machine_id`(64hex) / `device_id`(数字) / `DevicePublicKey` 同源于本机一对 EC P-256 密钥,
  **必须一起持久化**, 不能每次重新生成(服务端绑定设备, `DeviceBindStatus: BOUND`)。
* `Token` 是 RS256 JWT, 载荷 `{"data":{…}}`, 有效期 **14 天**; `RefreshExpireAt - iat` ≈ 180 天。

存储: `token` / `refresh_token` / `refresh_expire_at` / `machine_id` / `device_id` /
`device_private_key_pem`(须与已绑定公钥配对, 续期要用) / `checkin_device_id`
(签到专用设备号, 见 §6.1, 缺省即自动探测)。

## 2. 令牌续期方式

```text
POST https://api.trae.cn/trae/api/v3/oauth/ExchangeToken   （无认证）
{"ClientID":"ono9krqynydwx5","ClientSecret":"","RefreshToken":"…",
 "DeviceInfo":{…同登录, DevicePublicKey 为同一密钥的公钥…},
 "DeviceProof":{"Signature":"<b64 DER>","Timestamp":<秒>,"Nonce":"<32 hex>"},
 "IDEVersion":"3.4.18"}
```

* `DeviceProof` 签名消息: `["POST","/trae/api/v3/oauth/ExchangeToken",ClientID,RefreshToken,
  Timestamp,Nonce].join("\n")`, 用设备私钥 ECDSA-SHA256 → DER → 标准 base64。
  **注意 signing payload 是 RefreshToken 本身**, 不是 JSON body。
* 最小必填集实测为 `{ClientID, RefreshToken}` + 上述 DeviceProof 结构; `GrantType` 会导致 10101。
* **控制面是另一套信封**(与本协议其余接口的 `{"code":…}` 不同):
  `{"ResponseMetadata":{…,"Error":{"Code","Message",…}},"Result":{…},"success","logId"}`,
  错误解析须单独分支。`400 Code:"10101"` = 参数无效, `401 Code:"20101"` = 令牌无效。
* `Token` 过期时对话接口回 `401 {"code":1001}`。**我方策略**: `exp` 前主动续期,
  401 兜底触发续期后重试一次。官方客户端不因 401 续期(按本地 `exp` 判断), 属我方设计。
* ⚠️ 旧 `RefreshToken` 在续期后是否作废**未验证**, 故实现按「无条件写回新值」处理
  (两种情况下都正确)。

## 3. Chat Completions映射

### 3.1 请求头

`POST https://api5-normal.mchost.guru/api/agent/v3/llm_utils_chat`

| 头 | 必需性 | 缺失后果 |
|-|-|-|
| `user-agent: TraeClient/TTNet` | **强制** | 换其他 UA → HTTP 200 + 流内 `event:error code:4011`(限流) |
| `authorization: Cloud-IDE-JWT <jwt>` | 强制 | `401 {"code":1001}` |
| `x-app-version` + `x-app-version-code` + `x-ide-version` + `x-ide-version-code` | 四者齐全 | 缺一律 `400 expr_path=app_version_code` |
| `content-type: application/json` | 强制 | — |
| `x-machine-id` / `x-device-id` | 不强制 | 照抄官方 |
| `accept: */*`、`x-device-cpu`、`x-device-type`、`x-os-version`、`x-flow-traceparent` | 照抄官方 | — |

版本常量: `x-app-version` = `x-ide-version` = `3.4.18`, 两个 version-code 均为 `20261110`,
`x-device-type` = `linux`(不是 `PC`)。官方**不发** `X-Cloudide-Token` / `X-Ide-Token` / `X-Uid`。

⚠️ **实现陷阱**: 若用 HTTP 库的便捷 JSON 方法又手动设 `content-type`, 会发出两个 `Content-Type` 头,
上游报 `400 expr_path=app_id, cause=missing required parameter` —— **报错字段误导, 真实原因是重复头**。
必须手动序列化后只设一次头。

### 3.2 请求映射

| Chat Completions | 上游 body |
|-|-|
| `model` | `model` + `config_name` 同值(剥供应商标识前缀); 大小写按目录 `config_name` |
| — | `app_id: "6eefa01c-1036-4c7e-9ca5-d891f63bfcd8"`(必填) |
| — | `function: "solo_agent"`(**模型准入闸门**, 见 §4) |
| `stream` | 恒 `true`, 非流式由本地聚合 |
| `messages[].role:user` | `{role:"user",content:[{type:"text",text:"…"}]}` (**不能**是字符串) |
| 图片 | `content[]{type:"image_url",image_url:{url}}`, `url` 支持外链与 `data:` URL |
| `messages[].role:system` | `{role:"system",content:[{type:"text",text:"…"}]}`, 实测被上游消费 |
| assistant + tool_calls | `content:[]` + `tool_calls:[{id,type:"function",function_call:{name,arguments}}]` |
| `messages[].role:tool` | `{role:"tool",tool_call_id,name,content:[{type:"text",text:"…"}]}` |
| `tools[].function.parameters` | **必须字符串化 JSON**(对象会报 unmarshal 错误) |

对话 body 形态:

```json
{"app_id":"6eefa01c-1036-4c7e-9ca5-d891f63bfcd8",
 "model":"glm-5.2","config_name":"glm-5.2","function":"solo_agent","stream":true,
 "messages":[{"role":"user","content":[{"type":"text","text":"你好"}]}]}
```

⚠️ 图片的 `image_url` 必须是**对象**(`LLMRawMessageImageUrl` 类型); 传字符串或
`{type:"image",url:…}` 均失败。外链不可达时回流内 `3003 model service is unavailable`。

### 3.3 响应映射

上游是标准 SSE 分帧 `id:N\nevent:<name>\ndata:<json>\n\n`, 按空行切分即可, 无需第三方库。

| 上游 event | data | → Chat Completions |
|-|-|-|
| `metadata` | `{model:"", session_id, prompt_completion_id, …}` | 首 chunk: `id`/`created`/`model` 本地回填, `delta.role="assistant"` |
| `timing_cost` | `{name, provider_model_name, 各段耗时}` | 丢弃(`provider_model_name` 是内部服务名, 不外泄) |
| `output` | `{response, reasoning_content, tool_calls, multimodal_contents, phase}` | `delta.content` ← `response`; `delta.reasoning_content` ← `reasoning_content`; `delta.tool_calls` ← `tool_calls` |
| `extra_info` | `{model, version, content, input_token, expire_time}` | 丢弃 |
| `token_usage` | `{prompt_tokens, completion_tokens, total_tokens, reasoning_tokens, cache_read_input_tokens, cache_creation_input_tokens, …}` | 填 `usage`; `cached_tokens` ← `cache_read_input_tokens`; `reasoning_tokens` ← `reasoning_tokens` |
| `done` | `{finish_reason:"stop"}` | `finish_reason`(有 tool_calls 则改写, 见 §0), 随后 `data: [DONE]` |
| `error` | `{code, message, extra:null}` | 见 §7, **必须先于内容帧判断** |
| `progress_notice` | **裸字符串**(非 JSON, 如 `"Processing_1794708540"`) | 丢弃; 解析须容忍非 JSON 帧 |

`output` 帧规则:

* `response` 与 `reasoning_content` **同帧互斥填充**, 都是**增量**(不是累积快照)
* `tool_calls` 按 `index` 累积: 首帧带 `index/id/type/function_call.name`, `arguments` 为空串;
  后续帧 `id:"" type:"" name:""`, 只在同 `index` 上补 `arguments` 片段
* `multimodal_contents` / `phase` 实测恒 `null`

## 4. models端点映射

```text
POST https://api5-normal.mchost.guru/api/ide/v1/batch_get_detail_param   （明文）
Headers: x-app-id: 6eefa01c-1036-4c7e-9ca5-d891f63bfcd8
{"functions":["personal_assistant","assistant","solo_agent","solo_agent_lite","solo_coder",
  "solo_agent_remote","solo_work_lite","solo_work_remote","solo_design_lite",
  "solo_design_remote","builder"],
 "agent_type":"","current_config_info":{"config_name":"","is_custom_model":false},
 "mode_type":0,"access_type":1,"ab_force_vids":"","ab_autotest_advanced_mode":0}
```

⚠️ **准入靠请求头 `x-app-id`**: 缺失时 HTTP 仍 200, 但每个 function 回
`{"error_code":2001,"error_message":"Config detail for function: <fn> not found"}`,
响应仅 ~1.5KB。**必须显式检查 `error_code`**, 只看状态码会静默得到空模型列表。
其余抓包头逐个补入均无效。与对话接口的差异: 对话把同一值放 body `app_id`, 本接口放请求头。

解析规则:

* 取 `function == "solo_agent"` 且 `usage == "chat_completion"` 的条目
  (与本协议 §3.2 的 `function` 定稿一致; 选错 function 会导致对话回 `4001`)
* 再过滤 `is_invisible_to_user != true`(缺失视同 false), 结果与官方客户端 UI 列表一致
* 名字三层: 目录 `config_name` 小写 / 显示名混合大小写 / 内部 `model_name` 带 `__dev` 后缀。
  **请求侧用 `config_name`**, `/v1/models` 以 `config_name` 为 id(避免同名歧义)
* `display_contact_config` 与 `extra_config` 都是**字符串化 JSON**, 需二次解析
* 倍率: `display_contact_config.consumption_rate.data.rate`
* 上下文: `context_window_tokens.dev`
* 同一 `config_name` 可能有多套 `model_detail_list`(`__dev` = 默认上下文, `__max` = 大上下文),
  `max_tokens` 上限按 `__max` 记

⚠️ `is_invisible_to_user=true` **不表示不可用**, 只是不在官方 UI 展示; 这些模型 API 层仍可调用。

## 5. 余额获取

Host `api.trae.cn`, 头与签到相同(VSCode UA 那套, **不带** `x-machine-id`)。

```text
POST https://api.trae.cn/trae/api/v2/pay/ide_user_ent_usage
{"require_usage":true,"req_source":1}
  → {total_balance, balance_summary[{source,total_balance}], is_credits_billing,
     usage_summary{consumed_amount,total_amount,consumption_ratio},
     user_entitlement_pack_list[], trial_status}
```

对外取 `total_balance`。`req_source`: 桌面客户端 `1`, web `3`。

## 6. 签到协议

Host `api.trae.cn`。以下头为必需: `user-agent: VSCode 1.107.1 (Trae CN)`、
`authorization: Cloud-IDE-JWT <jwt>`、`x-app-version`、`x-device-id`、`x-device-type`、`x-os-version`。

2026-11-24 用 mitmproxy 抓 Windows 客户端(`Trae CN 3.4.24`)明文, 完整头为:

```text
user-agent: VSCode 1.107.1 (Trae CN)
authorization: Cloud-IDE-JWT <jwt>
content-type: application/json
x-app-version: 3.4.24        x-os-version: Windows 10 Enterprise
x-device-id: 929500238458201 x-device-type: windows
x-device-brand: 20216        x-user-region: CN
vscode-sessionid: <machine_id>          x-market-client-id: VSCode 1.107.1
x-market-user-id: <uuid>                x-lgw-req-sdk-type: 3
package-type: stable_cn      x-lscbd-aid: 787976   x-lscbd-platform: windows
x-request-id: <uuid>         x-tt-trace-id: <trace>
x-helios / x-medusa / x-neptune: <三方 SDK 生成的密钥材料>
```

其中 `x-helios` / `x-medusa` / `x-neptune` 三个头**可以不发**(本实现从不发它们,
2026-11-24 签到成功), 说明它们不参与签到校验。`x-device-brand` 也没有必填要求
(见下)。

```text
POST /trae/api/v2/ug/checkin_credits/status   {"req_source":1}     (16 字节)
  → {"checked_in":true,"code":0,"credits":100,"did_checked_in":false,
     "enable":true,"extra_credits":100,"message":"success"}

POST /trae/api/v2/ug/checkin_credits/claim    {"req_source":1}     (16 字节)
  → HTTP 200 {"code":0,"message":"success"}
```

⚠️ **body 是裸 JSON, 没有 `{"data": …}` 外壳**。这一条曾被写反过, 记在这里避免重犯:

早期版本用 `~/.config/Trae CN/monitor/parfait` 的 `sent_body_bytes` 反推客户端载荷,
看到 `status` 25 字节、`usage` 46 字节、`activity/info` 49 字节, 恰好等于
`{"data":{…}}` 的长度, 于是判定客户端包了 `data` 外壳并在 `73cc43a` 里照做。
**该推断是错的**: `sent_body_bytes` 量的是 TTNet 加密压缩后的载荷, 不是 JSON 明文。

2026-11-24 用 mitmproxy 直接抓 Windows 客户端(`Trae CN 3.4.24`)明文, 实际是:

| 接口 | 抓到的明文 | 字节 | 曾误判的 `{"data":{…}}` |
|-|-|-|-|
| `checkin_credits/status` | `{"req_source":1}` | **16** | 25 |
| `checkin_credits/claim` | `{"req_source":1}` | **16** | 25 |
| `pay/ide_user_ent_usage` | `{"require_usage":true,"req_source":1}` | **37** | 46 |
| `ug/activity/info` | `{"version_code":"3.4.24","req_source":1}` | **40** | 49 |

两种形态上游都接受(都能回 `code:0`), 但**以客户端为准, 发裸体**。

`req_source` = `ii(client) ? 2 : 1`, `ii()` 判 `packageType ∈ {SOLO_CN, SOLO_I18N,
SOLO_CN_ENTERPRISE}`。IDE 版为 `1`; 传 `3`(web) 与缺 `x-device-id` 都会回 `9004`。

⚠️ **claim 语义陷阱**: 当日已签到时再调 `claim`, 仍返回 `200 {"code":0,"message":"success"}`,
**与领取成功无法区分**。故不能靠返回值判断成败, 只能:

1. `status` 判 `enable && !checked_in`
2. `claim`
3. 复读 `status` / `ide_user_ent_usage` 确认

已实测重复调用无副作用(不扣款、不重复发分; 25 次连打全回 `code:0`)。本实现在
`checked_in == true` 时**直接跳过、不调 claim**。

字段语义: `checked_in:true` = 今日已签到(账号级); `did_checked_in:true` = **该设备**
今日参与过(设备级, 客户端 `o1e` 也把它当已签到)。实测(2026-11-23, 同一 token):

- 官方客户端 did `1694826430742544` → `did_checked_in:true`
- 工具 did 与随机 16 位数字 did → `did_checked_in:false`
- 对任意 did 单发一次 `claim`(回 `code:0`) **不会**把 `did_checked_in` 改成 true

故 `did_checked_in` 由服务端按该设备**真实的签到参与记录**写, 伪造 did 拿不到。

## 6.1 `9074`（参与用户太多）

### 已知事实

| 时间 | 场景 | 结果 |
|-|-|-|
| 2026-11-22 00:46:29Z | 工具用自己生成的 did claim | `{"code":9074,...}` |
| 2026-11-22 00:49:18Z | 用户在官方客户端手动点 | 余额 +100, 成功 |
| 2026-11-23 04:22:22Z | 同一 JWT、同一 body, 间隔 250ms 只换 did | 工具 did `8067955002856749` → `9074`; 官方 did `1694826430742544` → `code:0` |
| 2026-11-23 05:15 | 工具(仍用工具 did) | `9074` |
| 2026-11-24 01:22 | 工具改用本机客户端探测到的 did | **成功, 余额 +100, `claim.log` 首次 `success`** |
| 2026-11-24 11:18 | Windows 客户端全新设备 claim 未参与 | `status` 正常, `did_checked_in:false` (当日已签, 未测 claim) |

`9074` 不在客户端业务码表里(`jDs()` 只有 1002/2001/9004/9090/9093/9095),
`nls.messages.json[9074]` 是无关的 `"Manage"`, 客户端碰到它只会落到 default 文案
「Unable to check in. Please try again later.」。故客户端**不知道**这个码的语义。

### 归因: 设备号相关, 但机制未定

能确定: **换成「本机官方客户端在用的 did」后立刻成功**, 而同一 token、同一时刻用
本实现 `random_digits(16)` 生成的 did 会回 `9074`。

不能确定: 究竟是"did 必须由 aha 注册过", 还是"该 did 在该账号下有过真实参与记录",
或仅仅是设备号维度上的限流。已知的反对证据:

- 2026-11-23 那次 4×2 矩阵里, **失败的恰好是整串第一条**(`req_source=1` + 工具 did),
  其后 5 条全成功 —— "首请求被限流"同样能解释全部现象, 设备号与顺序共线。
- 2026-11-24 抓到的 Windows 设备 did `929500238458201` 是全新设备
  (`did_checked_in:false`), `status` 与 `activity/info` 都正常, 说明新 did 并不被
  一律拒绝。但该设备当日已签到, 没机会测 claim, 所以也不能反证。

结论: **保留"用本机官方客户端 did"这个做法**(它已被真实成功验证), 但不要再声称
已找到精确机制。`9074` 仍按瞬时拥挤退避重试, 因为即便 did 正确上游偶发也会回它。

### 关于 did 的形状（曾误判）

曾据 `1694826430742544 = 1694826430742 × 1000 + 544` 推断 did 是「毫秒时间戳 +
序号」、由服务端签发。**该推断不成立**:

| did | 来源 | 按"×1000"解出的日期 |
|-|-|-|
| `1694826430742544` | 官方客户端 (Linux 本机) | 2023-09-16 |
| `929500238458201` | 官方客户端 (Windows) | 1999-06-16 |
| `8067955002856749` | 本工具 `random_digits(16)` | 2225-08-31 |

三个日期毫无规律, 连纯随机生成的数也能套进这个"格式"。故 did 只是一串 16 位十进制
数字, 不要从形状反推来源。

### `BoundDeviceID` 不是判据

`BoundDeviceID`(base36, 如 `28j7pl18l6wkyk`) 与 did 不同源; 客户端 `main.js` 里
`grep BoundDeviceID` = 0 次, **客户端从不解析该字段**, 与签到无关。

### 设备注册（`device_register`）不能自己造 did

理想方案是服务器上不装客户端也能自助取 did, 但实测不可行:

- 端点 `https://log-klink.zijieapi.com/service/2/device_register/`
  (旧名 `log.snssdk.com/service/2/desktop/device_register/`)。
- 请求为 `content-type: application/octet-stream;tt-data=a`, **550 字节二进制密文**
  (加密 + 签名的 protobuf), 全部参数在 query 上:
  `os_version, cdid, channel, aid, device_platform, device_id, iid, tt_data=a`。
- 手工用明文 JSON 打各种参数组合, 一律回 `{"device_id":0,"install_id":0}` ——
  服务端只认那套加密载荷。
- 该端点跟签到域(`api.trae.cn`)无关, appId 是 `776330`(aha_doctor) / `787976`(Trae CN)。

故要"凭空发号"必须逆向那套加密 protobuf, 成本高且未验证。现实做法:
**装了官方客户端就自动探测, 没装就在 `credential.toml` 手填一个真客户端的 did**
(`checkin_device_id`, 见下)。

### 本实现的做法

`src/providers/trae/client_device.rs` 在本机装过官方客户端时直接复用它的 did
(同一台机器、同一账号, 本就是同一个设备), 全部只读、纯本地:

1. `User/globalStorage/storage.json` 的 `iCubeAuthInfo://icube-dc:<did>` 键名;
2. `logs/aha_log/aha_electron_*.log` 的 `device_id: <did>` / `did: <did>` 打点。

优先级: `credential.toml` 的 `checkin_device_id`(手工覆盖) → 本机探测 → 回退到
登录上报的 `device_id`。取不到(如服务器上没装客户端)时行为与旧版一致, 会退化成
「大概率 9074」, 此时可在 `credential.toml` 里手工填官方 did。

⚠️ **仅签到用这个设备号**: 对话/目录请求仍用 `device_id`, 因为 `x-device-id` 与
登录时上报的 `DeviceInfo.DeviceID` 绑在一起, 混用会牵连其他接口。

### 重试与判定口径

官方客户端的判定口径(`main.js`):

```js
// o1e(): 能否领取 —— 第一行就把任何非 0 code 判成 unknown（不给签到）
function o1e(t){ if(E0(t)) return "unknown";
    return e?.checked_in===!0||e?.did_checked_in===!0 ? "signed"
         : e?.enable===!1 ? "disabled"
         : e?.enable===!0 && e.checked_in===!1 ? "not_signed" : "unknown"; }

// E0(): 业务码分类（status 与 claim 都生效）
code===9000? … : code!==0 → {kind:"business", code, reason:
    9090 unavailable / 9102 not_signed / 1002 auth / 4001 parameters / 其它 unknown}

// a1e(): claim 成功 = HTTP 200 且无 E0 错误（即 code===0）
// fetchWithHeaders: retry ?? 0 —— 默认不重试, claim 亦显式 retry:0
```

结论: **`9074` 不能当「可领取」绕过**（客户端也不给签到）, 且**重试耗尽也不能只凭
`claim` 的错误码判失败**——必须复读 `status`。

本实现的处理:

| 响应 | 处理 |
|-|-|
| `code=0` | 正常 |
| `code=9074` | 退避重试(`status` 2 次 / `claim` 3 次, 2s/4s/8s) |
| `code=1001`(凭证失效) | → `Unauthorized`, 触发续期/重登, **不重试** |
| `code=1002`(客户端认定的鉴权码) | 同上, `Unauthorized` |
| 其它非 0 `code` | 不重试 |

不论 `claim` 成功、失败还是重试耗尽, **最后一步都是复读 `status`**:
只要它变成已签到就算本次成功(`claim` 的码只用来组装错误文案), 否则才报失败。
理由:`claim` 的返回值与真实结果无关(当日已签到时它也回 `code:0`), 按错误码直接判
失败会误杀——上游可能已经发过了(或同一账号在客户端刚领过)。

另: `o1e` 把 **`did_checked_in === true` 也算已签到**, 本实现一并判断,
避免在上游已发放的情况重复调 `claim`。

历史 bug(已修):

1. 旧实现 `status.check()` 见 `code != 0` 即报「上游响应异常」——方向对但**没区分**码,
   且 `claim` 的 9074 同样直接失败、不重试。
2. 早期版本曾把 9074 当作「忽略即可」——错, 客户端也不给签到。
3. 旧实现 `claim` 一报错就 `?` 直接返回, **不复读 status**——已改为以 status 为准。
4. `73cc43a` 曾把 body 包成 `{"data":{…}}`——**推断错误, 已回滚**(见 §6)。
5. `x-device-id` 用随机设备号会被回 `9074`——改用本机官方客户端 did 后成功。

## 7. 上游异常响应

**两类, 第二类是 HTTP 200, 只看状态码会漏。**

### 7.1 HTTP 层错误

| HTTP | 上游 body | 触发 | → 下游 |
|-|-|-|-|
| `401` | `{"code":1001,…}` | 无 authorization / 伪造 JWT | `401` + `authentication_error`, 并触发本地续期 |
| `400` | `{"code":4001,"message":"bad request: binding: expr_path=<字段>, cause=missing required parameter"}` | 缺版本头 / 缺 app_id / 重复 Content-Type 头 | `400` + `invalid_request_error`, `expr_path` 写进 message |
| `400` | `{"code":4001,"message":"…cannot unmarshal <类型> into Go struct field <结构>.<路径>…"}` | 类型写错(如 `parameters` 传对象) | 同上 |

⚠️ 若请求本应齐全却报 `expr_path=app_id`, 优先怀疑**重复 Content-Type 头**(§3.1),
不是 body 缺字段。

### 7.2 SSE 流内错误(HTTP 200)

上游在业务/限流错误时**照样回 HTTP 200 + SSE**:

```
id:1
event:error
data:{"code":4011,"message":"…exceeded the rate limit…","extra":null}

id:2
event:done
data:{"finish_reason":"stop"}
```

| `code` | 含义 | 触发 |
|-|-|-|
| `4011` | 限流 | UA 不是 TTNet(限流按 UA 分桶) |
| `4001` | 参数无效 | 假模型名 / 图片形态错 / **模型不在所给 `function` 的目录内** |
| `3003` | 模型服务不可用 | 图片外链不可达; message 里塞了内部 URL 与 Go 网络错误 |

⚠️ `done` 帧仍是 `finish_reason:"stop"`, **不能靠 `done` 判断成败**, 必须在解析时拦截 `event:error`。

下游策略(判定依据用「是否已产出内容帧」, **不用 `error` 的 id**——`3003` 前可能有 `progress_notice`):

1. `error` 之前无任何 `output` 帧 → 尚未发 200, 直接映射 HTTP 错误:
   `4011` → `429` + `rate_limit_error`; `4001` / `3003` → `400` + `invalid_request_error`;
   未观测到的 code → `502` + `api_error`, message 取首行, 原始 code 附在 `error.code`。
2. `error` 出现在 `output` 之后 → HTTP 200 已发出, 只能以 `data: {"error":{…}}` 结尾 + `[DONE]`。
3. 重试: `4011` 与 `1001` 各重试一次, 间隔递增; 不做无限重试。
4. `3003` 的 message 须**只取首行**, 丢弃后面的 URL / Go 错误 / Request id。
