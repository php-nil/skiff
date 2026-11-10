//! 最小 JSON Schema 校验器（P0 专用）。
//!
//! 只实现清单实际用到的关键字：`type`、`required`、`properties`、
//! `additionalProperties`、`items`、`enum`、`const`、`minLength`、`maxLength`、
//! `minimum`、`maximum`、`allOf` 与相对文件的 `$ref`。
//!
//! 选择不引入 JSON Schema 引擎：产物是纯数据、关键字子集固定，自带实现可避免
//! 新增依赖链与版本漂移；不支持的 `$ref` 片段会显式报错，不会静默放过。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::Value;

pub struct SchemaSet {
    dir: PathBuf,
}

impl SchemaSet {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 按 schema 文件名校验一个 JSON 值。
    pub fn validate(&self, schema_file: &str, value: &Value) -> Result<()> {
        let schema = self.load(schema_file)?;
        self.check(&schema, value, "$", 0)
    }

    fn load(&self, schema_file: &str) -> Result<Value> {
        let path = self.dir.join(schema_file);
        let text = fs::read_to_string(&path)
            .with_context(|| format!("读取 schema 失败: {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("解析 schema 失败: {}", path.display()))
    }

    fn check(&self, schema: &Value, value: &Value, path: &str, depth: usize) -> Result<()> {
        if depth > 32 {
            bail!("schema 嵌套过深（>{depth}）");
        }
        if let Some(reference) = schema.get("$ref") {
            let reference = reference
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("$ref 必须是字符串（{path}）"))?;
            if reference.starts_with('#') {
                bail!("不支持文档内 $ref 片段（{path}）: {reference}");
            }
            let target = self.load(reference)?;
            return self.check(&target, value, path, depth + 1);
        }
        if let Some(all_of) = schema.get("allOf") {
            let all_of = all_of
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("allOf 必须是数组（{path}）"))?;
            for item in all_of {
                self.check(item, value, path, depth + 1)?;
            }
        }
        if let Some(expected) = schema.get("type") {
            check_type(expected, value, path)?;
        }
        if let Some(constant) = schema.get("const") {
            if value != constant {
                bail!("{path} 必须等于 {constant}，实为 {value}");
            }
        }
        if let Some(options) = schema.get("enum") {
            let options = options
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("enum 必须是数组（{path}）"))?;
            if !options.contains(value) {
                bail!("{path} 必须是 {options:?} 之一，实为 {value}");
            }
        }
        if let Some(text) = value.as_str() {
            let length = text.chars().count();
            if let Some(min) = schema.get("minLength").and_then(Value::as_u64) {
                if (length as u64) < min {
                    bail!("{path} 长度不得小于 {min}，实为 {length}");
                }
            }
            if let Some(max) = schema.get("maxLength").and_then(Value::as_u64) {
                if (length as u64) > max {
                    bail!("{path} 长度不得大于 {max}，实为 {length}");
                }
            }
        }
        if let Some(number) = value.as_f64() {
            if let Some(min) = schema.get("minimum").and_then(Value::as_f64) {
                if number < min {
                    bail!("{path} 不得小于 {min}，实为 {number}");
                }
            }
            if let Some(max) = schema.get("maximum").and_then(Value::as_f64) {
                if number > max {
                    bail!("{path} 不得大于 {max}，实为 {number}");
                }
            }
        }
        if let Some(object) = value.as_object() {
            if let Some(required) = schema.get("required") {
                let required = required
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("required 必须是数组（{path}）"))?;
                for key in required {
                    let key = key
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("required 元素必须是字符串（{path}）"))?;
                    if !object.contains_key(key) {
                        bail!("{path} 缺少必填字段 `{key}`");
                    }
                }
            }
            let properties = schema.get("properties").and_then(Value::as_object);
            if let Some(properties) = properties {
                for (key, child) in properties {
                    if let Some(child_value) = object.get(key) {
                        self.check(child, child_value, &format!("{path}.{key}"), depth + 1)?;
                    }
                }
            }
            if let Some(additional) = schema.get("additionalProperties") {
                for (key, child_value) in object {
                    if properties.is_some_and(|p| p.contains_key(key)) {
                        continue;
                    }
                    match additional {
                        Value::Bool(false) => bail!("{path} 不允许字段 `{key}`"),
                        Value::Object(_) => {
                            self.check(
                                additional,
                                child_value,
                                &format!("{path}.{key}"),
                                depth + 1,
                            )?;
                        }
                        _ => {}
                    }
                }
            }
        }
        if let Some(items) = schema.get("items") {
            if let Some(array) = value.as_array() {
                for (index, item) in array.iter().enumerate() {
                    self.check(items, item, &format!("{path}[{index}]"), depth + 1)?;
                }
            }
        }
        Ok(())
    }
}

fn check_type(expected: &Value, value: &Value, path: &str) -> Result<()> {
    let names: Vec<&str> = match expected {
        Value::String(name) => vec![name.as_str()],
        Value::Array(items) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .ok_or_else(|| anyhow::anyhow!("type 数组元素必须是字符串（{path}）"))
            })
            .collect::<Result<_>>()?,
        _ => bail!("type 必须是字符串或字符串数组（{path}）"),
    };
    for name in names {
        if matches_type(name, value, path)? {
            return Ok(());
        }
    }
    bail!("{path} 的类型不是 {expected}，实为 {value}");
}

fn matches_type(name: &str, value: &Value, path: &str) -> Result<bool> {
    Ok(match name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        other => bail!("schema 使用了不支持的 type `{other}`（{path}）"),
    })
}
