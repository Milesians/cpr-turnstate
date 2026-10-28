//! CPR 插件“账号探测”：ModelTrace 指纹与测票，主动探测 OpenAI OAuth 账号是否被降智

pub mod bank_sync;
pub mod clock;
pub mod codex;
pub mod config;
pub mod fingerprint;
mod host;
mod management;
pub mod modeltrace;
mod runner;
mod scheduler;
mod session;
pub mod store;
pub mod turnstate;
pub mod verdict;

use std::sync::Arc;

pub use session::serve;

use gateway_plugin_sdk::{
    Manifest, ManifestError,
    client::{AuthorError, ComposedPlugin, PluginBuilder, methods},
};

pub const PLUGIN_ID: &str = "ezou.account-probe";
const MANIFEST: &[u8] = include_bytes!("../../plugin.json");

/// 打包器和运行注册共用的作者清单
///
/// # Errors
/// 清单无效时返回错误
pub fn manifest() -> Result<Manifest, ManifestError> {
    Manifest::from_author_slice(MANIFEST)
}

/// 插件进程内共享的状态；只活在这个会话里
pub struct App {
    runner: runner::Runner,
    scheduler: scheduler::Scheduler,
}

/// 组装处理器
///
/// # Errors
/// 清单与处理器不一致时返回错误
pub fn plugin(config: config::Config) -> Result<ComposedPlugin, AuthorError> {
    let app = Arc::new(App {
        runner: runner::Runner::new(config),
        scheduler: scheduler::Scheduler::default(),
    });
    let management_app = Arc::clone(&app);
    let maintenance_app = Arc::clone(&app);
    PluginBuilder::from_json(MANIFEST)?
        .management(management::registration(), move |call| {
            let app = Arc::clone(&management_app);
            async move { management::handle(&app, call).await }
        })?
        .on(methods::RECONCILE, move |call| {
            let app = Arc::clone(&maintenance_app);
            async move {
                app.scheduler.reconcile(&app.runner, &call.host).await?;
                Ok(gateway_plugin_sdk::client::TypedReply::new(
                    gateway_plugin_sdk::client::Empty {},
                ))
            }
        })?
        .build()
}
