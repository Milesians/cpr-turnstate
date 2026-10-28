//! 插件设置（CPR 插件详情里的“设置”），握手时读一次

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// 关闭时什么也发不出去
    pub probe_enabled: bool,
    /// 自动探测同账号同方法的最短间隔；手动不受限
    pub min_interval_minutes: u32,
    /// 0 表示不同步，只用内置指纹库
    pub fingerprint_sync_hours: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            probe_enabled: false,
            min_interval_minutes: 30,
            fingerprint_sync_hours: 6,
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
