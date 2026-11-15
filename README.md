# ofm-server

## 介绍
* [ofm插件](https://github.com/Ebony-Vinyl/dsh-our-free-model)的服务器重写版本
* 把各家llm免费额度聚合到一起, 作为一个provider对外提供api
* 自动签到: 默认每过一定时间检查所有供应商是否可以领取积分, 如果有就领取, 每个供应商的签到间隔可以在配置文件修改
* [新供应商资讯获取](https://freeegg.top/)
* 转发出的模型名称: `供应商id/模型名 (x供应商标的倍率)`

## 使用方式
* `ofm-server`: 启动服务
* `ofm-server login 供应商id`: 获取供应商的登录凭证并保存
* `ofm-server claim`: 签到所有供应商（每次签到写入 `运行目录/providers/供应商id/claim.log`，保留最近 50 条）
* `ofm-server status 供应商id`: 获取供应商当前信息(输出接入状态, 当前余额, 最近一次成功签到的时间与增量, 凭证到期时间, 模型列表)
* `ofm-server config`: 打印默认配置文件

## 供应商信息
|名称|id|厂家|
|-|-|-|
|TRAE|trae|字节|
|商汤小浣熊|raccoon|商汤|

## 配置信息
* 配置文件: `运行目录/config.toml`
* 导出api使用`OpenAI Chat Completions`格式, 但无需填写key
* 绑定ip: 默认`127.0.0.1`, 可以使用配置文件修改
* 端口: 默认`3060`, 可以使用配置文件修改
* 供应商的登录凭证存储目录: `运行目录/providers/供应商id/credential.toml`
* 供应商的签到记录: `运行目录/providers/供应商id/claim.log`

## 技术信息
* 使用Rust语言
* 日志可适配systemd, 也支持独立运行
