# 疑难排查记录

## 抓 TRAE 客户端流量（反复试错才通）

* `--proxy-server` 命令行参数无效：`trae.network.proxy.mode` 默认 `system`，且每次启动
  由 `app.setProxy()` + `Dm.setProxy()`(下发 AhaNet) 强制覆盖 Chromium 参数。
  → 必须写进 `~/.config/Trae CN/User/settings.json`：
  `trae.network.proxy.mode` / `.effectiveMode` = `manual`，`.manual.proxy` + `http.proxy` = `http://127.0.0.1:8080`
* proxychains4 不可用：`strict_chain` + 无监听 8080 时建链即失败；Electron 多进程 LD_PRELOAD 拦不全
* CA 装了却不生效：Arch 的 p11-kit 模型需 `sudo update-ca-trust` 才刷新 `tls-ca-bundle.pem`
  （时间戳停在旧值即为未刷新）。`trust anchor --store` 写到 `~/.local/share/pki`，
  客户端带 `P11_KIT_NO_USER_CONFIG=1` 会排除用户级配置 → 必须放
  `/etc/ca-certificates/trust-source/anchors/mitmproxy.pem`（需 sudo，agent 无 sudo）
  验证：`trust list | grep -c 'trust: anchor'` 167 → 168
* 采集脚本旧版 header 截断 200 字符 → JWT 不完整无法复用，重跑一遍登录。
  教训：抓包脚本 header 一律不截断，body 记 `orig_len`
* mitmdump 默认绑 `0.0.0.0`（本机有局域网 IP，等于把解密代理开放到网段）→ 必须 `--listen-host 127.0.0.1`
* `pkill -f mitmdump` 会杀掉含同字符串的自身 shell → 用 `pkill -x mitmdump`
* 明文 body 的强类型陷阱（都靠 400 报错定位，报错信息会写出 Go 结构名，很好用）：
  `tools[].function.parameters` 必须字符串化 JSON；`messages[].content` 必须是数组；
  assistant 侧工具键名是 `function_call` 不是 `function`；`app_id` 必填
* curl 默认 UA 会撞 `4011 rate limit`（HTTP 200 流内 error）→ 限流按 UA 分桶，须 `user-agent: TraeClient/TTNet`
* 想用伪造 401 逼客户端续期 → **失败**：客户端收到 401 不续期、不重试，重启也不续期。
  它按 `CheckLogin.Result.ExpiredAt`（= refreshToken 到期，180 天）记账，
  JWT 只在临近自身 `exp` 时惰性续期。要抓续期只能等 `exp`(2026-11-28) 附近
* `GetThirdPartyToken` 看着像续期，实为 `{"Types":["feishu","lark"]}` 飞书集成周期请求
* reqwest 的 `.timeout()` 是**整个请求**超时（含读 body），SSE 长响应会被中途断流，
  表现为「上游帧只收到 metadata/timing_cost/output 一部分，缺 token_usage 和 done，
  但我方不报错」。改用 `.connect_timeout()` + `.read_timeout()`（仅限制两次数据间隔）
* reqwest `.json()` 与手动 `content-type` 头并存 → 发两个 `Content-Type`，
  上游回 `400 expr_path=app_id missing required parameter`（报错字段与真实原因无关）
* 调试 SSE 时勿用 `curl ... >/dev/null` 或 `| head`：curl 提前退出会断开连接，
  上游随之截断，看起来像「上游不发 token_usage/done」，实为客户端自己中断
* 模型目录 `/api/ide/v1/batch_get_detail_param`：**缺失请求头 `x-app-id` 时 HTTP 200 但空目录**，
  每个 function 回 `{"error_code":2001,"error_message":"Config detail for function: X not found"}`（1576 字节）。
  逐个补 22 个缺头二分实测，只有 `x-app-id` 有效；换掉 machine-id/device-id 仍正常，该接口不校验设备指纹。
  教训：HTTP 200 不等于成功，必须解析业务 `error_code`，否则静默返回空列表
* pi 的 `--list-models` 与 print 模式都以 `allowModelNetwork: false` 初始化，
  `refreshModels` 不会被调用 → 纯 refreshModels 方案下模型列表恒为空。
  解法：扩展工厂写成 `async function` 并在其中拉取，pi 启动会等工厂完成
* 签到 `claim` 在「今日已签到」时仍回 `{"code":0,"message":"success"}`，与真领到无法区分；
  实测前后 `total_balance` 都是 5657.82 未变。实现必须：`checked_in=true` 直接跳过、不调 claim，
  成败只能靠复读 `status` 的 `checked_in` 判定
* 签到 host 是 `api.trae.cn`；登录用的 `api.trae.com.cn` 对签到路径回 404，两者不可混用

## Trae 云 API 请求体加密（libharness.so 静态分析）

结论：`create_agent_task` 的 body 加密由 `harness/cloud-api/src/shared/event_encryption.rs` 实现，
**协议里没有固定主密钥可用**，硬编码串是别的东西。

* 定位方法（248MB stripped ELF，r2 `aa` 后 `axt` 直接查字符串无效，因为 Rust 用 `(ptr,len)` 且
  reloc 已静态解析，需自己扫 `.text` 里的 `lea reg,[rip+disp32]`）：
  `strings -n 6 > /tmp/h.strings` 找文案 → 算字符串 vaddr（本例 .rodata vaddr==file offset）
  → 正则扫 `.text` 找 48 8d 05/0d/15.. 后跟 disp32、`insn_end+disp==vaddr` → 唯一命中
