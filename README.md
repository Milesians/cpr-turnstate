# 账号探测（CPR 插件）

[Codex Proxy RS](https://github.com/zyycn/codex-proxy-rs)（CPR）的插件：主动探测 CPR 里的 OpenAI OAuth 账号有没有被降智。页面在 CPR 的“插件”导航里打开，不需要另外部署前端。

插件 ID `milesians.cpr-turnstate`（发布者 `milesians`），要求 CPR `>=3.34.0, <4.0.0`（清单 v2、RPC v7、`management` v2、`maintenance` v1）。SDK 和打包 CLI 都固定在 CPR 的 `v3.34.0`（`47d30474af7c3e1fab433f2d4a70430fae548ff2`）。

## 两种探测

| | 测票 | ModelTrace |
| --- | --- | --- |
| 做什么 | 先拿一张 `X-Codex-Turn-State` 票，再带回最新的票最多 3 次（KlN-4096/sub2api v0.2.17-klno.1 的回声判据） | 发 3 道“选 N 个 1 到 355 的整数”的题，按数字分布和 [ModelTrace](https://github.com/xqy2006/ModelTrace) 指纹库比对 |
| 怎么发 | `host.http.account`：插件按当前凭据自己写请求，直连 `chatgpt.com/backend-api/codex/responses`，走账号在 CPR 绑定的代理（没绑就直连；绑了但不可用就失败，不回退直连）。UA、Originator、Version 实时复用 CPR 全局客户端身份；多 `Session_id`、`X-Codex-Turn-State` 和 `reasoning.effort=low`，一次测票内带上游下发的 cookie | `host.model.execute`：走 CPR 自己的模型链并指定这个账号，用插件自己维护的 Client Key“账号探测（ModelTrace）”（计入它的用量；分组“账号探测”的成员由插件维护） |
| 判定 | 第一次带回就被接受 → 正常；换过票后才被接受 → 疑似；每次都被换新票 → 降智；没发票、或负载均衡 cookie 变了 → 未判定 | 探测的模型概率 ≥ 50% 且最像它 → 正常；更像另一个模型且概率 ≥ 80%（至少 2 份有效回答）→ 降智；其余疑似；指纹库没有的模型 → 未判定。`gpt-6.1-sol` 按 `gpt-6-astra` 的指纹判定 |

上游明确说自己过载（消息里有 overload）→ 疑似，计入状态条和正常率；其他失败（包括没有 overload 字样的 503）是探测失败、未判定。上游的模型名是它自己的说法，只记录、不参与判定。票是会话凭据，只存长度和 sha256 前 8 位。

## 连续测票判定与 TG 通知

- **当前判定与单次结果分开保存**：连续 3 次测票正常，当前判定才从疑似/降智恢复正常；反之，连续 3 次测票疑似或降智才确认异常。这 3 次中出现降智就判为降智，全为疑似则判为疑似。同一状态不会重复通知；疑似与降智之间的确认变化也会通知。
- 仅统计测票，手动和自动探测一起计数；ModelTrace 不参与。失败、未判定中断连续计数，但保留当前判定；切换测票模型重新计数。新账号也需 3 次确认，首次确认正常不发送恢复通知，首次确认异常会通知。
- 当前判定与连续计数持久保存，插件重启不丢失。升级前的历史以最近一次有效测票判定为初始状态，不回放旧通知；从升级后的探测开始累计 3 次。
- 列表账号状态优先显示当前测票判定，不再取所选时间范围内最差的一次；没有测票结果时使用最近一次 ModelTrace 结果。时间条、正常率和详情仍保留原始单次结果，恢复正常不会改写历史，也不会修改 CPR 账号的启停或凭据状态。

在 CPR **插件详情 → 设置** 中配置 `telegramEnabled`（TG 状态通知）、`telegramChatId`（接收用户/群组 ID 或 `@频道名`），可选 `telegramThreadId`（话题 ID，0 为不指定）；在**敏感配置**中填写 `telegramBotToken`。Token 由 CPR 的独立 secret 字段管理，不写入普通配置、探测历史或插件页面响应。私聊先向机器人发送 `/start`，群组/频道需授予机器人发送权限。插件首页的“发送 TG 测试通知”可以验证配置，即使自动通知开关关闭也可主动测试。

通知使用 Telegram 官方 `sendMessage` 接口，由 CPR 受管 HTTP 从服务器发出；服务器须能访问 `api.telegram.org`。消息包含账号名称/ID、模型、前后判定、确认时间与事件 ID，不包含 OAuth 凭据或票。发送失败不改变探测结果，后台维护会按 1 分钟起、最长 1 小时的退避重试，并遵守 Telegram 的 `retry_after`；详情页显示待发送数量与脱敏错误。每账号最多保留 32 条通知，超限丢弃最旧项并显示计数。关闭通知会清空待发队列，再开启不补发关闭期间事件。正常情况下每次状态切换只发送一次；极端情况下 Telegram 已接收但进程在确认落库前退出，重试可能重复，事件 ID 可用于识别。

## 账号与批量配置

- 默认模型：测票 `gpt-6-luna`，ModelTrace `gpt-6.1-sol`。账号已有的模型选择继续保留；空模型回退到该方法的默认模型
- 每行齿轮进入账号配置：两种方法分别设置自动探测开关、模型和间隔，修改后直接保存；立即探测弹窗也能切换方法与模型，切换时各自保留本次选择
- 勾选账号后点“批量配置”，每批最多 200 个；支持全选当前搜索结果，分别选择方法、模型、开关和间隔。不指定的项目保持各账号原值，整批通过校验后一次保存，不发起立即探测
- v0.1.2 的插件 ID 改为 `milesians.cpr-turnstate`，发布者改为 `milesians`。CPR 按 ID 区分插件，新 ID 需要新安装，不能在旧 `ezou.account-probe` 实例上切换版本；请先停用旧实例，旧设置和历史保留在旧实例里，不会自动迁入新 ID

## 安全闸

手动和自动探测走同一套（`backend/src/runner.rs`）：

- 插件设置“发出真实探测”默认关闭，关闭时什么也发不出去
- 只探测启用中的 OpenAI OAuth 账号；凭据状态是 expired/banned/invalid 的不探测
- **不刷新令牌**：访问令牌已过期或 30 秒内过期就拦截，等 CPR 自己刷新
- 同账号串行，全局最多 8 个并发
- 自动探测同账号同方法间隔不小于“最短间隔”（默认 30 分钟）；手动不受它限制
- 遇到上游拒绝就停止这次探测的后续请求；不改 CPR 里的账号状态

## 自动探测、历史与指纹库

- 宿主每轮维护（`plugin.reconcile`，每轮结束后约 30 秒）找到期的任务；忙碌的下一轮重试，被拦截的 10 分钟后重试；上游拒绝后不冷却
- 列表以 Uptime Kuma 风格展示时间条，支持最近 24 小时、7 天、30 天；格子按可用宽度合并相邻时段，完整保留所选范围与全部记录。浅灰表示没有探测，深灰表示探测过但未判定或失败，蓝色表示进行中；它们都不计入正常率
- 自动 ModelTrace：每轮维护答一道题，跨调用完成同一条记录，每道题前重新检查账号与安全闸。管理和维护使用 SDK 默认的 120 秒预算，兼容 CPR 受管 HTTP 的上限；请求超时前留出时间保存失败结果
- 手动探测：测票在一次请求里做完；ModelTrace 由页面一题一题推进（CPR 管理请求 180 秒超时）。页面关掉或插件重启时，超过 20 分钟没完成的记录标成“中断”
- 历史存在插件私有状态：每个账号每种方法保留 30 天摘要（状态条、正常率），详情只留最近 50 条（单条私有状态最大 256 KiB、每个命名空间 16 MiB）
- 指纹库内置一份（`backend/data/unified_bank.json`），按“指纹库同步间隔”（默认 6 小时，0 关闭）从 GitHub 同步 `xqy2006/ModelTrace` 的 `static/data/unified_bank.json`；页面上可以立即同步。新库校验不过就继续用手上的那份并显示原因。指纹库只放内存，插件重启后先用内置的

## 开发

需要 Rust 1.99.1、Node.js 24、pnpm 12.6。

```bash
pnpm --dir frontend install --frozen-lockfile
pnpm --dir frontend build
pnpm --dir frontend test
CHROME_BIN=/path/to/chrome node scripts/check-ui.mjs
cargo test --manifest-path backend/Cargo.toml
```

`backend/tests/fingerprint.rs` 用 ModelTrace JS 实现生成的黄金向量逐项对拍 Rust 移植。

## 打包与安装

```bash
cargo install --locked --git https://github.com/zyycn/codex-proxy-rs.git --rev 47d30474af7c3e1fab433f2d4a70430fae548ff2 codex-proxy-plugin-cli --root .tools
bash scripts/package            # 默认本机平台；也可传 aarch64-unknown-linux-gnu 等
```

在 CPR 的“插件管理”点安装，来源选 **GitHub**，填 `Milesians/cpr-turnstate`（标签留空取最新稳定版），按 CPR 服务器的平台选 `x86_64` 或 `aarch64` 的包，确认完整信任后安装；以后可以在插件详情里检查更新。也可以上传本地 `dist/` 里的 `.tar.gz`。

发布新版本：改 `plugin.json` 的 `version` 和 `release/notes.md`，推送 `v<版本>` 标签，GitHub Actions 检查通过后构建 Linux x86_64 / aarch64 两个安装包，连同 `.sha256` 和 `checksums.txt` 发布为 Release。插件以 CPR 的系统身份运行（能读账号凭据），CPR 不提供沙箱。

## 许可

MIT。ModelTrace 指纹算法与指纹库来自 xqy2006/ModelTrace（MIT），Codex 提示词来自 openai/codex（Apache-2.0），协议见 `backend/data/`。
