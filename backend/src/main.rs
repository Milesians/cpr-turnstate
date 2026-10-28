use cpr_account_probe::{PLUGIN_ID, config::Config, manifest, plugin};
use gateway_plugin_sdk::{
    CallBudgets, Stage,
    client::{PluginSession, SessionConfig, SessionError},
};

/// 自动探测在维护调用里跑完：几个账号的三道题加起来可能要十几分钟
const MAINTENANCE_BUDGET_MS: u64 = 30 * 60 * 1000;
/// 管理页的请求在 CPR 前端 180 秒超时，留一点余量
const MANAGEMENT_BUDGET_MS: u64 = 170 * 1000;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // stdout 用于协议通信，不能打印任何东西
    let mut session = PluginSession::accept(
        tokio::io::stdin(),
        tokio::io::stdout(),
        SessionConfig::default(),
    )
    .await?;
    let manifest = manifest()?;
    if session.handshake().plugin_id != PLUGIN_ID
        || session.handshake().contributes != manifest.contributes
    {
        return Err(SessionError::Handshake.into());
    }
    let config = Config::from_handshake(&session.handshake().configuration);
    session.set_call_budgets(CallBudgets::from([
        (Stage::Maintenance, MAINTENANCE_BUDGET_MS),
        (Stage::Management, MANAGEMENT_BUDGET_MS),
    ]))?;
    session.run(plugin(config)?).await?;
    Ok(())
}
