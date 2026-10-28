//! 按回答里的数字分布判断是哪个模型
//!
//! 移植自 ModelTrace 的 `static/fingerprint-core.js`
//! （<https://github.com/xqy2006/ModelTrace>，MIT，见 `data/LICENSE.ModelTrace`），
//! 经 ezou-manager 的 Go 版本转写。浮点运算保持原实现的顺序，结果用 JS 生成的
//! 黄金向量逐项核对（`tests/fingerprint.rs`）。

mod bank;
mod challenge;

pub use bank::{Bank, BankModel, Calibration, EMBEDDED_BANK, EMBEDDED_COMMIT};
pub use challenge::{Challenge, generate_challenges, system_random};
use serde::Serialize;

pub const VALUE_MIN: usize = 1;
pub const VALUE_MAX: usize = 355;
/// 不同取值的个数
pub const DIMENSION: usize = VALUE_MAX - VALUE_MIN + 1;

const ALPHA: f64 = 0.5;
const ORDERED_BINS: usize = 16;
const ORDERED_LAST_DIGIT: usize = 10;
pub(crate) const ORDERED_DIMENSION: usize = 4 * ORDERED_BINS + ORDERED_LAST_DIGIT;
const MIN_VALID_NUMBERS: usize = 80;
const MIN_VALID_RATIO: f64 = 0.55;

/// 指纹库没有的模型按另一个模型的指纹判定；每一项都要有依据
const ALIASES: &[(&str, &str)] = &[
    // 2026-10-18 确认 gpt-6.1-sol 回答 ModelTrace 题目的方式和 gpt-6-astra 一样
    ("gpt-6.1-sol", "gpt-6-astra"),
];

/// 判定 `model` 时使用的库内模型：它自己，或它的别名目标
#[must_use]
pub fn fingerprint_of(model: &str) -> &str {
    ALIASES
        .iter()
        .find(|(alias, _)| *alias == model)
        .map_or(model, |(_, target)| target)
}

