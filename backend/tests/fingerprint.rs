use cpr_account_probe::fingerprint::{Bank, Output, generate_challenges, parse_numbers};
use cpr_account_probe::modeltrace::{self, Completion};
use cpr_account_probe::verdict::Level;
use serde_json::Value;

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * 1_f64.max(a.abs()).max(b.abs())
}

#[test]
fn embedded_bank_loads() {
    let bank = Bank::embedded();
    assert_eq!(bank.models.len(), 18);
}

#[test]
fn rejects_bad_banks() {
    for doc in [
        "{",
        r#"{"models":[]}"#,
        r#"{"models":[{"id":"a","counts":[1,2,3]}]}"#,
    ] {
        assert!(Bank::parse(doc.as_bytes()).is_err(), "{doc}");
    }
}

#[test]
fn parse_numbers_matches_the_go_port() {
    let cases: &[(&str, &[usize])] = &[
        ("[1, 2, 355]", &[1, 2, 355]),
        ("5, 0, 356, 1000, 7", &[5, 7]),
        ("007 042 000", &[7, 42]),
        ("99999999999999999999, 3", &[3]),
        ("1 2 and 3 4 5", &[3, 4, 5]),
        ("1 2 then 3 4", &[1, 2]),
        ("1、2，3", &[1, 2, 3]),
        ("第1个是12，然后是34", &[1]),
        ("第1个是12，然后是34 56 78", &[34, 56, 78]),
        ("１２３ 4", &[4]),
        ("none", &[]),
        ("", &[]),
    ];
    for (text, want) in cases {
        assert_eq!(parse_numbers(text), *want, "{text}");
    }
}

/// 黄金向量由 ModelTrace 自己的 JS 实现生成（ezou-manager 的 scripts/gen-fingerprint-golden.mjs）
#[test]
fn golden_against_modeltrace_js() {
    let raw = std::fs::read("tests/data/golden.json").unwrap();
    let golden: Value = serde_json::from_slice(&raw).unwrap();
    let bank = Bank::embedded();
    let cases = golden["cases"].as_array().unwrap();
    assert!(cases.len() >= 50);
    let (mut real_total, mut real_correct) = (0, 0);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let texts: Vec<(String, usize)> = case["outputs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| {
                (
                    o["text"].as_str().unwrap().to_owned(),
                    usize::try_from(o["expected_count"].as_u64().unwrap_or(0)).unwrap(),
                )
            })
            .collect();
        let outputs: Vec<Output<'_>> = texts
            .iter()
            .map(|(t, n)| Output {
                text: t,
                expected_count: *n,
            })
            .collect();
        let (_, got) = bank.analyze(&outputs);
        let want = &case["expected"];
        if want.get("error").is_some() {
            assert!(
                got.is_none(),
                "{name}: JS rejected the outputs but Rust accepted them"
            );
            continue;
        }
        let got = got.unwrap_or_else(|| panic!("{name}: JS accepted the outputs but Rust did not"));
        assert_eq!(
            got.prediction,
            want["prediction"].as_str().unwrap(),
            "{name}"
        );
        assert_eq!(
            got.family_prediction,
            want["family_prediction"].as_str().unwrap(),
            "{name}"
        );
        assert_eq!(
            got.used_outputs as u64,
            want["used_outputs"].as_u64().unwrap(),
            "{name}"
        );
        assert!(
            near(got.probability, want["probability"].as_f64().unwrap()),
            "{name}"
        );
        assert!(
            near(
                got.family_probability,
                want["family_probability"].as_f64().unwrap()
            ),
            "{name}"
        );
        assert_eq!(
            got.calibration.queries,
            want["calibration"]["queries"].as_str().unwrap(),
            "{name}"
        );
        for (i, d) in want["diagnostics"].as_array().unwrap().iter().enumerate() {
            assert_eq!(
                got.diagnostics[i].parsed_numbers as u64,
                d["parsed_numbers"].as_u64().unwrap(),
                "{name}"
            );
            assert_eq!(
                got.diagnostics[i].accepted,
                d["accepted"].as_bool().unwrap(),
                "{name}"
            );
        }
        let results = want["results"].as_array().unwrap();
        assert_eq!(got.results.len(), results.len());
        for (g, w) in got.results.iter().zip(results) {
            assert_eq!(g.model, w["model"].as_str().unwrap(), "{name}");
            assert_eq!(g.family_name, w["family_name"].as_str().unwrap(), "{name}");
            for (field, value) in [
                ("probability", g.probability),
                ("profile_similarity", g.profile_similarity),
                ("score", g.score),
                ("conditional_probability", g.conditional_probability),
            ] {
                assert!(
                    near(value, w[field].as_f64().unwrap()),
                    "{name} {} {field}: {value} vs {}",
                    g.model,
                    w[field]
                );
            }
        }
        if let Some(model) = name.strip_prefix("real/") {
            real_total += 1;
            if model.starts_with(&format!("{}/", got.prediction)) {
                real_correct += 1;
            }
        }
    }
    assert!(real_total > 0);
    assert!(
        real_correct * 10 >= real_total * 9,
        "{real_correct}/{real_total}"
    );
}

