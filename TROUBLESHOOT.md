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
