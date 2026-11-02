//! 自动分组：按测票当前判定（连续 3 次确认的那个）调整 CPR 分组成员
//!
//! 两种规则：
//! - 插件新建的分组：插件 SDK 只允许插件改自己建的分组（按 resource_key）。分组名和颜色只在第一次
//!   创建时使用，之后在 CPR 的分组管理里改；管理员删掉分组后下次同步重建；删掉规则先清空成员再丢掉。
//! - CPR 现有分组（用户 2026-11-02）：SDK 改不了，经 CPR 管理员接口（[`crate::admin`]）读账号现在
//!   所在的全部分组，换掉规则管的那几个后整体写回（CPR 只支持整体替换）；删掉规则只是不再调整。
//!
//! 关闭自动分组后成员保持原样。

use std::collections::{BTreeMap, BTreeSet, HashMap};
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
use crate::store::{self, Method};
use crate::verdict::{Level, one_line};

pub const MAX_RULES: usize = 8;
/// 全量重读判定、纠正 CPR 里手动改动的间隔；其间只处理刚测完的账号
const FULL_SYNC_MS: i64 = 5 * 60 * 1000;
/// 改成员失败后多久全量重试
const RETRY_MS: i64 = 60 * 1000;
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
    /// 删掉的插件新建分组的规则：成员清空前留着，清空后丢掉
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub retired: Vec<String>,
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
    pub members: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Status {
    pub synced_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub rules: BTreeMap<String, RuleStatus>,
}

