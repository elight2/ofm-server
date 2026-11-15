# ofm-server

## 介绍
* [ofm插件](https://github.com/Ebony-Vinyl/dsh-our-free-model)的服务器重写版本
* 把各家llm免费额度聚合到一起, 作为一个provider对外提供api
* 自动签到: 每过`1h`检查所有供应商是否可以领取积分, 如果有就领取
* [新供应商资讯获取](https://freeegg.top/)
* 转发出的模型名称: `供应商id/模型名 (x供应商标的倍率)`

## 使用方式
* `ofm-server`: 启动服务
* `ofm-server login 供应商id`: 获取供应商的登录凭证并保存
* `ofm-server claim`: 签到所有供应商
* `ofm-server status 供应商id`: 获取供应商当前信息（目前只输出余额；接入状态、最后签到时间、令牌到期时间为规划项）
* 配置文件: `运行目录/config.toml`

## 供应商信息
|名称|id|厂家|接入方式|备注|
|-|-|-|-|-|
|TRAE|trae|字节|使用客户端的api调用方式| |

## 技术信息
* 绑定ip: 默认`127.0.0.1`, 可以使用配置文件修改
* 端口: 默认`3060`, 可以使用配置文件修改
* 供应商的登录凭证存储目录: `运行目录/providers/供应商id/credential.toml`
* 使用Rust语言
* 导出api使用`OpenAI Chat Completions`格式
* 日志可适配systemd, 也支持独立运行
* 无需api key校验
