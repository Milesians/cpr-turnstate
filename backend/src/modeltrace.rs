//! ModelTrace：三道“选 N 个整数”的题，按数字分布和指纹库比对

use serde::{Deserialize, Serialize};

use crate::fingerprint::{self, Bank, Challenge, FamilyProbability, Output};
use crate::verdict::{Level, Verdict, excerpt};

/// 一次探测发的题数。指纹库的校准针对 1 到 3 份有效回答，3 份最好
pub const CHALLENGE_COUNT: usize = 3;
/// 探测的模型概率达到它算吻合
const OK_PROBABILITY: f64 = 0.5;
/// 判定降智时，指纹对“其实是另一个模型”的把握
const DEGRADED_PROBABILITY: f64 = 0.8;
const TOP_CANDIDATES: usize = 5;

/// 一份回答
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Completion {
    pub text: String,
    /// 上游自称的模型，只作参考：它自己的说法不是证据
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub response_model: String,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub duration_ms: u64,
}

/// 一道题的回答是怎么读的
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputInfo {
    pub expected_count: usize,
    pub parsed_numbers: usize,
    pub accepted: bool,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub response_model: String,
    pub excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub model: String,
    pub display_name: String,
    pub family: String,
    pub probability: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Family {
    pub family: String,
    pub display_name: String,
    pub probability: f64,
}

impl From<FamilyProbability> for Family {
    fn from(f: FamilyProbability) -> Self {
        Self {
            family: f.family,
            display_name: f.display_name,
            probability: f.probability,
        }
    }
}

/// 一次 ModelTrace 的结果
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelTraceResult {
    pub model: String,
    /// 判定所用的库内模型，与 `model` 不同时才有（别名）
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fingerprint: String,
    /// 指纹库是否认识判定所用的模型；不认识时结果只是最像的已知模型，不能证实或否定
    pub in_bank: bool,
    /// 判定所用的指纹库版本（ModelTrace 提交）
    pub bank_commit: String,
    pub used_outputs: usize,
    pub outputs: Vec<OutputInfo>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prediction: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prediction_name: String,
    pub probability: f64,
    pub probability_of_probed: f64,
    pub candidates: Vec<Candidate>,
    pub families: Vec<Family>,
}

/// 生成一次探测的题目
///
/// # Errors
/// 随机数不可用时返回错误
pub fn challenges() -> Result<Vec<Challenge>, String> {
    fingerprint::generate_challenges(&mut fingerprint::system_random, CHALLENGE_COUNT)
}

/// 用指纹库判定收齐的回答
#[must_use]
pub fn analyze(
    bank: &Bank,
    bank_commit: &str,
    model: &str,
    challenges: &[Challenge],
    completions: &[Completion],
) -> ModelTraceResult {
    let target = fingerprint::fingerprint_of(model);
    let outputs: Vec<Output<'_>> = challenges
        .iter()
        .zip(completions)
        .map(|(c, out)| Output {
            text: &out.text,
            expected_count: c.expected_count,
        })
        .collect();
    let (diagnostics, analysis) = bank.analyze(&outputs);
    let mut result = ModelTraceResult {
        model: model.to_owned(),
        fingerprint: if target == model {
            String::new()
        } else {
            target.to_owned()
        },
        in_bank: bank.has_model(target),
        bank_commit: bank_commit.to_owned(),
        used_outputs: 0,
        outputs: challenges
            .iter()
            .zip(completions)
            .zip(&diagnostics)
            .map(|((c, out), d)| OutputInfo {
                expected_count: c.expected_count,
                parsed_numbers: d.parsed_numbers,
                accepted: d.accepted,
                output_tokens: out.output_tokens,
                reasoning_tokens: out.reasoning_tokens,
                duration_ms: out.duration_ms,
                response_model: out.response_model.clone(),
                excerpt: excerpt(&out.text, 120),
            })
            .collect(),
        prediction: String::new(),
        prediction_name: String::new(),
        probability: 0.0,
        probability_of_probed: 0.0,
        candidates: Vec::new(),
        families: Vec::new(),
    };
    let Some(analysis) = analysis else {
        return result; // 拒答或被截断：由判定说明，不是错误
    };
    result.used_outputs = analysis.used_outputs;
    result.prediction = analysis.prediction;
    result.prediction_name = analysis.prediction_name;
    result.probability = analysis.probability;
    result.families = analysis
        .family_probabilities
        .into_iter()
        .map(Family::from)
        .collect();
    for (i, r) in analysis.results.into_iter().enumerate() {
        if r.model == target {
            result.probability_of_probed = r.probability;
        }
        if i < TOP_CANDIDATES {
            result.candidates.push(Candidate {
                model: r.model,
                display_name: r.display_name,
                family: r.family,
                probability: r.probability,
            });
        }
    }
    result
}

fn percent(p: f64) -> String {
    format!("{:.0}%", p * 100.0)
}

/// 判定。概率是闭集的：库外模型会被归到最近的候选，所以不认识的模型一律未判定
#[must_use]
pub fn verdict(r: &ModelTraceResult) -> Verdict {
    let valid = format!("{}/{} 份回答有效", r.used_outputs, r.outputs.len());
    let (target, name) = if r.fingerprint.is_empty() {
        (r.model.as_str(), r.model.clone())
    } else {
        (
            r.fingerprint.as_str(),
            format!("{}（按 {} 的指纹）", r.model, r.fingerprint),
        )
    };
    if r.used_outputs == 0 {
        return Verdict::new(
            Level::Unknown,
            format!("没有可用的数字序列（拒答或被截断），无法判断（{valid}）"),
        );
    }
    if !r.in_bank {
        return Verdict::new(
            Level::Unknown,
            format!(
                "指纹库里没有 {name}，最像 {}（{}），不能据此判断",
                r.prediction_name,
                percent(r.probability)
            ),
        );
    }
    if r.prediction == target && r.probability_of_probed >= OK_PROBABILITY {
        return Verdict::new(
            Level::Ok,
            format!(
                "指纹与 {name} 吻合（{}，{valid}）",
                percent(r.probability_of_probed)
            ),
        );
    }
    if r.prediction != target && r.probability >= DEGRADED_PROBABILITY && r.used_outputs >= 2 {
        return Verdict::new(
            Level::Degraded,
            format!(
                "实际更像 {}（{}），不是 {name}（{}）（{valid}）",
                r.prediction_name,
                percent(r.probability),
                percent(r.probability_of_probed)
            ),
        );
    }
    let extra = if r.used_outputs < 2 {
        "，只有 1 份有效回答，结论不稳"
    } else {
        ""
    };
    Verdict::new(
        Level::Suspect,
        format!(
            "与 {name} 的吻合度只有 {}，最像 {}（{}）（{valid}{extra}）",
            percent(r.probability_of_probed),
            r.prediction_name,
            percent(r.probability)
        ),
    )
}
