# 禁止行为
* 禁止修改的文件: `./AGENTS.md`, `./README.md`, `./config.example.toml`, `./PROVIDERS.md`, `./TODO.md`
* 需要的工具未安装时，须暂停工作要求用户安装，禁止绕路或自行安装
* 未经允许, **禁止修改项目目录外文件**

# 资料文件
* 项目内, 临时目录使用`./tmp/`
* 供应商官方客户端：`~/tools/docs/agent-clients/`
* openai api完整文档: `~/tools/docs/openai-openapi/`
* `./testdata/`: 本项目的测试目录
* `./tmp/`里有ofm插件和ofm插件重写版本的代码, **仅供参考**, server版本需重写
* `./provider-info/`: 供应商的协议分析及编写模板

# 工作记录
* 修改好一个功能后git提交文件，标明`[Committed by Agent]`，并且写清楚修改了什么
* 提交前`cargo fmt`

# 其他要求
* 功能细节不确定时询问用户，禁止猜测
* 实现某个功能前, 先搜索可用的开源库, 避免重复造轮子
* 尽可能使用rust原生库
* 代码实现需跨平台(Linux/Windows)

# 测试

* 修改代码后, 必须进行构建测试(仅需测试Linux版本)
* 添加新的供应商实现后, 必须进行功能测试(仅需测试Linux版本)

## 构建测试
* 使用`cargo`构建项目, 无报错, 无警告
* 在`testdata`中程序正常启动

## 功能测试
* 使用`ssh`在`test`用户下测试pi, 按照[维护标准](./MAINTAIN.md)测试每个功能是否正常使用
* test已经配置[ssh私钥](`~/.ssh/test`)登录
* **禁止使用所在用户**测试pi