#[derive(Default)]
struct State {
    /// 每个账号的测票当前判定；不支持探测的账号不在里面
    levels: HashMap<String, Level>,
    /// 插件新建的分组在 CPR 里的 ID，不能再被选作现有分组
    owned_group_ids: BTreeSet<String>,
    full_ms: i64,
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

/// 校验页面提交的设置，得到新设置；删掉的插件新建分组规则进 `retired`
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
        });
    }
    let mut retired = current.retired.clone();
    for old in &current.rules {
        if old.group_id.is_none()
            && !rules.iter().any(|r| r.id == old.id)
            && !retired.contains(&old.id)
        {
            retired.push(old.id.clone());
        }
    }
    retired.retain(|id| !rules.iter().any(|r| &r.id == id));
    Ok(GroupConfig {
        enabled: input.enabled,
        rules,
        retired,
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

/// 把账号按是否进组分批增删；没有变化时宿主不发布新配置
async fn set_members(
    host: &HostClient,
    resource_key: &str,
    accounts: &[(&str, bool)],
) -> Result<(), PluginFault> {
    for chunk in accounts.chunks(CHUNK) {
        let (add, remove): (Vec<_>, Vec<_>) = chunk.iter().partition(|(_, member)| *member);
        host.change_group_members(GroupMembersChange {
            resource_key: resource_key.to_owned(),
            add: add.into_iter().map(|(id, _)| (*id).to_owned()).collect(),
            remove: remove.into_iter().map(|(id, _)| (*id).to_owned()).collect(),
        })
        .await?;
    }
    Ok(())
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

    /// 每轮维护和保存设置后调用；`force` 时全量重读判定并同步所有账号
    ///
    /// # Errors
    /// 读设置或账号失败时返回错误；分组操作失败记在状态里
    pub async fn sync(
        &self,
        runner: &Runner,
        host: &HostClient,
        accounts: &[AuthRuntimeAccount],
        force: bool,
    ) -> Result<(), PluginFault> {
        let _guard = self.sync.lock().await;
        let (config, _) = store::group_config(host).await?;
        // 关闭期间测完的账号也要记下，开启时一并处理
        let finished = runner.take_turnstate_finished();
        if !config.enabled && config.retired.is_empty() {
            return Ok(());
        }
        let now = now_ms();
        let full = force || now - lock(&self.state).full_ms >= FULL_SYNC_MS;
        let ids: Vec<&str> = accounts.iter().map(|a| a.account_id.as_str()).collect();

        let retired_error = if config.retired.is_empty() {
            None
        } else {
            self.clear_retired(host, &config, &ids).await?
        };
        lock(&self.state).status.error.clone_from(&retired_error);
        if !config.enabled {
            return Ok(());
        }

        // 要重读判定的账号
        let reread: Vec<&AuthRuntimeAccount> = accounts
            .iter()
            .filter(|a| supported(a) && (full || finished.contains(&a.account_id)))
            .collect();
        if !full && reread.is_empty() {
            return Ok(());
        }
        let mut changed: BTreeSet<String> = BTreeSet::new();
        let mut levels = HashMap::with_capacity(reread.len());
        for a in &reread {
            let (_, health) =
                store::history_with_health(host, &a.account_id, Method::Turnstate).await?;
            levels.insert(
                a.account_id.clone(),
                health.map_or(Level::Unknown, |h| h.health.level),
            );
        }
        {
            let mut state = lock(&self.state);
            if full {
                state.levels.retain(|id, _| levels.contains_key(id));
            }
            for (id, level) in levels {
                if state.levels.insert(id.clone(), level) != Some(level) {
                    changed.insert(id);
                }
            }
        }
        if !full && changed.is_empty() {
            return Ok(());
        }
        let levels = lock(&self.state).levels.clone();
        // 全量时同步所有账号（不支持探测的移出），否则只同步判定变了的账号
        let targets: Vec<&str> = if full {
            ids.clone()
        } else {
            changed.iter().map(String::as_str).collect()
        };

        let mut statuses = BTreeMap::new();
        // 只有改成员失败才提前重试；建组失败（多半重名）要等用户改规则，按常规间隔重试
        let mut retry = false;
        for rule in config.rules.iter().filter(|r| r.group_id.is_none()) {
            let mut status = RuleStatus {
                members: levels.values().filter(|l| rule.levels.contains(l)).count(),
                ..RuleStatus::default()
            };
            let result = async {
                let group = host
                    .ensure_group(GroupEnsureRequest {
                        resource_key: rule.id.clone(),
                        name: rule.name.clone(),
                        color: rule.color.clone(),
                        description: Some("账号探测插件按测票当前判定自动维护成员".into()),
                    })
                    .await?;
                lock(&self.state).owned_group_ids.insert(group.id.clone());
                status.group_id = group.id;
                status.group_name = group.name;
                let members: Vec<(&str, bool)> = targets
                    .iter()
                    .map(|id| {
                        (
                            *id,
                            levels.get(*id).is_some_and(|l| rule.levels.contains(l)),
                        )
                    })
                    .collect();
                set_members(host, &rule.id, &members)
                    .await
                    .inspect_err(|_| retry = true)
            }
            .await;
            if let Err(error) = result {
                status.error = Some(host_message(&error));
            }
            statuses.insert(rule.id.clone(), status);
        }
        let existing: Vec<&GroupRule> = config
            .rules
            .iter()
            .filter(|r| r.group_id.is_some())
            .collect();
        if !existing.is_empty() {
            let changed: BTreeSet<&str> = changed.iter().map(String::as_str).collect();
            if let Err(reason) = sync_existing(
                runner,
                host,
                &existing,
                &levels,
                (!full).then_some(&changed),
                &mut statuses,
            )
            .await
            {
                retry = true;
                for rule in &existing {
                    statuses.entry(rule.id.clone()).or_default().error = Some(reason.clone());
                }
            }
        }
        let mut state = lock(&self.state);
        // 改成员失败的，一分钟后全量重试，保证成员最终一致
        state.full_ms = if retry {
            now - FULL_SYNC_MS + RETRY_MS
        } else if full {
            now
        } else {
            state.full_ms
        };
        state.status = Status {
            synced_ms: now,
            error: retired_error,
            rules: statuses,
        };
        Ok(())
    }

    /// 清空删掉的规则的成员；返回清不掉的原因
    async fn clear_retired(
        &self,
        host: &HostClient,
        config: &GroupConfig,
        ids: &[&str],
    ) -> Result<Option<String>, PluginFault> {
        let members: Vec<(&str, bool)> = ids.iter().map(|id| (*id, false)).collect();
        let mut cleared = Vec::new();
        let mut error = None;
        for id in &config.retired {
            match set_members(host, id, &members).await {
                Ok(()) => cleared.push(id.clone()),
                // 分组从没建过或已在 CPR 删掉（宿主报冲突）：没什么可清的
                Err(e)
                    if matches!(
                        e.code,
                        ErrorCode::Conflict | ErrorCode::PermissionDenied | ErrorCode::Rejected
                    ) =>
                {
                    cleared.push(id.clone());
                }
                Err(e) => error = Some(format!("清空已删除规则的分组失败：{}", host_message(&e))),
            }
        }
        if !cleared.is_empty() {
            let _ = store::update_group_config(host, |c| {
                c.retired.retain(|id| !cleared.contains(id));
                Ok(())
            })
            .await?;
        }
        Ok(error)
    }
}

/// 经 CPR 管理员接口调整现有分组：读账号现在的全部分组，换掉规则管的那几个，整体写回。
/// `only` 有值时只处理这些账号（刚测完、判定变了的），否则处理全部账号
async fn sync_existing(
    runner: &Runner,
    host: &HostClient,
    rules: &[&GroupRule],
    levels: &HashMap<String, Level>,
    only: Option<&BTreeSet<&str>>,
    statuses: &mut BTreeMap<String, RuleStatus>,
) -> Result<(), String> {
    let admin = Admin::new(host, &runner.config)?;
    let groups = admin.groups().await?;
    // 分组在 CPR 里删掉了的规则不动，其余照常
    let mut managed: Vec<(&GroupRule, &str)> = Vec::new();
    for rule in rules {
        let gid = rule.group_id.as_deref().unwrap_or_default();
        let status = statuses.entry(rule.id.clone()).or_default();
        status.group_id = gid.to_owned();
        status.members = levels.values().filter(|l| rule.levels.contains(l)).count();
        match groups.iter().find(|g| g.id == gid) {
            Some(group) => {
                status.group_name.clone_from(&group.name);
                managed.push((rule, gid));
            }
            None => {
                status.group_name.clone_from(&rule.name);
                status.error = Some("CPR 里找不到这个分组（可能已删除），请删掉这条规则".into());
            }
        }
    }
    if managed.is_empty() {
        return Ok(());
    }
    let managed_ids: BTreeSet<&str> = managed.iter().map(|(_, gid)| *gid).collect();
    // 结果相同的账号合成一批写
    let mut batches: BTreeMap<Vec<String>, Vec<String>> = BTreeMap::new();
    for account in admin.accounts().await? {
        if only.is_some_and(|only| !only.contains(account.id.as_str())) {
            continue;
        }
        let current: BTreeSet<&str> = account.groups.iter().map(|g| g.id.as_str()).collect();
        let level = levels.get(&account.id);
        let mut desired: BTreeSet<&str> = current
            .iter()
            .copied()
            .filter(|g| !managed_ids.contains(g))
            .collect();
        for (rule, gid) in &managed {
            if level.is_some_and(|l| rule.levels.contains(l)) {
                desired.insert(gid);
            }
        }
        if desired != current {
            batches
                .entry(desired.into_iter().map(str::to_owned).collect())
                .or_default()
                .push(account.id.clone());
        }
    }
    for (group_ids, account_ids) in &batches {
        admin.set_groups(account_ids, group_ids).await?;
    }
    Ok(())
}
