# ofm-server

* [ofm插件](https://github.com/Ebony-Vinyl/dsh-our-free-model)的服务器重写版本, 部分逻辑参考其代码
* 聚合免费大模型
* 自动签到: 定时检查签到, 领取每日积分
* [资讯获取1](https://freeegg.top/)
* [资讯获取2](https://jidan.si/)
* 供应商信息: PROVIDERS.md

## 命令行参数
* `ofm-server`: 启动服务
* `ofm-server login 供应商id`: 获取供应商的登录凭证并保存
* `ofm-server claim`: 签到所有供应商（每次签到写入 `运行目录/providers/供应商id/claim.log`，保留最近 50 条）
* `ofm-server status 供应商id`: 获取供应商当前信息(输出接入状态, 当前余额, 最近一次成功签到的时间与增量, 凭证到期时间, 模型列表)
* `ofm-server config`: 打印[默认配置文件](./config.example.toml)
* `ofm-server refresh 供应商id`: 刷新供应商登录令牌

## 配置说明
* 导出api使用`OpenAI Chat Completions`格式, 但无需填写key
* 转发出的模型名称: `供应商id/模型名 (x供应商标的倍率)`
* 配置文件: `运行目录/config.toml`, [默认配置](./config.example.toml)
* 端口: 默认`3060`
* 登录凭证存储目录: `运行目录/providers/供应商id/credential.toml`
* 签到记录: `运行目录/providers/供应商id/claim.log`
* 由于每个供应商接口差异大, 本项目没有实现自定义上下文大小限制, 走下游客户端默认大小
* 部分供应商没有获取缓存读取token数量功能, 无法在下游客户端显示缓存命中率
* 由于OpenAI协议没有获取模型推理强度列表的部分, 思考强度设置只能在软件配置文件手动改
* 部分供应商的模型表现可能与官方模型不同, 可以添加自定义系统提示词修正模型行为: `运行目录/prompts/供应商id.md`会自动加入系统提示词发送给模型, [默认版本](./default-prompts/)

## 模型修正提示词测试
* `./std-benchmark/`
* 提示词: `完成本项目`

## 技术信息
* 使用Rust语言
* 日志可适配systemd, 也支持独立运行

## 构建
* release模式: `cargo build --release`
* 产物: `./target/release/ofm-server`
