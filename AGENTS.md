# Delivery First                                                                                               
- Ship first, refine later: get a usable artifact out, then add detail.                                         
- Once you judge "that's enough", your next output must be the artifact itself — not another check.             
- Reuse findings you already have; never repeat an identical operation to confirm.                              
- At most two verifications per question; take everything you need from one source in one pass.                 
- "Just simple / that's enough / stop" closes the task: note new doubts, don't chase them. 

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

# 其他要求
* 软件功能的细节不确定时询问用户，禁止猜测
* 实现某个功能前, 先搜索可用的开源库, 避免重复造轮子

# 测试

* 修改好一个功能后, 必须进行构建测试

## 构建测试
* 使用`cargo`构建项目, 无报错, 无警告
* 在`testdata`中程序正常启动

## 功能测试
* 使用ssh或su在`test`用户下运行pi, 按照测试标准测试api是否正常使用
* test已经可以使用ssh私钥登录(`~/.ssh/test`)
* 测试前编辑`test`用户的pi配置, 添加ofm-server的本地api
* 按照`./MAINTAIN.md`测试供应商是否正常工作
* **禁止使用工作用户**测试pi
