//! 自动探测：宿主每轮维护（`plugin.reconcile`，每轮结束后约 30 秒）找到期的任务，
//! 经运行器发起，和手动探测同一套安全闸
//!
//! 只有测票能自动探测，ModelTrace 只手动探测。
//! 同账号串行，全局最多 [`MAX_CONCURRENT`] 个；忙碌的下一轮重试，其他拦截 10 分钟后重试。
//! 上游拒绝后不冷却。
//!
//! 所有启用的账号，每轮按 CPR 给的账号状态判断是否因为账号本身的问题受阻（凭据失效、
//! 令牌过期等），和开没开自动测票、真实探测无关：持续 [`BLOCK_CONFIRM_MS`] 才确认并发 TG 通知，
//! 解除时（确认过的）再通知一次。受阻不是判定，不改当前判定。

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use futures::future::join_all;
use gateway_plugin_sdk::{
    PluginFault,
    call::host::{AuthRuntimeAccount, LogLevel},
    client::HostClient,
};
use serde::Serialize;
use serde_json::json;

use crate::clock::now_ms;
use crate::health::{BLOCK_CONFIRM_MS, Blocked, NoticeKind};
use crate::host;
use crate::runner::{Blocker, MAX_CONCURRENT, Runner, STALE_MS, account_blockers, supported};
use crate::store::{self, Method, Status};

const BLOCKED_RETRY_MS: i64 = 10 * 60 * 1000;
const RECOVER_EVERY_MS: i64 = 10 * 60 * 1000;

/// 一个账号一种方法上次自动探测的情况，给页面看
#[derive(Debug, Clone, Default, Serialize)]
pub struct LastAttempt {
    pub at_ms: i64,
    pub started: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blockers: Vec<Blocker>,
}

#[derive(Default)]
struct State {
    /// 上次开始探测的时间，启动后第一次用到时从历史读
    last_started: HashMap<(String, Method), i64>,
    /// 被拦截后什么时候再试
    retry_at: HashMap<(String, Method), i64>,
    last: HashMap<(String, Method), LastAttempt>,
    recovered_ms: i64,
    /// 各账号记着的受阻状态，启动后第一次用到时从历史读；只在变化时写回
    blocked: HashMap<String, Option<Blocked>>,
}

