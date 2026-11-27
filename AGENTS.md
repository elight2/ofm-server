# 禁止行为
* 禁止修改的文件: `./AGENTS.md`, `./README.md`, `./config.example.toml`, `./PROVIDERS.md`, `./TODO.md`
* 需要的工具未安装时，须暂停工作要求用户安装，禁止绕路或自行安装
* 未经允许, **禁止修改项目目录外文件**

# 资料文件
* 工作时, 临时目录使用`/tmp/ofm-server/`, **禁止使用/tmp下其他位置**, 并且在工作结束后**清空**
* 供应商官方客户端：`~/tools/docs/agent-clients/`
* openai api完整文档: `~/tools/docs/openai-openapi/`
* `./testdata/`: 本项目的测试目录, 其中测试用凭证由用户提供, **完成工作后清空**
* `./provider-info/`: 供应商的协议分析及编写模板

# 工作记录
* 修改好一个功能后git提交文件，标明`[Committed by Agent]`，并且写清楚修改了什么

# 代码要求
* 实现某个功能前, 先搜索可用的开源库, 避免重复造轮子
* 尽可能使用rust原生库
* 代码需跨平台(Linux/Windows)
* 及时清除不再需要的死代码
* 修改代码后及时修改注释, 保证正确

# 其他要求
* 功能细节不确定时询问用户，禁止猜测
* kill旧的测试进程时检查pid,**禁止停止系统中原本运行的ofm-server**

# 测试

* 修改代码后, 必须进行构建测试
* 添加新的供应商实现后, 必须进行功能测试(仅需测试Linux版本)
* 涉及refresh令牌的测试, refresh成功后必须使用新令牌覆盖旧令牌保存, 防止丢失测试凭证
* **被测程序的运行目录必须是./testdata/, 禁止在项目根目录运行**

## 构建测试
* 执行`cargo fmt && cargo clippy --all-targets -- -D warnings && cargo build --release`
* 在`testdata`中程序正常启动
* Windows版本测试: 使用用户的默认wine prefix尝试启动Windows版程序, 启动成功即可

## 功能测试
* 使用`ssh -i ~/.ssh/test test@127.0.0.1`测试pi, 按照[维护标准](./MAINTAIN.md)测试每个功能是否正常使用
* **禁止使用所在用户**测试pi
