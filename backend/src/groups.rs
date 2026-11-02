//! 自动分组：账号的测票当前判定（连续 3 次确认的那个）变化时调整 CPR 分组成员
//!
//! 只在判定变化时动（用户 2026-11-02）：每个账号记下自动分组上次按哪个判定调整过；判定变了，
//! 只对这个账号按“旧判定 → 新判定”做最小改动：新判定符合、旧判定不符合的组加进去，旧判定符合、
//! 新判定不符合的组移出去，其余一概不动。不做全量核对，不纠正 CPR 里的手动改动；保存设置、
//! 删除规则都不改成员。第一次看到某个账号、或从关闭切到开启时，只记下当前判定作为起点。
//!
//! 两种目标分组：
//! - 插件新建的分组：插件 SDK 只允许插件改自己建的分组（按 resource_key）。分组名和颜色只在第一次
//!   创建时使用，之后在 CPR 的分组管理里改；管理员删掉分组后下次调整时重建。
//! - CPR 现有分组：SDK 改不了，经 CPR 管理员接口（[`crate::admin`]）读账号现在所在的全部分组，
//!   加上或去掉这条规则的分组后整体写回（CPR 只支持整体替换）。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use gateway_plugin_sdk::{
    ErrorCode, PluginFault,
    call::{
        host::AuthRuntimeAccount,
        resources::{GroupEnsureRequest, GroupMembersChange},
    },
    client::HostClient,
};
use serde::{Deserialize, Serialize};

use crate::admin::{Admin, GroupRef};
use crate::clock::now_ms;
use crate::runner::{Runner, supported};
use crate::store;
use crate::verdict::{Level, one_line};

pub const MAX_RULES: usize = 8;
/// 宿主单次增删上限
const CHUNK: usize = 200;
const RULE_PREFIX: &str = "auto-";

