# 内置数据

- `unified_bank.json`：ModelTrace 指纹库，取自 [xqy2006/ModelTrace](https://github.com/xqy2006/ModelTrace)（MIT，见 `LICENSE.ModelTrace`）的 `static/data/unified_bank.json`，提交 `6cd4a8ff0f3f4e49d6d6969dd2fbadfa1569fde2`（2026-10-09）。原样保留、不手改。它只是兜底：插件按设置的间隔从 GitHub 同步最新一份（见 `src/bank_sync.rs`）。更新内置的这份时整体替换，并同步 `src/fingerprint/bank.rs` 里的 `EMBEDDED_COMMIT`。
- `gpt_5_codex_prompt.md`：Codex 基础提示词，取自 [openai/codex](https://github.com/openai/codex)（Apache-2.0，见 `LICENSE.openai-codex`）的 `codex-rs/core/gpt_5_codex_prompt.md`，提交 `6ae8b8aa7c608730d4315d59c5c081897ae02780`，原样未改，SHA-256 `42842be69650ae563d212695e8d3f3591534908fd8ca33b63f742daf41f88b65`。ChatGPT 的 Codex 接口要求请求带 `instructions`；这份与 sub2api“测试连接”默认发的逐字节一致。
- `../tests/data/golden.json`：ModelTrace 自己的 JS 实现算出的黄金向量（由 ezou-manager 的 `scripts/gen-fingerprint-golden.mjs` 生成），Rust 移植逐项对拍。
