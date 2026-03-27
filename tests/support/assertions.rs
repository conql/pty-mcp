#![allow(dead_code)]

use anyhow::{Result, anyhow, ensure};
use serde_json::Value;

pub fn assert_names_include<'a>(
    actual: impl IntoIterator<Item = &'a str>,
    expected: &[&str],
    label: &str,
) -> Result<()> {
    let actual = actual.into_iter().collect::<Vec<_>>();
    for name in expected {
        ensure!(
            actual.iter().any(|candidate| candidate == name),
            "{label} missing expected item {name:?}; actual={actual:?}"
        );
    }
    Ok(())
}

pub fn assert_json_array_len_at_least(
    value: &Value,
    field: &str,
    expected_min: usize,
) -> Result<()> {
    let items = value
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("json field {field:?} is not an array: {value}"))?;
    ensure!(
        items.len() >= expected_min,
        "json field {field:?} has len {} which is less than {expected_min}: {value}",
        items.len()
    );
    Ok(())
}

pub fn assert_text_contains(text: &str, needle: &str, label: &str) -> Result<()> {
    ensure!(
        text.contains(needle),
        "{label} missing expected text {needle:?}; actual={text:?}"
    );
    Ok(())
}
