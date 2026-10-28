CPR 插件“账号探测”：ModelTrace 指纹与测票，主动探测 OpenAI OAuth 账号是否被降智。

- 要求 Codex Proxy RS `>=3.34.0, <4.0.0`
- 在 CPR 的“插件管理”选 GitHub 来源，填 `Milesians/cpr-turnstate` 安装；按 CPR 服务器平台选择 `x86_64` 或 `aarch64` 的包
- 安装后在插件设置里打开“发出真实探测”才会发送请求；探测会消耗账号的真实额度
