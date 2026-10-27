//! Ordered configuration values shared by every source syntax and output format.

use anyhow::{Context, Result, bail};
use serde_json::{Map, Number};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    /// A TOML date or time, kept in its RFC 3339 spelling.
    Datetime(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

/// A key path from an object's root to one of its leaves.
pub(crate) type Path = Vec<String>;

impl Value {
    pub(crate) fn object() -> Self {
        Self::Object(Vec::new())
    }

    pub(crate) fn as_object(&self) -> Option<&[(String, Value)]> {
        match self {
            Self::Object(fields) => Some(fields),
            _ => None,
        }
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Value> {
        self.as_object()?
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub(crate) fn is_empty_object(&self) -> bool {
        matches!(self, Self::Object(fields) if fields.is_empty())
    }

    /// Scalars, arrays and empty objects, in document order.
    pub(crate) fn leaves(&self) -> Vec<(Path, &Value)> {
        fn walk<'a>(value: &'a Value, path: &mut Path, leaves: &mut Vec<(Path, &'a Value)>) {
            match value {
                Value::Object(fields) if !fields.is_empty() => {
                    for (key, value) in fields {
                        path.push(key.clone());
                        walk(value, path, leaves);
                        path.pop();
                    }
                }
                leaf => leaves.push((path.clone(), leaf)),
            }
        }
        let mut leaves = Vec::new();
        if !self.is_empty_object() {
            walk(self, &mut Vec::new(), &mut leaves);
        }
        leaves
    }

    /// Sets a leaf, creating intermediate objects and keeping existing key positions.
    pub(crate) fn set_path(&mut self, path: &[String], value: Value) -> Result<()> {
        let Some((first, rest)) = path.split_first() else {
            *self = value;
            return Ok(());
        };
        let Self::Object(fields) = self else {
            bail!("cannot set {first:?} inside a value that is not a table");
        };
        let index = match fields.iter().position(|(key, _)| key == first) {
            Some(index) => index,
            None => {
                fields.push((first.clone(), Value::object()));
                fields.len() - 1
            }
        };
        let slot = &mut fields[index].1;
        if rest.is_empty() {
            *slot = value;
            return Ok(());
        }
        if !matches!(slot, Self::Object(_)) {
            bail!("{first:?} is not a table");
        }
        slot.set_path(rest, value)
            .with_context(|| format!("inside {first:?}"))
    }

    /// Merges `other` into `self`: tables merge key by key; anything else is replaced.
    pub(crate) fn merge(&mut self, other: &Value) {
        match (self, other) {
            (Self::Object(fields), Self::Object(incoming)) => {
                for (key, value) in incoming {
                    match fields.iter_mut().find(|(name, _)| name == key) {
                        Some((_, existing)) => existing.merge(value),
                        None => fields.push((key.clone(), value.clone())),
                    }
                }
            }
            (slot, value) => *slot = value.clone(),
        }
    }

    pub(crate) fn from_pkl(value: &pklr::Value, path: &str) -> Result<Self> {
        Ok(match value {
            pklr::Value::Null => Self::Null,
            pklr::Value::Bool(value) => Self::Bool(*value),
            pklr::Value::Int(value) => Self::Int(*value),
            pklr::Value::Float(value) if value.is_finite() => Self::Float(*value),
            pklr::Value::Float(_) => bail!("{path}: non-finite numbers cannot be rendered"),
            pklr::Value::String(value) => Self::String(value.to_string()),
            pklr::Value::Lambda(..) => bail!(
                "{path}: a Pkl function is not an output value; call it to compute a data value"
            ),
            pklr::Value::List(values) => Self::Array(
                values
                    .iter()
                    .enumerate()
                    .map(|(index, value)| Self::from_pkl(value, &format!("{path}[{index}]")))
                    .collect::<Result<_>>()?,
            ),
            pklr::Value::Object(values, _) => Self::Object(
                values
                    .iter()
                    .map(|(key, value)| {
                        Self::from_pkl(value, &format!("{path}.{key}"))
                            .map(|value| (key.to_string(), value))
                    })
                    .collect::<Result<_>>()?,
            ),
            _ => bail!("{path}: this kind of Pkl value cannot be rendered"),
        })
    }

    pub(crate) fn from_json(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(value) => Self::Bool(*value),
            serde_json::Value::Number(number) => match number.as_i64() {
                Some(value) => Self::Int(value),
                None => Self::Float(number.as_f64().unwrap_or_default()),
            },
            serde_json::Value::String(value) => Self::String(value.clone()),
            serde_json::Value::Array(values) => {
                Self::Array(values.iter().map(Self::from_json).collect())
            }
            serde_json::Value::Object(fields) => Self::Object(
                fields
                    .iter()
                    .map(|(key, value)| (key.clone(), Self::from_json(value)))
                    .collect(),
            ),
        }
    }

    pub(crate) fn to_json(&self) -> Result<serde_json::Value> {
        Ok(match self {
            Self::Null => serde_json::Value::Null,
            Self::Bool(value) => (*value).into(),
            Self::Int(value) => (*value).into(),
            Self::Float(value) => serde_json::Value::Number(
                Number::from_f64(*value).context("non-finite numbers cannot be rendered")?,
            ),
            Self::String(value) | Self::Datetime(value) => value.clone().into(),
            Self::Array(values) => {
                serde_json::Value::Array(values.iter().map(Self::to_json).collect::<Result<_>>()?)
            }
            Self::Object(fields) => {
                let mut result = Map::new();
                for (key, value) in fields {
                    result.insert(key.clone(), value.to_json()?);
                }
                serde_json::Value::Object(result)
            }
        })
    }

    pub(crate) fn to_toml(&self) -> Result<toml::Value> {
        Ok(match self {
            Self::Null => bail!("TOML has no null; remove the value instead"),
            Self::Bool(value) => toml::Value::Boolean(*value),
            Self::Int(value) => toml::Value::Integer(*value),
            Self::Float(value) => toml::Value::Float(*value),
            Self::String(value) => toml::Value::String(value.clone()),
            Self::Datetime(value) => toml::Value::Datetime(
                value
                    .parse()
                    .with_context(|| format!("invalid TOML date-time {value:?}"))?,
            ),
            Self::Array(values) => {
                toml::Value::Array(values.iter().map(Self::to_toml).collect::<Result<_>>()?)
            }
            Self::Object(fields) => {
                let mut table = toml::Table::new();
                for (key, value) in fields {
                    table.insert(key.clone(), value.to_toml()?);
                }
                toml::Value::Table(table)
            }
        })
    }

    /// A compact single-line spelling for diagnostics.
    pub(crate) fn describe(&self) -> String {
        match self.to_json() {
            Ok(json) => json.to_string(),
            Err(_) => format!("{self:?}"),
        }
    }
}

pub(crate) fn display_path(path: &[String]) -> String {
    path.iter()
        .map(|key| {
            if !key.is_empty()
                && key
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
            {
                key.clone()
            } else {
                format!("{key:?}")
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(keys: &[&str]) -> Path {
        keys.iter().map(|key| (*key).to_owned()).collect()
    }

    #[test]
    fn set_path_keeps_positions_and_creates_tables() {
        let mut value = Value::object();
        value.set_path(&path(&["b"]), Value::Int(1)).unwrap();
        value.set_path(&path(&["a", "x"]), Value::Int(2)).unwrap();
        value.set_path(&path(&["b"]), Value::Int(3)).unwrap();
        assert_eq!(
            value.to_json().unwrap().to_string(),
            r#"{"b":3,"a":{"x":2}}"#
        );
        assert!(value.set_path(&path(&["b", "c"]), Value::Int(4)).is_err());
    }

    #[test]
    fn merge_combines_tables_and_replaces_arrays() {
        let mut base = Value::from_json(&serde_json::json!({"a": {"x": 1, "y": [1]}, "b": 1}));
        base.merge(&Value::from_json(
            &serde_json::json!({"a": {"y": [2], "z": 3}}),
        ));
        assert_eq!(
            base.to_json().unwrap().to_string(),
            r#"{"a":{"x":1,"y":[2],"z":3},"b":1}"#
        );
    }

    #[test]
    fn leaves_include_arrays_and_empty_tables() {
        let value = Value::from_json(&serde_json::json!({"a": {"b": [1], "c": {}}, "d": 1}));
        let leaves = value
            .leaves()
            .into_iter()
            .map(|(path, _)| display_path(&path))
            .collect::<Vec<_>>();
        assert_eq!(leaves, ["a.b", "a.c", "d"]);
    }
}
