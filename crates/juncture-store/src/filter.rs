//! Filter expressions for querying stored items.

use serde_json::Value;

/// Filter expression for querying items.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum FilterExpr {
    /// Equality check.
    Eq {
        /// Field path (e.g., "metadata.status").
        field: String,
        /// Value to compare against.
        value: Value,
    },
    /// Inequality check.
    Ne {
        /// Field path.
        field: String,
        /// Value to compare against.
        value: Value,
    },
    /// Greater than check (numeric or string).
    Gt {
        /// Field path.
        field: String,
        /// Value to compare against.
        value: Value,
    },
    /// Greater than or equal check.
    Gte {
        /// Field path.
        field: String,
        /// Value to compare against.
        value: Value,
    },
    /// Less than check.
    Lt {
        /// Field path.
        field: String,
        /// Value to compare against.
        value: Value,
    },
    /// Less than or equal check.
    Lte {
        /// Field path.
        field: String,
        /// Value to compare against.
        value: Value,
    },
    /// Logical AND - all expressions must match.
    And(Vec<FilterExpr>),
    /// Logical OR - at least one expression must match.
    Or(Vec<FilterExpr>),
    /// Logical NOT - expression must not match.
    Not(Box<FilterExpr>),
}

impl FilterExpr {
    /// Evaluates the filter expression against a JSON value.
    #[must_use]
    pub fn matches(&self, value: &Value) -> bool {
        matches_filter(value, self)
    }
}

/// Evaluates a filter expression against a JSON value.
#[must_use]
pub fn matches_filter(value: &Value, filter: &FilterExpr) -> bool {
    match filter {
        FilterExpr::Eq {
            field,
            value: target,
        } => compare_values(get_field(value, field), target) == std::cmp::Ordering::Equal,
        FilterExpr::Ne {
            field,
            value: target,
        } => compare_values(get_field(value, field), target) != std::cmp::Ordering::Equal,
        FilterExpr::Gt {
            field,
            value: target,
        } => compare_values(get_field(value, field), target) == std::cmp::Ordering::Greater,
        FilterExpr::Gte {
            field,
            value: target,
        } => compare_values(get_field(value, field), target) != std::cmp::Ordering::Less,
        FilterExpr::Lt {
            field,
            value: target,
        } => compare_values(get_field(value, field), target) == std::cmp::Ordering::Less,
        FilterExpr::Lte {
            field,
            value: target,
        } => compare_values(get_field(value, field), target) != std::cmp::Ordering::Greater,
        FilterExpr::And(exprs) => exprs.iter().all(|expr| matches_filter(value, expr)),
        FilterExpr::Or(exprs) => exprs.iter().any(|expr| matches_filter(value, expr)),
        FilterExpr::Not(expr) => !matches_filter(value, expr),
    }
}

/// Gets a nested field from a JSON value by path (e.g., "metadata.status").
#[must_use]
fn get_field<'a>(value: &'a Value, path: &str) -> &'a Value {
    let mut current = value;
    for part in path.split('.') {
        current = match current {
            Value::Object(map) => map.get(part).unwrap_or(&Value::Null),
            Value::Array(arr) => part
                .parse::<usize>()
                .map_or(&Value::Null, |index| arr.get(index).unwrap_or(&Value::Null)),
            _ => &Value::Null,
        };
    }
    current
}

/// Compares two JSON values for ordering.
fn compare_values(left: &Value, right: &Value) -> std::cmp::Ordering {
    match (left, right) {
        (Value::Null, Value::Null) | (Value::Object(_), Value::Object(_)) => {
            std::cmp::Ordering::Equal
        }
        (Value::Null, _) => std::cmp::Ordering::Less,
        (_, Value::Null) => std::cmp::Ordering::Greater,
        (Value::Bool(l), Value::Bool(r)) => l.cmp(r),
        (Value::Bool(_), _) => std::cmp::Ordering::Less,
        (_, Value::Bool(_)) => std::cmp::Ordering::Greater,
        (Value::Number(l), Value::Number(r)) => {
            if let (Some(li), Some(ri)) = (l.as_i64(), r.as_i64()) {
                li.cmp(&ri)
            } else if let (Some(lf), Some(rf)) = (l.as_f64(), r.as_f64()) {
                lf.partial_cmp(&rf).unwrap_or(std::cmp::Ordering::Equal)
            } else {
                std::cmp::Ordering::Equal
            }
        }
        (Value::Number(_), _) => std::cmp::Ordering::Less,
        (_, Value::Number(_)) => std::cmp::Ordering::Greater,
        (Value::String(l), Value::String(r)) => l.cmp(r),
        (Value::String(_), _) => std::cmp::Ordering::Less,
        (_, Value::String(_)) => std::cmp::Ordering::Greater,
        (Value::Array(l), Value::Array(r)) => match l.len().cmp(&r.len()) {
            std::cmp::Ordering::Equal => std::cmp::Ordering::Equal,
            other => other,
        },
        (Value::Array(_), _) => std::cmp::Ordering::Less,
        (_, Value::Array(_)) => std::cmp::Ordering::Greater,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_filter_eq() {
        let value = json!({"name": "test", "count": 42});
        let filter = FilterExpr::Eq {
            field: "name".to_string(),
            value: json!("test"),
        };
        assert!(matches_filter(&value, &filter));
    }

    #[test]
    fn test_filter_gt() {
        let value = json!({"count": 42});
        let filter = FilterExpr::Gt {
            field: "count".to_string(),
            value: json!(40),
        };
        assert!(matches_filter(&value, &filter));
    }

    #[test]
    fn test_filter_and() {
        let value = json!({"name": "test", "count": 42, "active": true});
        let filter = FilterExpr::And(vec![
            FilterExpr::Eq {
                field: "name".to_string(),
                value: json!("test"),
            },
            FilterExpr::Gt {
                field: "count".to_string(),
                value: json!(40),
            },
        ]);
        assert!(matches_filter(&value, &filter));
    }

    #[test]
    fn test_filter_nested() {
        let value = json!({"metadata": {"status": "active"}});
        let filter = FilterExpr::Eq {
            field: "metadata.status".to_string(),
            value: json!("active"),
        };
        assert!(matches_filter(&value, &filter));
    }

    #[test]
    fn test_filter_not() {
        let value = json!({"status": "inactive"});
        let filter = FilterExpr::Not(Box::new(FilterExpr::Eq {
            field: "status".to_string(),
            value: json!("active"),
        }));
        assert!(matches_filter(&value, &filter));
    }
}

// Rust guideline compliant 2026-05-19