/// 一份回答；`expected_count` 是题目要求的个数，未知为 0
pub struct Output<'a> {
    pub text: &'a str,
    pub expected_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub index: usize,
    pub parsed_numbers: usize,
    pub minimum_numbers: usize,
    pub accepted: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelResult {
    pub model: String,
    pub display_name: String,
    pub probability: f64,
    pub profile_similarity: f64,
    pub score: f64,
    pub family: String,
    pub family_name: String,
    pub conditional_probability: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FamilyProbability {
    pub family: String,
    pub display_name: String,
    pub probability: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CalibrationInfo {
    pub queries: String,
    pub beta: f64,
    pub cv_accuracy: f64,
}

/// 归属结果。概率是闭集的：库里没有的模型会被归到最相似的候选
#[derive(Debug, Clone, Serialize)]
pub struct Analysis {
    pub prediction: String,
    pub prediction_name: String,
    pub probability: f64,
    pub used_outputs: usize,
    pub results: Vec<ModelResult>,
    pub diagnostics: Vec<Diagnostic>,
    pub calibration: CalibrationInfo,
    pub family_prediction: String,
    pub family_prediction_name: String,
    pub family_probability: f64,
    pub family_probabilities: Vec<FamilyProbability>,
}

/// 回答里最长的一段在范围内的整数。两个数字之间出现字母就断开；超出范围的数字跳过但不断开
#[must_use]
pub fn parse_numbers(text: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let (mut best, mut current) = (Vec::new(), Vec::new());
    let mut previous_end = 0;
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if !current.is_empty() && contains_letter(&text[previous_end..start]) {
            if current.len() > best.len() {
                best = std::mem::take(&mut current);
            } else {
                current.clear();
            }
        }
        if let Some(value) = in_range(&text[start..i]) {
            current.push(value);
        }
        previous_end = i;
    }
    if current.len() > best.len() {
        best = current;
    }
    best
}

fn contains_letter(s: &str) -> bool {
    s.chars().any(char::is_alphabetic)
}

fn in_range(digits: &str) -> Option<usize> {
    let trimmed = digits.trim_start_matches('0');
    if trimmed.is_empty() || trimmed.len() > 3 {
        return None;
    }
    let value: usize = trimmed.parse().ok()?;
    (VALUE_MIN..=VALUE_MAX).contains(&value).then_some(value)
}

fn count_numbers(numbers: &[usize]) -> Vec<f64> {
    let mut counts = vec![0.0; DIMENSION];
    for &n in numbers {
        counts[n - VALUE_MIN] += 1.0;
    }
    counts
}

fn sum(values: &[f64]) -> f64 {
    let mut total = 0.0;
    for v in values {
        total += v;
    }
    total
}

#[allow(clippy::cast_precision_loss)]
fn mean(values: &[f64]) -> f64 {
    sum(values) / values.len() as f64
}

fn standardize(values: &[f64]) -> Vec<f64> {
    let center = mean(values);
    let squares: Vec<f64> = values.iter().map(|v| (v - center) * (v - center)).collect();
    let scale = mean(&squares).sqrt().max(1e-12);
    values.iter().map(|v| (v - center) / scale).collect()
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    let mut v = 0.0;
    for i in 0..a.len() {
        v += a[i] * b[i];
    }
    v
}

fn normalized(values: &[f64]) -> Vec<f64> {
    let scale = dot(values, values).sqrt().max(1e-12);
    values.iter().map(|v| v / scale).collect()
}

fn subtract_basis(values: &[f64], basis: &[Vec<f64>]) -> Vec<f64> {
    let mut out = values.to_vec();
    for vector in basis {
        let projection = dot(&out, vector);
        for i in 0..out.len() {
            out[i] -= projection * vector[i];
        }
    }
    out
}

#[allow(clippy::cast_precision_loss)]
fn hellinger_feature(counts: &[f64]) -> Vec<f64> {
    let total = sum(counts) + ALPHA * DIMENSION as f64;
    counts
        .iter()
        .map(|v| ((v + ALPHA) / total).sqrt())
        .collect()
}

fn split_into_four(values: &[usize]) -> [&[usize]; 4] {
    let (base, remainder) = (values.len() / 4, values.len() % 4);
    let mut chunks: [&[usize]; 4] = [&[]; 4];
    let mut start = 0;
    for (i, chunk) in chunks.iter_mut().enumerate() {
        let size = base + usize::from(i < remainder);
        *chunk = &values[start..start + size];
        start += size;
    }
    chunks
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn ordered_block_feature(numbers: &[usize]) -> Vec<f64> {
    let mut pieces = Vec::with_capacity(ORDERED_DIMENSION);
    for chunk in split_into_four(numbers) {
        let mut bins = vec![0.5; ORDERED_BINS];
        for &v in chunk {
            let index = (((v - 1) as f64 / 355.0) * ORDERED_BINS as f64).floor() as usize;
            bins[index.min(ORDERED_BINS - 1)] += 1.0;
        }
        let total = sum(&bins);
        pieces.extend(bins.iter().map(|v| (v / total).sqrt()));
    }
    let mut last = vec![0.5; ORDERED_LAST_DIGIT];
    for &v in numbers {
        last[v % 10] += 1.0;
    }
    let total = sum(&last);
    pieces.extend(last.iter().map(|v| (v / total).sqrt()));
    pieces
}

fn softmax(values: &[f64]) -> Vec<f64> {
    let maximum = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let weights: Vec<f64> = values.iter().map(|v| (v - maximum).exp()).collect();
    let total = sum(&weights);
    weights.iter().map(|w| w / total).collect()
}

/// 1 减去观测计数与模型参考计数之间的归一化 Jensen-Shannon 距离
#[allow(clippy::cast_precision_loss)]
fn js_similarity(left: &[f64], right: &[f64]) -> f64 {
    let left_total = sum(left);
    let right_total = sum(right) + ALPHA * DIMENSION as f64;
    let p: Vec<f64> = left.iter().map(|v| v / left_total).collect();
    let q: Vec<f64> = right.iter().map(|v| (v + ALPHA) / right_total).collect();
    let midpoint: Vec<f64> = p.iter().zip(&q).map(|(a, b)| (a + b) / 2.0).collect();
    let divergence = |values: &[f64]| {
        let mut total = 0.0;
        for (i, &v) in values.iter().enumerate() {
            if v != 0.0 {
                total += v * (v / midpoint[i]).ln();
            }
        }
        total
    };
    let js = (divergence(&p) + divergence(&q)) / 2.0;
    1.0 - (js / std::f64::consts::LN_2).sqrt()
}

fn first_non_empty<'a>(a: &'a str, b: &'a str) -> &'a str {
    if a.is_empty() { b } else { a }
}

fn family_of(model: &BankModel) -> &str {
    first_non_empty(&model.family, "models")
}

impl Bank {
    fn robust_score_counts(&self, counts: &[f64]) -> Vec<f64> {
        let h = &self.robust.hellinger;
        let feature = hellinger_feature(counts);
        let projected: Vec<f64> = feature
            .iter()
            .enumerate()
            .map(|(i, v)| (v - h.feature_mean[i]) / h.feature_scale[i])
            .collect();
        let projected = normalized(&subtract_basis(&projected, &h.nuisance_basis));
        let scores: Vec<f64> = h.centroids.iter().map(|c| dot(&projected, c)).collect();
        standardize(&scores)
    }

    fn ordered_block_scores(&self, ob: &bank::OrderedBlocks, numbers: &[usize]) -> Vec<f64> {
        let feature = ordered_block_feature(numbers);
        let std: Vec<f64> = feature
            .iter()
            .enumerate()
            .map(|(i, v)| (v - ob.feature_mean[i]) / ob.feature_scale[i])
            .collect();
        let unit = normalized(&std);
        let models = ob.centroids.len();
        let template: Vec<f64> = (0..models)
            .map(|m| {
                ob.environment_centroids
                    .iter()
                    .map(|env| dot(&unit, &env[m]))
                    .fold(f64::NEG_INFINITY, |best, s| if s > best { s } else { best })
            })
            .collect();
        let template = standardize(&template);
        let projected = normalized(&subtract_basis(&std, &ob.nuisance_basis));
        let nuisance: Vec<f64> = ob.centroids.iter().map(|c| dot(&projected, c)).collect();
        let nuisance = standardize(&nuisance);
        let fused: Vec<f64> = template
            .iter()
            .zip(&nuisance)
            .map(|(t, n)| 0.5 * t + 0.5 * n)
            .collect();
        standardize(&fused)
    }

    fn robust_score_numbers(&self, numbers: &[usize]) -> Vec<f64> {
        let marginal = self.robust_score_counts(&count_numbers(numbers));
        match &self.robust.ordered_blocks {
            Some(ob) if ob.weight != 0.0 => {
                let ordered = self.ordered_block_scores(ob, numbers);
                marginal
                    .iter()
                    .zip(&ordered)
                    .map(|(m, o)| (1.0 - ob.weight) * m + ob.weight * o)
                    .collect()
            }
            _ => marginal,
        }
    }

    /// 把回答归属到候选模型。数字太少的回答记在诊断里、不计入；一份可用的都没有时返回 `None`
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    pub fn analyze(&self, outputs: &[Output<'_>]) -> (Vec<Diagnostic>, Option<Analysis>) {
        struct Scored {
            counts: Vec<f64>,
            scores: Vec<f64>,
        }
        let mut valid = Vec::new();
        let mut diagnostics = Vec::with_capacity(outputs.len());
        for (index, output) in outputs.iter().enumerate() {
            let numbers = parse_numbers(output.text);
            let minimum = if output.expected_count > 0 {
                MIN_VALID_NUMBERS
                    .max((output.expected_count as f64 * MIN_VALID_RATIO).ceil() as usize)
            } else {
                MIN_VALID_NUMBERS
            };
            let accepted = numbers.len() >= minimum;
            diagnostics.push(Diagnostic {
                index,
                parsed_numbers: numbers.len(),
                minimum_numbers: minimum,
                accepted,
            });
            if accepted {
                valid.push(Scored {
                    counts: count_numbers(&numbers),
                    scores: self.robust_score_numbers(&numbers),
                });
            }
        }
        if valid.is_empty() {
            return (diagnostics, None);
        }

        let models = self.models.len();
        let combined: Vec<f64> = (0..models)
            .map(|m| mean(&valid.iter().map(|v| v.scores[m]).collect::<Vec<_>>()))
            .collect();
        let key = valid.len().min(3).to_string();
        let calibration = self.calibration[&key];
        let scaled: Vec<f64> = combined.iter().map(|v| calibration.beta * v).collect();
        let probabilities = softmax(&scaled);

        let mut pooled = vec![0.0; DIMENSION];
        for (i, slot) in pooled.iter_mut().enumerate() {
            for v in &valid {
                *slot += v.counts[i];
            }
        }

        let mut family_order: Vec<String> = Vec::new();
        let mut family_names: Vec<String> = Vec::new();
        for model in &self.models {
            let family = family_of(model);
            if !family_order.iter().any(|f| f == family) {
                family_order.push(family.to_owned());
                family_names.push(first_non_empty(&model.family_name, family).to_owned());
            }
        }
        let family_name = |family: &str| {
            let i = family_order.iter().position(|f| f == family).unwrap_or(0);
            family_names[i].clone()
        };

        let mut results: Vec<ModelResult> = self
            .models
            .iter()
            .enumerate()
            .map(|(i, model)| ModelResult {
                model: model.id.clone(),
                display_name: model.display_name.clone(),
                probability: probabilities[i],
                profile_similarity: js_similarity(&pooled, &model.counts),
                score: combined[i],
                family: family_of(model).to_owned(),
                family_name: family_name(family_of(model)),
                conditional_probability: 0.0,
            })
            .collect();
        // 稳定排序，与原实现一致
        results.sort_by(|a, b| {
            b.probability
                .partial_cmp(&a.probability)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let family_probability: Vec<f64> = family_order
            .iter()
            .map(|family| {
                let mut total = 0.0;
                for r in &results {
                    if &r.family == family {
                        total += r.probability;
                    }
                }
                total
            })
            .collect();
        for r in &mut results {
            let i = family_order
                .iter()
                .position(|f| f == &r.family)
                .unwrap_or(0);
            r.conditional_probability = r.probability / family_probability[i];
        }
        let mut winning = 0;
        for i in 0..family_order.len() {
            if family_probability[i] > family_probability[winning] {
                winning = i;
            }
        }
        let family_probabilities = family_order
            .iter()
            .enumerate()
            .map(|(i, family)| FamilyProbability {
                family: family.clone(),
                display_name: family_names[i].clone(),
                probability: family_probability[i],
            })
            .collect();

        let analysis = Analysis {
            prediction: results[0].model.clone(),
            prediction_name: results[0].display_name.clone(),
            probability: results[0].probability,
            used_outputs: valid.len(),
            results,
            diagnostics: diagnostics.clone(),
            calibration: CalibrationInfo {
                queries: key,
                beta: calibration.beta,
                cv_accuracy: calibration.cv_accuracy,
            },
            family_prediction: family_order[winning].clone(),
            family_prediction_name: family_names[winning].clone(),
            family_probability: family_probability[winning],
            family_probabilities,
        };
        (diagnostics, Some(analysis))
    }
}
