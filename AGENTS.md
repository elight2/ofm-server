# 禁止行为
* 禁止修改`AGENTS.md`
* 禁止修改`README.md`
* 需要的工具未安装时，须暂停工作要求用户安装，禁止绕路或自行安装
* 未经允许, **禁止修改项目目录外文件**

# 工作文件
* `./TROUBLESHOOT.md`：疑难排查记录
* `./MAINTAIN.md`: 插件维护标准
* `./provider-info/`: 各种供应商的协议分析以及写分析的模板

# 资料文件
* 项目内, 临时目录使用`./tmp/`
* 供应商官方客户端：`~/tools/docs/agent-clients/`
* openai api完整文档: `~/tools/docs/openai-openapi/`
* `./testdata/`是测试ofm-server用的目录
* `./tmp/`里有ofm插件和ofm插件重写版本的代码, **仅供参考**, server版本需重写

# 工作记录
* 修改好一个功能后git提交文件，标明`[Committed by Agent]`，并且写清楚修改了什么
* 如果一个问题经过反复试错才修复成功，记录在`./TROUBLESHOOT.md`，对于简单的问题不记录

# 其他要求
* 软件功能的细节不确定时询问用户，禁止猜测
* 各种记录文件用语简练，仅记录重要内容
* 实现某个功能前, 先搜索可用的开源库, 避免重复造轮子

# 测试

* 修改好一个功能后, 必须进行构建测试

## 构建测试方式
* 使用`cargo`构建项目, 无报错, 无警告(无法消除的警告须让用户同意)
* 在`testdata`中程序正常启动

## 功能测试方式
* 使用ssh或su在`test`用户下运行pi, 按照测试标准测试api是否正常使用
* test已经可以使用ssh私钥登录(`~/.ssh/test`)
* 测试前编辑`test`用户的pi配置, 添加ofm-server的本地api
* **禁止使用工作用户**测试pi