#[derive(Default)]
pub struct Scheduler {
    state: Mutex<State>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Scheduler {
    #[must_use]
    pub fn last(&self, account_id: &str, method: Method) -> Option<LastAttempt> {
        lock(&self.state)
            .last
            .get(&(account_id.to_owned(), method))
            .cloned()
    }

    /// 手动探测开始后也算一次，自动探测按它计算间隔
    pub fn note_started(&self, account_id: &str, method: Method, at_ms: i64) {
        lock(&self.state)
            .last_started
            .insert((account_id.to_owned(), method), at_ms);
    }

    /// 一轮维护
    ///
    /// # Errors
    /// 读账号或设置失败时返回错误，宿主 5 秒后重试
    pub async fn reconcile(
        &self,
        runner: &Runner,
        groups: &crate::groups::Groups,
        host: &HostClient,
    ) -> Result<(), PluginFault> {
        runner
            .bank
            .sync_if_due(host, runner.config.fingerprint_sync_hours)
            .await;
        let accounts = host::openai_accounts(host).await?;
        // 先记受阻，确认的通知这一轮就能发出去
        self.track_blocked(runner, host, &accounts).await;
        let mut delivered = 0;
        for account in &accounts {
            if crate::telegram::dispatch(host, &runner.config, account).await {
                delivered += 1;
            }
            if delivered >= 8 || host.remaining_budget().is_some_and(|b| b.as_secs() < 15) {
                break;
            }
        }
        for (account_id, run_id) in runner.stale() {
            runner.abandon(host, &account_id, &run_id).await?;
        }
        self.recover(runner, host, &accounts).await?;
        // 判定变了的账号调整分组；不发探测，不受“发出真实探测”开关影响
        if let Err(error) = groups.sync(runner, host, &accounts).await {
            host::log(
                host,
                "group_sync_failed",
                LogLevel::Warn,
                json!({"code": format!("{:?}", error.code)}),
            )
            .await;
        }
        if !runner.config.probe_enabled {
            return Ok(());
        }
        let (schedules, _) = store::schedules(host).await?;
        let by_id: HashMap<&str, &AuthRuntimeAccount> = accounts
            .iter()
            .map(|a| (a.account_id.as_str(), a))
            .collect();

        // 到期的任务，按逾期时间排序；同账号的放在一起串行
        let now = now_ms();
        let mut due: Vec<(i64, &AuthRuntimeAccount, Method, String)> = Vec::new();
        for (account_id, schedule) in &schedules.accounts {
            let Some(account) = by_id.get(account_id.as_str()) else {
                continue;
            };
            for method in Method::ALL {
                let s = schedule.get(method);
                if !method.schedulable() || !s.enabled || s.model.is_empty() {
                    continue;
                }
                let key = (account_id.clone(), method);
                if lock(&self.state)
                    .retry_at
                    .get(&key)
                    .is_some_and(|t| *t > now)
                {
                    continue;
                }
                let last = self.last_started(host, account_id, method).await?;
                let interval =
                    i64::from(s.interval_minutes.max(runner.config.min_interval_minutes)) * 60_000;
                let at = last.map_or(now, |l| l + interval);
                if at <= now {
                    due.push((at, account, method, s.model.clone()));
                }
            }
        }
        due.sort_by_key(|(at, ..)| *at);
        let mut groups: BTreeMap<String, (i64, Vec<(Method, String)>)> = BTreeMap::new();
        for (at, account, method, model) in &due {
            groups
                .entry(account.account_id.clone())
                .or_insert((*at, Vec::new()))
                .1
                .push((*method, model.clone()));
        }
        let mut ordered: Vec<_> = groups.into_iter().collect();
        ordered.sort_by_key(|(_, (at, _))| *at);
        ordered.truncate(MAX_CONCURRENT);

        let tasks = ordered.into_iter().map(|(account_id, (_, methods))| {
            let account = by_id[account_id.as_str()];
            async move {
                for (method, model) in methods {
                    if runner.running(&account_id).is_some()
                        || host
                            .remaining_budget()
                            .is_some_and(|budget| budget.as_secs() < 15)
                    {
                        break;
                    }
                    self.run_one(runner, host, account, method, &model).await;
                }
            }
        });
        join_all(tasks).await;
        Ok(())
    }

    async fn run_one(
        &self,
        runner: &Runner,
        host: &HostClient,
        account: &AuthRuntimeAccount,
        method: Method,
        model: &str,
    ) {
        let key = (account.account_id.clone(), method);
        let last = lock(&self.state).last_started.get(&key).copied();
        let mut attempt = LastAttempt {
            at_ms: now_ms(),
            started: false,
            blockers: Vec::new(),
        };
        if let Some(blocker) = runner.too_soon(last) {
            attempt.blockers.push(blocker);
        } else {
            match runner
                .start(host, account, method, model, crate::runner::Trigger::Auto)
                .await
            {
                Ok(Ok(run)) => {
                    attempt.started = true;
                    lock(&self.state)
                        .last_started
                        .insert(key.clone(), run.started_ms);
                }
                Ok(Err(blockers)) => attempt.blockers = blockers,
                Err(error) => {
                    host::log(
                        host,
                        "scheduled_probe_failed",
                        LogLevel::Warn,
                        json!({"code": format!("{:?}", error.code), "method": method.as_str()}),
                    )
                    .await;
                    attempt.blockers.push(Blocker {
                        code: "host",
                        message: "宿主操作失败，稍后重试".into(),
                    });
                }
            }
        }
        let mut state = lock(&self.state);
        if !attempt.started && !attempt.blockers.iter().all(Blocker::busy) {
            state
                .retry_at
                .insert(key.clone(), attempt.at_ms + BLOCKED_RETRY_MS);
        } else {
            state.retry_at.remove(&key);
        }
        state.last.insert(key, attempt);
    }

    async fn last_started(
        &self,
        host: &HostClient,
        account_id: &str,
        method: Method,
    ) -> Result<Option<i64>, PluginFault> {
        let key = (account_id.to_owned(), method);
        if let Some(at) = lock(&self.state).last_started.get(&key) {
            return Ok(Some(*at));
        }
        let at = store::history(host, account_id, method)
            .await?
            .last()
            .map(|s| s.started_ms);
        if let Some(at) = at {
            lock(&self.state).last_started.insert(key, at);
        }
        Ok(at)
    }

    /// 受阻状态的下一步：返回新状态（`None` 表示没变化）和要发的通知
    fn next_blocked(
        known: Option<&Blocked>,
        current: Option<&Blocker>,
        now: i64,
    ) -> Option<(Option<Blocked>, Option<NoticeKind>)> {
        match (known, current) {
            (None, None) => None,
            (None, Some(b)) => Some((
                Some(Blocked {
                    code: b.code.to_owned(),
                    message: b.message.clone(),
                    since_ms: now,
                    confirmed_ms: 0,
                }),
                None,
            )),
            (Some(k), Some(b)) => {
                let changed = k.code != b.code || k.message != b.message;
                let confirm = k.confirmed_ms == 0 && now - k.since_ms >= BLOCK_CONFIRM_MS;
                if !changed && !confirm {
                    return None;
                }
                // 原因换了也还是同一段受阻，从最初开始算
                let next = Blocked {
                    code: b.code.to_owned(),
                    message: b.message.clone(),
                    since_ms: k.since_ms,
                    confirmed_ms: if confirm { now } else { k.confirmed_ms },
                };
                Some((Some(next), confirm.then_some(NoticeKind::Blocked)))
            }
            (Some(k), None) => Some((None, (k.confirmed_ms > 0).then_some(NoticeKind::Unblocked))),
        }
    }

    /// 记下开着自动测票的账号是否因为账号本身的问题受阻。读写失败只记日志，下一轮再试
    async fn track_blocked(
        &self,
        runner: &Runner,
        host: &HostClient,
        accounts: &[AuthRuntimeAccount],
    ) {
        let now = now_ms();
        for account in accounts.iter().filter(|a| supported(a)) {
            let id = &account.account_id;
            // 所有启用的账号都看（和开没开自动测票、真实探测无关）；在 CPR 停用的不算，也不发恢复通知
            let monitored = account.enabled;
            let current = if monitored {
                account_blockers(account, Method::Turnstate, now)
                    .into_iter()
                    .next()
            } else {
                None
            };
            let cached = lock(&self.state).blocked.get(id).cloned();
            // 启动后每个账号读一次历史，之后只看内存
            let known = match cached {
                Some(known) => known,
                None => match store::blocked(host, id).await {
                    Ok(known) => known,
                    Err(_) => continue,
                },
            };
            let Some((next, notice)) = Self::next_blocked(known.as_ref(), current.as_ref(), now)
            else {
                lock(&self.state).blocked.insert(id.clone(), known);
                continue;
            };
            // 不在监控时清掉旧状态但不通知
            let notice = notice.filter(|_| monitored);
            match store::set_blocked(
                host,
                id,
                next.clone(),
                notice,
                runner.config.telegram_enabled,
            )
            .await
            {
                Ok(()) => {
                    lock(&self.state).blocked.insert(id.clone(), next);
                }
                Err(error) => {
                    host::log(
                        host,
                        "blocked_track_failed",
                        LogLevel::Warn,
                        json!({"code": format!("{:?}", error.code)}),
                    )
                    .await;
                }
            }
        }
    }

    /// 插件重启后，历史里还“进行中”的记录不会再有人推进：超时的标成中断
    async fn recover(
        &self,
        runner: &Runner,
        host: &HostClient,
        accounts: &[AuthRuntimeAccount],
    ) -> Result<(), PluginFault> {
        let now = now_ms();
        if now - lock(&self.state).recovered_ms < RECOVER_EVERY_MS {
            return Ok(());
        }
        for account in accounts.iter().filter(|a| supported(a)) {
            for method in Method::ALL {
                for summary in store::history(host, &account.account_id, method).await? {
                    if summary.status == Status::Running
                        && summary.started_ms < now - STALE_MS
                        && runner.running(&account.account_id).as_deref()
                            != Some(summary.id.as_str())
                    {
                        runner
                            .abandon(host, &account.account_id, &summary.id)
                            .await?;
                    }
                }
            }
        }
        lock(&self.state).recovered_ms = now;
        Ok(())
    }
}