/// CPR 分组 ID：`grp_` 加 32 位小写十六进制
fn valid_group_id(id: &str) -> bool {
    id.strip_prefix("grp_").is_some_and(|rest| {
        rest.len() == 32
            && rest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GroupConfig {
    pub enabled: bool,
    pub rules: Vec<GroupRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupRule {
    /// 插件新建的分组时也是宿主的 resource_key
    pub id: String,
    /// CPR 现有分组的 ID；没有就是插件新建的分组
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    /// 现有分组时是保存时 CPR 里的名称，只用来显示
    pub name: String,
    /// `#RRGGBBAA`
    pub color: String,
    /// 当前测票判定在其中的账号进这个组；`unknown` 是还没有确认过的判定
    pub levels: Vec<Level>,
    /// 账号类型（CPR 给的套餐，小写，没有套餐是 `unknown`）；空表示不限（用户 2026-11-02）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plans: Vec<String>,
}

impl GroupRule {
    /// 判定和账号类型都符合才进组
    fn admits(&self, level: Option<&Level>, plan: Option<&str>) -> bool {
        level.is_some_and(|l| self.levels.contains(l))
            && (self.plans.is_empty() || plan.is_some_and(|p| self.plans.iter().any(|x| x == p)))
    }
}

/// 账号类型：CPR 给的套餐，小写；没有就是 `unknown`
#[must_use]
pub fn plan_of(account: &AuthRuntimeAccount) -> String {
    account
        .plan_type
        .as_deref()
        .map(|p| p.trim().to_lowercase())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

const MAX_PLANS: usize = 16;

fn valid_plan(plan: &str) -> bool {
    (1..=32).contains(&plan.len())
        && plan
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
}

/// 页面提交的规则；新规则不带 id
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleInput {
    #[serde(default)]
    pub id: Option<String>,
    /// 选 CPR 现有分组时填；名称和颜色以 CPR 为准
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub color: String,
    pub levels: Vec<Level>,
    /// 不填或为空表示不限账号类型
    #[serde(default)]
    pub plans: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigInput {
    pub enabled: bool,
    pub rules: Vec<RuleInput>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RuleStatus {
    /// CPR 里的分组 ID 和当前名称（可能已在 CPR 改名）
    pub group_id: String,
    pub group_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 最近一次按判定变化调整的结果
#[derive(Debug, Clone, Serialize)]
pub struct Change {
    pub account_id: String,
    pub from: Level,
    pub to: Level,
    pub at_ms: i64,
    /// 加进去和移出去的分组名
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Status {
    /// 上次调整（或核对起点）的时间
    pub synced_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub rules: BTreeMap<String, RuleStatus>,
    /// 最近的调整，新的在前，最多 20 条
    pub changes: Vec<Change>,
}

const MAX_CHANGES: usize = 20;

#[derive(Default)]
struct State {
    /// 插件新建的分组在 CPR 里的 ID，不能再被选作现有分组
    owned_group_ids: BTreeSet<String>,
    /// 启动后核对过所有账号没有（补上插件不在时的判定变化、给新账号记起点）
    scanned: bool,
    /// 上次调整失败、下一轮再试的账号
    retry: BTreeSet<String>,
    status: Status,
}

#[derive(Default)]
pub struct Groups {
    state: Mutex<State>,
    /// 同一时间只有一次同步（维护和保存设置可能同时触发）
    sync: tokio::sync::Mutex<()>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `#RRGGBB` 补成不透明的 `#RRGGBBFF`；返回大写
fn normalize_color(color: &str) -> Option<String> {
    let color = color.trim();
    let hex = color.strip_prefix('#')?;
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match hex.len() {
        6 => Some(format!("#{}FF", hex.to_ascii_uppercase())),
        8 => Some(format!("#{}", hex.to_ascii_uppercase())),
        _ => None,
    }
}

fn valid_rule_id(id: &str) -> bool {
    id.strip_prefix(RULE_PREFIX).is_some_and(|rest| {
        rest.len() == 8
            && rest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

fn new_rule_id() -> Result<String, String> {
    let mut bytes = [0_u8; 4];
    crate::fingerprint::system_random(&mut bytes).map_err(|_| "生成规则 ID 失败".to_owned())?;
    Ok(format!("{RULE_PREFIX}{}", hex::encode(bytes)))
}

/// 页面提交时需要的 CPR 现状
pub struct Existing<'a> {
    /// CPR 里的分组；只有提交里有现有分组规则时才读
    pub groups: Option<&'a [GroupRef]>,
    /// 不能选的分组：插件自己维护成员的那些
    pub forbidden: &'a BTreeSet<String>,
}

/// 校验页面提交的设置，得到新设置
///
/// # Errors
/// 设置不合法时返回给页面看的原因
pub fn apply_input(
    current: &GroupConfig,
    input: ConfigInput,
    existing: &Existing<'_>,
) -> Result<GroupConfig, String> {
    if input.rules.len() > MAX_RULES {
        return Err(format!("最多 {MAX_RULES} 个分组"));
    }
    let mut names = BTreeSet::new();
    let mut targets = BTreeSet::new();
    let mut rules = Vec::with_capacity(input.rules.len());
    for rule in input.rules {
        let mut levels: Vec<Level> = rule.levels;
        levels.sort();
        levels.dedup();
        let mut plans: Vec<String> = rule.plans.iter().map(|p| p.trim().to_lowercase()).collect();
        plans.sort();
        plans.dedup();
        if plans.len() > MAX_PLANS || !plans.iter().all(|p| valid_plan(p)) {
            return Err("账号类型无效".into());
        }
        let previous = match rule.id.as_deref().filter(|id| !id.is_empty()) {
            None => None,
            Some(id) => Some(
                current
                    .rules
                    .iter()
                    .find(|r| r.id == id)
                    .ok_or("规则已不存在，请刷新页面")?,
            ),
        };
        let group_id = rule.group_id.filter(|g| !g.is_empty());
        if let Some(previous) = previous
            && previous.group_id != group_id
        {
            return Err(format!(
                "分组“{}”的目标不能修改，请删掉这条规则再新建",
                previous.name
            ));
        }
        let (name, color) = if let Some(group_id) = &group_id {
            if !valid_group_id(group_id) {
                return Err("CPR 分组 ID 无效，请刷新页面".into());
            }
            if existing.forbidden.contains(group_id) {
                return Err("这个分组由插件自己维护成员，不能再选作现有分组".into());
            }
            if !targets.insert(group_id.clone()) {
                return Err("同一个 CPR 分组只能对应一条规则".into());
            }
            let found = existing
                .groups
                .and_then(|groups| groups.iter().find(|g| &g.id == group_id));
            match (found, previous) {
                (Some(group), _) => (
                    group.name.clone(),
                    normalize_color(&group.color).unwrap_or_else(|| "#6B7280FF".into()),
                ),
                // 已有规则的分组在 CPR 里删掉了：照常保存，同步时在规则下报错
                (None, Some(previous)) => (previous.name.clone(), previous.color.clone()),
                (None, None) => return Err("CPR 里没有这个分组，请刷新后重选".into()),
            }
        } else {
            let name = rule.name.trim().to_owned();
            if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
                return Err("分组名需要 1 到 100 个字符".into());
            }
            if !names.insert(name.to_lowercase()) {
                return Err(format!("分组名“{name}”重复"));
            }
            let color = normalize_color(&rule.color).ok_or("颜色格式应为 #RRGGBB")?;
            // 宿主只在创建时用名称和颜色
            if let Some(previous) = previous
                && (previous.name != name || previous.color != color)
            {
                return Err(format!(
                    "分组“{}”已创建，名称和颜色请在 CPR 的分组管理里修改",
                    previous.name
                ));
            }
            (name, color)
        };
        if levels.is_empty() {
            return Err(format!("分组“{name}”至少要选一种判定"));
        }
        let id = match previous {
            Some(previous) => previous.id.clone(),
            None => new_rule_id()?,
        };
        if !valid_rule_id(&id) {
            return Err("规则 ID 无效，请刷新页面".into());
        }
        rules.push(GroupRule {
            id,
            group_id,
            name,
            color,
            levels,
            plans,
        });
    }
    Ok(GroupConfig {
        enabled: input.enabled,
        rules,
    })
}

fn host_message(error: &PluginFault) -> String {
    match error.code {
        // 宿主把同名分组和分组归属变化都报成冲突
        ErrorCode::Conflict => {
            "CPR 里已有同名分组（或插件实例变了），请删掉这条规则后换个名字重建".into()
        }
        ErrorCode::PermissionDenied | ErrorCode::Rejected => {
            "插件没有权限修改这个分组，请检查插件实例".into()
        }
        _ => format!("宿主操作失败：{}", one_line(&error.message, 120)),
    }
}

/// 一个账号的判定变化
struct Transition {
    account_id: String,
    from: Level,
    to: Level,
    plan: String,
}

impl Transition {
    /// 这条规则对这个账号要做的：`Some(true)` 加进去，`Some(false)` 移出去，`None` 不动
    fn action(&self, rule: &GroupRule) -> Option<bool> {
        let before = rule.admits(Some(&self.from), Some(&self.plan));
        let after = rule.admits(Some(&self.to), Some(&self.plan));
        (before != after).then_some(after)
    }
}

/// 调整中可以下一轮重试的失败（宿主或管理接口暂时不可用）；配置问题（重名、分组被删）不重试
struct Outcome {
    retry: bool,
}

impl Groups {
    #[must_use]
    pub fn status(&self) -> Status {
        lock(&self.state).status.clone()
    }

    /// 插件自己维护成员的分组（新建的分组和 ModelTrace 用的分组），不能选作现有分组
    #[must_use]
    pub fn plugin_group_ids(&self, runner: &Runner) -> BTreeSet<String> {
        let mut ids = lock(&self.state).owned_group_ids.clone();
        ids.extend(runner.probe_group_id());
        ids
    }

    /// 每轮维护调用：只处理刚测完（和上次失败）的账号；启动后第一次核对全部账号
    ///
    /// # Errors
    /// 读设置或历史失败时返回错误；分组操作失败记在状态里
    pub async fn sync(
        &self,
        runner: &Runner,
        host: &HostClient,
        accounts: &[AuthRuntimeAccount],
    ) -> Result<(), PluginFault> {
        let _guard = self.sync.lock().await;
        let (config, _) = store::group_config(host).await?;
        let finished = runner.take_turnstate_finished();
        if !config.enabled {
            return Ok(());
        }
        let (scan_all, retry) = {
            let state = lock(&self.state);
            (!state.scanned, state.retry.clone())
        };
        let candidates: Vec<&AuthRuntimeAccount> = accounts
            .iter()
            .filter(|a| {
                supported(a)
                    && (scan_all
                        || finished.contains(&a.account_id)
                        || retry.contains(&a.account_id))
            })
            .collect();
        let mut transitions = Vec::new();
        for a in candidates {
            let (current, applied) = store::group_levels(host, &a.account_id).await?;
            match applied {
                // 起点：不调整
                None => store::set_grouped_level(host, &a.account_id, current).await?,
                Some(applied) if applied == current => {}
                Some(applied) => transitions.push(Transition {
                    account_id: a.account_id.clone(),
                    from: applied,
                    to: current,
                    plan: plan_of(a),
                }),
            }
        }
        lock(&self.state).scanned = true;
        if transitions.is_empty() {
            lock(&self.state).retry.clear();
            return Ok(());
        }
        let mut changes: Vec<Change> = transitions
            .iter()
            .map(|t| Change {
                account_id: t.account_id.clone(),
                from: t.from,
                to: t.to,
                at_ms: now_ms(),
                added: Vec::new(),
                removed: Vec::new(),
            })
            .collect();
        let mut statuses = lock(&self.state).status.rules.clone();
        statuses.retain(|id, _| config.rules.iter().any(|r| &r.id == id));
        let owned = self
            .apply_owned(host, &config, &transitions, &mut changes, &mut statuses)
            .await;
        let existing = apply_existing(
            runner,
            host,
            &config,
            &transitions,
            &mut changes,
            &mut statuses,
        )
        .await;
        let retry = owned.retry || existing.retry;
        if !retry {
            for t in &transitions {
                store::set_grouped_level(host, &t.account_id, t.to).await?;
            }
        }
        let mut state = lock(&self.state);
        state.retry = if retry {
            transitions.iter().map(|t| t.account_id.clone()).collect()
        } else {
            BTreeSet::new()
        };
        state.status.synced_ms = now_ms();
        state.status.rules = statuses;
        if !retry {
            for change in changes.into_iter().rev() {
                state.status.changes.insert(0, change);
            }
            state.status.changes.truncate(MAX_CHANGES);
        }
        Ok(())
    }

    /// 保存设置后调用：不改任何成员。从关闭切到开启时，按当前判定重新记起点；
    /// 确认插件新建的分组存在、读现有分组的名称，结果放进状态
    ///
    /// # Errors
    /// 读设置或历史失败时返回错误
    pub async fn after_save(
        &self,
        runner: &Runner,
        host: &HostClient,
        accounts: &[AuthRuntimeAccount],
        enabling: bool,
    ) -> Result<(), PluginFault> {
        let _guard = self.sync.lock().await;
        let (config, _) = store::group_config(host).await?;
        if !config.enabled {
            return Ok(());
        }
        if enabling {
            runner.take_turnstate_finished();
            for a in accounts.iter().filter(|a| supported(a)) {
                let (current, _) = store::group_levels(host, &a.account_id).await?;
                store::set_grouped_level(host, &a.account_id, current).await?;
            }
            let mut state = lock(&self.state);
            state.scanned = true;
            state.retry.clear();
        }
        let mut statuses = BTreeMap::new();
        for rule in config.rules.iter().filter(|r| r.group_id.is_none()) {
            let status: &mut RuleStatus = statuses.entry(rule.id.clone()).or_default();
            match self.ensure(host, rule).await {
                Ok((id, name)) => {
                    status.group_id = id;
                    status.group_name = name;
                }
                Err(error) => status.error = Some(host_message(&error)),
            }
        }
        if config.rules.iter().any(|r| r.group_id.is_some()) {
            let groups = match Admin::new(host, &runner.config) {
                Ok(admin) => admin.groups().await,
                Err(reason) => Err(reason),
            };
            for rule in config.rules.iter().filter(|r| r.group_id.is_some()) {
                let status: &mut RuleStatus = statuses.entry(rule.id.clone()).or_default();
                describe_existing(rule, groups.as_deref(), status);
            }
        }
        let mut state = lock(&self.state);
        state.status.rules = statuses;
        state.status.synced_ms = now_ms();
        state.status.error = None;
        Ok(())
    }

    /// 建（或找到）插件新建的分组，返回 CPR 里的 ID 和当前名称
    async fn ensure(
        &self,
        host: &HostClient,
        rule: &GroupRule,
    ) -> Result<(String, String), PluginFault> {
        let group = host
            .ensure_group(GroupEnsureRequest {
                resource_key: rule.id.clone(),
                name: rule.name.clone(),
                color: rule.color.clone(),
                description: Some("账号探测插件在测票判定变化时调整成员".into()),
            })
            .await?;
        lock(&self.state).owned_group_ids.insert(group.id.clone());
        Ok((group.id, group.name))
    }

    async fn apply_owned(
        &self,
        host: &HostClient,
        config: &GroupConfig,
        transitions: &[Transition],
        changes: &mut [Change],
        statuses: &mut BTreeMap<String, RuleStatus>,
    ) -> Outcome {
        let mut retry = false;
        for rule in config.rules.iter().filter(|r| r.group_id.is_none()) {
            let actions: Vec<(usize, bool)> = transitions
                .iter()
                .enumerate()
                .filter_map(|(i, t)| t.action(rule).map(|a| (i, a)))
                .collect();
            if actions.is_empty() {
                continue;
            }
            let status = statuses.entry(rule.id.clone()).or_default();
            let (group_id, group_name) = match self.ensure(host, rule).await {
                Ok(found) => found,
                // 重名等配置问题：等用户改规则，不重试
                Err(error) => {
                    status.error = Some(host_message(&error));
                    continue;
                }
            };
            status.group_id = group_id;
            status.group_name.clone_from(&group_name);
            status.error = None;
            let mut failed = None;
            for chunk in actions.chunks(CHUNK) {
                let pick = |add: bool| -> Vec<String> {
                    chunk
                        .iter()
                        .filter(|(_, a)| *a == add)
                        .map(|(i, _)| transitions[*i].account_id.clone())
                        .collect()
                };
                if let Err(error) = host
                    .change_group_members(GroupMembersChange {
                        resource_key: rule.id.clone(),
                        add: pick(true),
                        remove: pick(false),
                    })
                    .await
                {
                    failed = Some(error);
                    break;
                }
            }
            match failed {
                Some(error) => {
                    retry = true;
                    status.error = Some(host_message(&error));
                }
                None => {
                    for (i, add) in actions {
                        let list = if add {
                            &mut changes[i].added
                        } else {
                            &mut changes[i].removed
                        };
                        list.push(group_name.clone());
                    }
                }
            }
        }
        Outcome { retry }
    }
}

fn describe_existing(
    rule: &GroupRule,
    groups: Result<&[GroupRef], &String>,
    status: &mut RuleStatus,
) {
    let gid = rule.group_id.as_deref().unwrap_or_default();
    status.group_id = gid.to_owned();
    status.group_name.clone_from(&rule.name);
    match groups {
        Ok(groups) => match groups.iter().find(|g| g.id == gid) {
            Some(group) => {
                status.group_name.clone_from(&group.name);
                status.error = None;
            }
            None => {
                status.error = Some("CPR 里找不到这个分组（可能已删除），请删掉这条规则".into());
            }
        },
        Err(reason) => status.error = Some(reason.clone()),
    }
}

/// 经 CPR 管理员接口调整现有分组：只读、只写判定变了的账号；每个账号读它现在的全部分组，
/// 加上或去掉规则的分组后整体写回，账号的其他分组（包括手动加的）不变
async fn apply_existing(
    runner: &Runner,
    host: &HostClient,
    config: &GroupConfig,
    transitions: &[Transition],
    changes: &mut [Change],
    statuses: &mut BTreeMap<String, RuleStatus>,
) -> Outcome {
    let rules: Vec<&GroupRule> = config
        .rules
        .iter()
        .filter(|r| r.group_id.is_some())
        .collect();
    let needed = rules
        .iter()
        .any(|rule| transitions.iter().any(|t| t.action(rule).is_some()));
    if !needed {
        return Outcome { retry: false };
    }
    let fail = |statuses: &mut BTreeMap<String, RuleStatus>, reason: &str| {
        for rule in &rules {
            statuses.entry(rule.id.clone()).or_default().error = Some(reason.to_owned());
        }
        Outcome { retry: true }
    };
    let admin = match Admin::new(host, &runner.config) {
        Ok(admin) => admin,
        // 没配好是配置问题，不重试
        Err(reason) => {
            fail(statuses, &reason);
            return Outcome { retry: false };
        }
    };
    let groups = match admin.groups().await {
        Ok(groups) => groups,
        Err(reason) => return fail(statuses, &reason),
    };
    // 分组在 CPR 里删掉了的规则不动
    let mut live: Vec<(&GroupRule, &str, String)> = Vec::new();
    for rule in &rules {
        let status = statuses.entry(rule.id.clone()).or_default();
        describe_existing(rule, Ok(&groups), status);
        if status.error.is_none() {
            live.push((
                rule,
                rule.group_id.as_deref().unwrap_or_default(),
                status.group_name.clone(),
            ));
        }
    }
    if live.is_empty() {
        return Outcome { retry: false };
    }
    let accounts = match admin.accounts().await {
        Ok(accounts) => accounts,
        Err(reason) => return fail(statuses, &reason),
    };
    let mut batches: BTreeMap<Vec<String>, Vec<usize>> = BTreeMap::new();
    for (i, t) in transitions.iter().enumerate() {
        let Some(account) = accounts.iter().find(|a| a.id == t.account_id) else {
            continue;
        };
        let current: BTreeSet<&str> = account.groups.iter().map(|g| g.id.as_str()).collect();
        let mut desired = current.clone();
        for (rule, gid, _) in &live {
            match t.action(rule) {
                Some(true) => {
                    desired.insert(gid);
                }
                Some(false) => {
                    desired.remove(gid);
                }
                None => {}
            }
        }
        if desired != current {
            batches
                .entry(desired.into_iter().map(str::to_owned).collect())
                .or_default()
                .push(i);
        }
    }
    for (group_ids, indexes) in &batches {
        let ids: Vec<String> = indexes
            .iter()
            .map(|i| transitions[*i].account_id.clone())
            .collect();
        if let Err(reason) = admin.set_groups(&ids, group_ids).await {
            return fail(statuses, &reason);
        }
    }
    // 记下每个账号实际加进/移出的现有分组
    for (i, t) in transitions.iter().enumerate() {
        let Some(account) = accounts.iter().find(|a| a.id == t.account_id) else {
            continue;
        };
        for (rule, gid, name) in &live {
            let member = account.groups.iter().any(|g| g.id == *gid);
            match t.action(rule) {
                Some(true) if !member => changes[i].added.push(name.clone()),
                Some(false) if member => changes[i].removed.push(name.clone()),
                _ => {}
            }
        }
    }
    Outcome { retry: false }
}
