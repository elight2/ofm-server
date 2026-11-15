# 客户端配置

## pi

`pi-ofm-provider.ts` 是 pi 的 provider 扩展，从 ofm-server 的 `/v1/models`
**自动发现**模型，无需手写模型清单。

安装（目标用户自己的 agent 目录，不随项目提交）：

```sh
mkdir -p ~/.pi/agent/extensions
cp client-config/pi-ofm-provider.ts ~/.pi/agent/extensions/ofm-provider.ts
```

注意事项：

* 服务地址写死在脚本顶部（`http://127.0.0.1:3060/v1`），改了端口要同步调整。
* 不能只用 `models.json` + `refreshModels`：pi 的 `--list-models` 和 print 模式都以
  `allowModelNetwork: false` 初始化，不会调用 `refreshModels`，列表恒为空。
  因此脚本把拉取放在 **async 扩展工厂**里（pi 启动会等工厂完成）。
* 启动顺序：先起 ofm-server，再起 pi。服务未启动时扩展只打印告警，pi 仍能启动。
