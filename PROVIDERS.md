# 供应商信息

## 基本信息
|编号|名称|id|厂家|特殊情况|
|-|-|-|-|-|
|1|TRAE|trae|字节|kimi pro模型和qwen4max是会员专属, 不能用; 模型严重依赖修复提示词, 否则工作效果很差|
|2|商汤小浣熊|raccoon|商汤|限流严重|
|3|华为云 CodeArts Agent|codearts|华为云|部分模型是"限时体验", 不一定长期可用|
|4|MiniMax Code|minimax|MiniMax|MiniMax-M2.7不可用|
|5|Loomy|loomy|讯飞|限流稍严重|
|6|WorkBuddy中国版|workbuddy|腾讯|小心风控|

## 功能信息
* 具备的功能写`x`, 不具备的写空` `

|编号|id|图片识别|工具调用|倍率信息|签到|令牌刷新|
|-|-|-|-|-|-|-|
|1|trae|x|x|x|x|x|
|2|raccoon|x|x|x|x| |
|3|codearts|x|x|x|x|x|
|4|minimax|x|x| |x|x|
|5|loomy|x|x| |x| |
|6|workbuddy|x|x|x|x|x|

备注:
* 图片识别: 六家均已透传 `image_url`, 但**能否识别取决于模型**:
  codearts 的 `GLM-5.2` / `openpangu-2.x` 实测回 `The request model is not multimodal`;
  loomy 仅 `capabilities.vision` 为真的模型可收图; minimax 上游只收 base64(外链图片会被丢弃)。
* 倍率信息: minimax 目录无倍率字段, loomy 上游目录亦无倍率字段, 两者 `rate` 恒为 `None`。
* 令牌刷新: raccoon 上游 `/refresh` 原样返还 token(不延长有效期), loomy 无续期机制;
  两者 `refresh` 均返回 `Unsupported`, 到期需重新 `login`。
* 签到: raccoon 无传统签到端点, 用移动端登录领积分端点代替(每日需真实登录触发)。
