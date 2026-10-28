//! ModelTrace 的题目生成，对应 `static/challenge-browser.js`

use serde::{Deserialize, Serialize};

/// 发给被测模型的一道题。作为普通用户消息发送：系统提示词会改变模型的数字偏好
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Challenge {
    pub id: String,
    pub expected_count: usize,
    pub prompt: String,
}

const LENGTH_MIN: usize = 292;
const LENGTH_RANGE: usize = 41; // 292..=332

const OPENINGS: &[&str] = &[
    "这是一次独立的数值选择记录",
    "请完成下面的无语义整数选择任务",
    "执行一次第一反应取值记录",
    "生成一组不承载语义的整数选择",
    "进行一轮快速逐项取值",
];
const ACTIONS: &[&str] = &[
    "为各个位置分别凭第一反应选择",
    "逐项选择",
    "每次只决定当前一项，共给出",
    "分别凭第一反应给出",
    "逐个直接选择",
];
const ENDINGS: &[&str] = &[
    "允许某个数字再次出现；每项写出后不要回头排序、去重或替换。",
    "偶然重复是有效的；不要重新排列或修正已经写出的项目。",
    "相同值可以再次出现；输出过程中不要整理或改写前面的项目。",
    "重复值无需删除；不要筛选、重排或补成某种规律。",
    "不必赋予数字任何含义；已经给出的值保持不变。",
];
const SEPARATORS: &[&str] = &[
    "数字之间用逗号或空格分隔均可。",
    "使用一种一致的常见分隔符即可。",
    "可以用逗号、空格或换行分隔。",
    "只要每个整数边界清楚，格式可自行选择。",
];

/// 生成 `count` 道长度互不相同的独立题目。`random` 填满给定缓冲区；生产环境用系统随机数
///
/// # Errors
/// 数量超出范围或随机数源失败时返回错误
pub fn generate_challenges(
    random: &mut dyn FnMut(&mut [u8]) -> Result<(), String>,
    count: usize,
) -> Result<Vec<Challenge>, String> {
    if !(1..=LENGTH_RANGE).contains(&count) {
        return Err(format!("题目数量必须在 1..{LENGTH_RANGE}"));
    }
    let mut available: Vec<usize> = (0..LENGTH_RANGE).map(|i| LENGTH_MIN + i).collect();
    let mut lengths = Vec::with_capacity(count);
    while lengths.len() < count {
        let i = random_index(random, available.len())?;
        lengths.push(available.remove(i));
    }
    let mut out = Vec::with_capacity(count);
    for (i, length) in lengths.into_iter().enumerate() {
        let opening = OPENINGS[random_index(random, OPENINGS.len())?];
        let action = ACTIONS[random_index(random, ACTIONS.len())?];
        let ending = ENDINGS[random_index(random, ENDINGS.len())?];
        let separator = SEPARATORS[random_index(random, SEPARATORS.len())?];
        let mut suffix = [0_u8; 8];
        random(&mut suffix)?;
        out.push(Challenge {
            id: format!("probe-{}-{}", i + 1, hex::encode(suffix)),
            expected_count: length,
            prompt: format!(
                "{opening}。{action} {length} 个 1 到 355（含端点）的整数。\
                 每个位置都要单独选择；不要从 1 开始计数，不要连续递增或递减，也不要采用等差、循环、重复区块或其他规则化模式。\
                 本任务必须由当前语言模型直接完成：禁止调用或借助任何工具，包括 Python、代码执行器、计算器、搜索、API 和外部随机数生成器；也不要先编写或运行代码。\
                 {ending}{separator}\
                 直接从第一个取值开始输出，不要在序列前重复数量、范围或任务说明。"
            ),
        });
    }
    Ok(out)
}

/// 无取模偏差地从 `[0, n)` 取一个数；与 Go 版本消耗同样的字节
fn random_index(
    random: &mut dyn FnMut(&mut [u8]) -> Result<(), String>,
    n: usize,
) -> Result<usize, String> {
    let n = n as u128;
    let bits = 128 - n.leading_zeros() as usize;
    let byte_len = bits.div_ceil(8).max(1);
    let space = 1_u128 << (byte_len * 8);
    let limit = space - space % n;
    let mut buf = vec![0_u8; byte_len];
    loop {
        random(&mut buf)?;
        let v = buf
            .iter()
            .fold(0_u128, |acc, b| (acc << 8) | u128::from(*b));
        if v < limit {
            #[allow(clippy::cast_possible_truncation)]
            return Ok((v % n) as usize);
        }
    }
}

/// 系统随机数源
///
/// # Errors
/// 操作系统随机数不可用时返回错误
pub fn system_random(buf: &mut [u8]) -> Result<(), String> {
    getrandom::fill(buf).map_err(|error| format!("随机数不可用：{error}"))
}
