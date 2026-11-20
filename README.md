# ofm-server

* [ofm插件](https://github.com/Ebony-Vinyl/dsh-our-free-model)的服务器重写版本, 部分逻辑参考其代码
* 聚合免费大模型
* 自动签到: 定时检查签到, 领取每日积分
* [领鸡蛋1](https://freeegg.top/)
* [领鸡蛋2](https://jidan.si/)
* 供应商信息: PROVIDERS.md

## 命令行参数
* `ofm-server`: 启动服务
* `ofm-server login 供应商id`: 获取供应商的登录凭证并保存
* `ofm-server claim`: 签到所有供应商（每次签到写入 `运行目录/providers/供应商id/claim.log`，保留最近 50 条）
* `ofm-server status 供应商id`: 获取供应商当前信息(输出接入状态, 当前余额, 最近一次成功签到的时间与增量, 凭证到期时间, 模型列表)
* `ofm-server config`: 打印[默认配置文件](./config.example.toml)
* `ofm-server refresh 供应商id`: 刷新供应商登录令牌

## API配置
* 导出api使用`OpenAI Chat Completions`格式, 但无需填写key
* 服务端口: 默认`3060`
* 转发出的模型名称: `供应商id/模型名 (x供应商标的倍率)`

## 功能说明
* 配置文件: `运行目录/config.toml`, [默认配置](./config.example.toml)
* 登录凭证存储目录: `运行目录/providers/供应商id/credential.toml`
* 签到记录: `运行目录/providers/供应商id/claim.log`

### 局限性
* 由于每个供应商接口差异大, 没有实现对上下文限制的获取和思考强度的修改
* 部分供应商没有获取缓存读取token数量功能, 无法在下游客户端显示缓存命中率

### 修正提示词
* 部分供应商的模型表现可能与官方模型不同, 可以添加自定义系统提示词修正模型行为: `运行目录/prompts/供应商id.md`会自动加入系统提示词发送给模型(如果放置同名空文件则视为不添加修正提示词), [每个供应商的默认版本](./default-prompts/)

#### 模型修正提示词测试
* 让官方模型和供应商提供的模型完成同一个项目, 对比其工作log, 得到模型变化, 从而写出修正提示词
* 测评项目: `./std-benchmark/`
* 只说一句: `完成本项目`

## 技术信息
* 使用Rust语言
* 日志可接入systemd, 独立运行时输出到终端

## 构建

### Linux
* release模式: `cargo build --release`
* 产物: `./target/release/ofm-server`

### Windows
* `cargo xwin build --release --target x86_64-pc-windows-msvc`
* 产物: `./target/x86_64-pc-windows-msvc/release/ofm-server.exe`
