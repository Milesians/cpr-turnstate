//! 插件设置（CPR 插件详情里的“设置”），握手时读一次

use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// 关闭时什么也发不出去
    pub probe_enabled: bool,
    /// 自动探测同账号同方法的最短间隔；手动不受限
    pub min_interval_minutes: u32,
    /// 0 表示不同步，只用内置指纹库
    pub fingerprint_sync_hours: u32,
    pub telegram_enabled: bool,
    pub telegram_bot_token: String,
    pub telegram_chat_id: String,
    pub telegram_thread_id: u32,
    /// 调整现有分组时用的 CPR 管理接口地址与部署级管理 API Key（敏感字段）
    pub cpr_admin_url: String,
    pub cpr_admin_api_key: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            probe_enabled: false,
            min_interval_minutes: 30,
            fingerprint_sync_hours: 6,
            telegram_enabled: false,
            telegram_bot_token: String::new(),
            telegram_chat_id: String::new(),
            telegram_thread_id: 0,
            cpr_admin_url: String::new(),
            cpr_admin_api_key: String::new(),
        }
    }
}

impl Config {
    /// 读宿主握手里的设置；字段缺失时取默认值，类型不对时整份退回默认（探测保持关闭）
    #[must_use]
    pub fn from_handshake(value: &Value) -> Self {
        serde_json::from_value(value.clone()).unwrap_or_default()
    }
}
