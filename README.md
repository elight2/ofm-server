# ofm-server

## 介绍
* ofm插件的服务器版本
* 把各家llm免费额度聚合到一起, 作为一个provider对外提供api
* 自动签到: 每过`1h`检查所有供应商是否可以领取积分, 如果有就领取
* [新供应商资讯获取](https://freeegg.top/)

## 使用方式
* `ofm-server`: 启动服务
* `ofm-server login 供应商id`: 获取供应商的登录凭证并保存
* `ofm-server claim`: 签到所有供应商

## 供应商信息
|名称|id|厂家|接入方式|备注|
|-|-|-|-|-|
|TRAE|trae|字节|使用客户端的api调用方式| |

## 技术信息
* 绑定ip: `127.0.0.1`
* 端口: `3060`
* 供应商的登录凭证存储目录: `运行目录/providers/供应商id/credential.toml`
* 使用Rust语言
* 模型名称: `供应商id/模型名`