#[test]
fn challenges_have_distinct_lengths_and_are_deterministic() {
    let mut counter = 0_u8;
    let mut fixed = |buf: &mut [u8]| {
        for b in buf {
            *b = counter;
            counter = counter.wrapping_add(37);
        }
        Ok(())
    };
    let a = generate_challenges(&mut fixed, 3).unwrap();
    let mut counter = 0_u8;
    let mut fixed = |buf: &mut [u8]| {
        for b in buf {
            *b = counter;
            counter = counter.wrapping_add(37);
        }
        Ok(())
    };
    let b = generate_challenges(&mut fixed, 3).unwrap();
    assert_eq!(a, b);
    let mut lengths: Vec<usize> = a.iter().map(|c| c.expected_count).collect();
    lengths.sort_unstable();
    lengths.dedup();
    assert_eq!(lengths.len(), 3);
    for c in &a {
        assert!((292..=332).contains(&c.expected_count));
        assert!(c.prompt.contains(&format!(
            " {} 个 1 到 355（含端点）的整数",
            c.expected_count
        )));
        assert!(c.prompt.contains("禁止调用或借助任何工具"));
        assert!(c.id.starts_with("probe-"));
    }
    let mut empty = |_: &mut [u8]| Err("exhausted".to_owned());
    assert!(generate_challenges(&mut empty, 3).is_err());
    assert!(generate_challenges(&mut fixed, 0).is_err());
    assert!(generate_challenges(&mut fixed, 42).is_err());
}

#[test]
fn modeltrace_verdicts() {
    let bank = Bank::embedded();
    let challenges = modeltrace::challenges().unwrap();
    // 拒答：没有数字
    let refused: Vec<Completion> = challenges
        .iter()
        .map(|_| Completion {
            text: "抱歉".into(),
            ..Completion::default()
        })
        .collect();
    let r = modeltrace::analyze(&bank, "c", "gpt-6-astra", &challenges, &refused);
    assert_eq!(r.used_outputs, 0);
    assert_eq!(modeltrace::verdict(&r).level, Level::Unknown);
    // 库外模型一律未判定
    let numbers: Vec<Completion> = challenges
        .iter()
        .enumerate()
        .map(|(i, c)| Completion {
            text: (0..c.expected_count)
                .map(|n| ((n * 37 + i * 11) % 355 + 1).to_string())
                .collect::<Vec<_>>()
                .join(","),
            ..Completion::default()
        })
        .collect();
    let r = modeltrace::analyze(&bank, "c", "not-in-bank", &challenges, &numbers);
    assert!(!r.in_bank);
    assert_eq!(modeltrace::verdict(&r).level, Level::Unknown);
    // 别名按目标的指纹判定
    let r = modeltrace::analyze(&bank, "c", "gpt-6.1-sol", &challenges, &numbers);
    assert_eq!(r.fingerprint, "gpt-6-astra");
    assert!(r.in_bank);
    let total: f64 = r.candidates.iter().map(|c| c.probability).sum();
    assert!(total <= 1.0 + 1e-9);
}