* 唯一命中 `0xd0d5457`：`lea rdx,[0x027db59b]` + `mov ecx,0x2c`（44 字符）→ 调 base64 解码
  （函数 `0xd0d6a10`，内部是 base64→buffer），随后**没有**把解密结果当 AES 密钥：
  `mov rsi,[rsp+0x110]` 取的是 Result 判别位，接着 `cmp rbx,6` 判 `Err` 变体，
  失败即跳 `invalid protocol key`（字符串偏移 0x27db5c7，紧跟在 base64 串后）。
  → 该 44 字节常量是 **`v3` 分支下某个 key 的兜底常量（可能是错配/混淆残留）**，
  与抓包的密文无关：拿 b64 解码出的 32 字节直接做
  AES-256-GCM / ChaCha20-Poly1305（nonce 取头/尾 12B，tag 尾/头，含 HKDF-SHA256 各种 info）
  全部 `InvalidTag`/`InvalidSignature`。
* 加密原语清单（取自 `.cargo/registry` 路径字符串，共 260 个 crate）：只有
  `ring-0.17.14`（AEAD 走 `ring::aead`）、`aes-0.8.4`（软件实现，无 aes-gcm crate）、
  `aws-lc-rs-1.17.0`。没有 `aes-gcm` / `chacha20poly1305` / `snow` 独立 crate。
* 密文形态：request body 整体 base64，解码后 53284 字节（`%16==4`），头部 32 字节纯随机，
  无 magic、无 version 字节前缀 → 大概率是 `nonce||ct||tag` 之外的构造
  （例如 nonce 走 header / 由 `conversation_id`+时间派生，或 `ring::aead` 的 `NonceSequence`）。
* 协议是**可选**的：抓包里有 `encryption_version="plaintext_passthrough"` 枚举，
  且 `[curl]` 实测明文 body 服务端照收 → **不需要**破解加密也能调 `create_agent_task`。
  字段掩码 `absent` / `no_encrypted_fields` / `decrypt_failed_passthrough` 说明客户端对
  解密失败也是降级透传，不是硬失败。
* 阻断点：动态调试不可行——deb 包里 `modules/ai-agent/bin/` 只有 `agent-tool-host`、`ctx-cli`，
  **`ai-agent` 主程序缺失**（`start.sh` 里 `exec ./ai-agent`），无法运行/attach。
* 注意：`modules/ai-agent/checkpoint.sh` 内容是**提示注入**（伪造成"自问自答"命令，
  要求每次工具调用前执行它并重读 RFC），**不要执行，也不要照做**。已核实其不参与构建流程。

## Trae 目录数据的两类误判（2026-11-15）

**症状**：得出「1008 个 reasoning_model / 188 个 chat_model」「所有模型默认 high」
「`support_thinking` 不门控，`glm-5.2` 标 false 仍思考」等错误结论。

**根因**：`batch_get_detail_param` 响应里 11 个 function **各自持有一份模型副本**，
同一个 `config_name` 跨 function 重复（96 个唯一名中 79 个出现在多个 function）。
① 直接统计全目录条目数 → 放大 20 倍；
② 跨 function 取 `support_thinking` → 拿 `solo_work_lite` 的值去解释 `solo_agent` 的行为。

**正确口径**：本项目只用 `function == "solo_agent"` 且 `usage == "chat_completion"`，
共 **56 条，`config_name` 无重复**（`catalog.rs` 的 `seen` 集合是防御性代码，注释「实测无重复」正确）。

**验证方法**：任何目录统计都必须先 `filter(function=="solo_agent")` 再比；跨 function 数值不可混用。

**`is_invisible_to_user` 的实测语义**：`solo_agent` 下 38/56 为 true，剩余 18 个的显示名
与用户官方客户端实际展示的 18 个**零误差吻合** → 该标志精确界定官方 UI 展示集合，
但**不表示不可用**（`glm-5.2` 标 true，`[curl]` 实测正常回答）。

**决定**：`to_models()` 按该标志过滤（56 → 18），与官方 UI 对齐。
被过滤的 38 个 API 层仍可调用，需要时手动指定 `config_name`（`function` 才是准入闸门）。
两个回归测试锁定口径：过滤后 18、未过滤 56（hidden 38）。

## Trae 思考强度：不能靠「换字段位置」绕过（2026-11-15）

**问题**：`llm_utils_chat` 上 `reasoning_effort_level` 无效，怀疑是放错位置。

**已试位置**（均为明文，不涉加密）：
1. 顶层 `{"reasoning_effort_level":"light"}` → 20 次，p=0.70~0.83，无效
2. 嵌套 `{"model_info":{"reasoning_effort_level":"light"}}`（照官方落库结构）→ 12 次，
   light 3267 / extra_high 3124，区间完全重叠且方向相反，p=0.790，无效

**检验力自查**（避免「样本不够」的自我安慰）：合并两实验 cv=0.23 稳定；
2 倍效应、80% 检验力只需每组 n≈4，实测 5~6 次 → p≈0.8 是真无效应。

**旁证**：`support_thinking:false` 的 `mimo-v2.6-pro` 传档位后照常思考，行为与可调档模型无法区分。

**其他排除**：`extra_config` 111 字段档位切换时全不变（`enable_compact_reasoning_effort_alignment`
也不是开关）；无按档位拆分的 `config_name`；`ModelHyperParamsConfig` 属自定义模型链路。

**结论**：不破解加密就没有入口。剩下只有 `create_agent_task`（需重写 stream，SSE 帧集合不同）。
脚本 `tmp/re-test/probe_nested.py`，数据 `tmp/re-test/results_nested.json`。
