# ofm-server

## 介绍
* ofm插件的服务器版本
* 把各家llm免费额度汇总到一起, 作为一个provider对外提供api
* 自动签到: 每过`1h`检查所有供应商是否可以领取积分, 如果有就领取

## 使用方式
* `ofm-server`: 启动服务
* `ofm-server login 渠道id`: 获取供应商的登录凭证并保存
* `ofm-server claim`: 签到所有供应商

## 技术信息
* 绑定ip: `127.0.0.1`
* 端口: `3060`
* 供应商的登录凭证存储目录: `运行目录/providers/供应商/credential.toml`

