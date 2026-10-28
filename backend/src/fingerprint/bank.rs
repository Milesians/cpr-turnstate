//! ModelTrace 指纹库（`static/data/unified_bank.json`）的读取与校验

use serde::Deserialize;
use std::collections::HashMap;

use super::{DIMENSION, ORDERED_DIMENSION};

/// 内置的一份指纹库，GitHub 同步失败或关闭同步时使用
pub const EMBEDDED_BANK: &[u8] = include_bytes!("../../data/unified_bank.json");
/// 内置指纹库对应的 ModelTrace 提交
pub const EMBEDDED_COMMIT: &str = "6cd4a8ff0f3f4e49d6d6969dd2fbadfa1569fde2";

/// 指纹库；质心按 `models` 的顺序排列
#[derive(Debug, Deserialize)]
pub struct Bank {
    pub models: Vec<BankModel>,
    pub(super) robust: Robust,
    pub(super) calibration: HashMap<String, Calibration>,
}

#[derive(Debug, Deserialize)]
pub struct BankModel {
    pub id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub family: String,
    #[serde(default)]
    pub family_name: String,
    pub counts: Vec<f64>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Calibration {
    pub beta: f64,
    #[serde(default)]
    pub cv_accuracy: f64,
}

#[derive(Debug, Deserialize)]
pub(super) struct Robust {
    pub(super) hellinger: Hellinger,
    #[serde(default)]
    pub(super) ordered_blocks: Option<OrderedBlocks>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Hellinger {
    pub(super) feature_mean: Vec<f64>,
    pub(super) feature_scale: Vec<f64>,
    #[serde(default)]
    pub(super) nuisance_basis: Vec<Vec<f64>>,
    pub(super) centroids: Vec<Vec<f64>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct OrderedBlocks {
    #[serde(default)]
    pub(super) weight: f64,
    pub(super) feature_mean: Vec<f64>,
    pub(super) feature_scale: Vec<f64>,
    #[serde(default)]
    pub(super) nuisance_basis: Vec<Vec<f64>>,
    pub(super) centroids: Vec<Vec<f64>>,
    #[serde(default)]
    pub(super) environment_centroids: Vec<Vec<Vec<f64>>>,
}

impl Bank {
    /// 解析并校验评分代码会索引到的每个维度，坏库在加载时失败而不是评分时越界
    ///
    /// # Errors
    /// JSON 无效或维度不一致时返回说明
    pub fn parse(data: &[u8]) -> Result<Self, String> {
        let bank: Self = serde_json::from_slice(data)
            .map_err(|error| format!("指纹库不是有效 JSON：{error}"))?;
        bank.validate()
            .map_err(|error| format!("指纹库无效：{error}"))?;
        Ok(bank)
    }

    /// 解析内置的指纹库
    ///
    /// # Panics
    /// 内置文件损坏时，由单元测试保证不会发生
    #[must_use]
    pub fn embedded() -> Self {
        Self::parse(EMBEDDED_BANK).expect("embedded fingerprint bank is valid")
    }

    /// 库里有没有这个模型
    #[must_use]
    pub fn has_model(&self, id: &str) -> bool {
        self.models.iter().any(|model| model.id == id)
    }

    fn validate(&self) -> Result<(), String> {
        let n = self.models.len();
        if n == 0 {
            return Err("没有模型".into());
        }
        for model in &self.models {
            if model.counts.len() != DIMENSION {
                return Err(format!(
                    "模型 {} 的 counts 有 {} 项，应为 {DIMENSION}",
                    model.id,
                    model.counts.len()
                ));
            }
        }
        let h = &self.robust.hellinger;
        if h.feature_mean.len() != DIMENSION || h.feature_scale.len() != DIMENSION {
            return Err(format!("hellinger 特征均值/尺度应为 {DIMENSION} 项"));
        }
        check_matrix(
            "hellinger nuisance_basis",
            &h.nuisance_basis,
            None,
            DIMENSION,
        )?;
        check_matrix("hellinger centroids", &h.centroids, Some(n), DIMENSION)?;
        if let Some(ob) = &self.robust.ordered_blocks {
            if ob.feature_mean.len() != ORDERED_DIMENSION
                || ob.feature_scale.len() != ORDERED_DIMENSION
            {
                return Err(format!(
                    "ordered_blocks 特征均值/尺度应为 {ORDERED_DIMENSION} 项"
                ));
            }
            check_matrix(
                "ordered_blocks nuisance_basis",
                &ob.nuisance_basis,
                None,
                ORDERED_DIMENSION,
            )?;
            check_matrix(
                "ordered_blocks centroids",
                &ob.centroids,
                Some(n),
                ORDERED_DIMENSION,
            )?;
            for (i, env) in ob.environment_centroids.iter().enumerate() {
                check_matrix(
                    &format!("ordered_blocks environment_centroids[{i}]"),
                    env,
                    Some(n),
                    ORDERED_DIMENSION,
                )?;
            }
            if ob.weight != 0.0 && ob.environment_centroids.is_empty() {
                return Err("ordered_blocks 有权重但没有 environment_centroids".into());
            }
        }
        for key in ["1", "2", "3"] {
            match self.calibration.get(key) {
                None => return Err(format!("缺少 calibration[{key}]")),
                Some(c) if c.beta <= 0.0 || !c.beta.is_finite() => {
                    return Err(format!("calibration[{key}].beta 必须为正数"));
                }
                Some(_) => {}
            }
        }
        Ok(())
    }
}

fn check_matrix(
    name: &str,
    matrix: &[Vec<f64>],
    rows: Option<usize>,
    width: usize,
) -> Result<(), String> {
    if let Some(rows) = rows
        && matrix.len() != rows
    {
        return Err(format!("{name} 有 {} 行，应为 {rows}", matrix.len()));
    }
    for (i, row) in matrix.iter().enumerate() {
        if row.len() != width {
            return Err(format!("{name} 第 {i} 行有 {} 项，应为 {width}", row.len()));
        }
    }
    Ok(())
}
