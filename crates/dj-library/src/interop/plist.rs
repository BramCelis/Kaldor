//! Apple XML property lists (`<plist>`: `dict` of alternating `key` and
//! value elements, `array`, `string`, `integer`, `real`, `date`, `true`,
//! `false`, `data`), as the iTunes / Music "Library.xml" export uses.

use super::xml::El;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    Dict(Vec<(String, Value)>),
    Array(Vec<Value>),
    Str(String),
    Int(i64),
    Real(f64),
    Bool(bool),
    /// ISO 8601 text, e.g. "2024-05-01T10:20:30Z".
    Date(String),
    /// Base64 data, not decoded (not needed here).
    Data,
}

impl Value {
    pub fn from_el(el: &El) -> Option<Self> {
        Some(match el.name.as_str() {
            "dict" => {
                let mut out = Vec::new();
                let mut key: Option<String> = None;
                for c in &el.children {
                    if c.name == "key" {
                        key = Some(c.text.clone());
                    } else if let Some(k) = key.take()
                        && let Some(v) = Value::from_el(c)
                    {
                        out.push((k, v));
                    }
                }
                Value::Dict(out)
            }
            "array" => Value::Array(el.children.iter().filter_map(Value::from_el).collect()),
            "string" => Value::Str(el.text.clone()),
            "integer" => Value::Int(el.text.trim().parse().ok()?),
            "real" => Value::Real(el.text.trim().parse().ok()?),
            "date" => Value::Date(el.text.trim().to_string()),
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            "data" => Value::Data,
            _ => return None,
        })
    }

    pub fn get(&self, k: &str) -> Option<&Value> {
        match self {
            Value::Dict(d) => d.iter().find(|(n, _)| n == k).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn str(&self, k: &str) -> Option<&str> {
        match self.get(k)? {
            Value::Str(s) | Value::Date(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn int(&self, k: &str) -> Option<i64> {
        match self.get(k)? {
            Value::Int(i) => Some(*i),
            Value::Real(r) => Some(r.round() as i64),
            _ => None,
        }
    }

    pub fn flag(&self, k: &str) -> bool {
        matches!(self.get(k), Some(Value::Bool(true)))
    }

    pub fn entries(&self) -> &[(String, Value)] {
        match self {
            Value::Dict(d) => d,
            _ => &[],
        }
    }

    pub fn items(&self) -> &[Value] {
        match self {
            Value::Array(a) => a,
            _ => &[],
        }
    }
}

/// The top value of a `<plist>` document.
pub(crate) fn root(doc: &El) -> Option<Value> {
    if doc.name != "plist" {
        return None;
    }
    doc.children.first().and_then(Value::from_el)
}
