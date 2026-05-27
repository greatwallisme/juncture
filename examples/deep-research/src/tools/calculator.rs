//! Calculator tool for evaluating arithmetic expressions.

#![allow(
    dead_code,
    reason = "Public API components may not all be used in current binary"
)]

use async_trait::async_trait;
use juncture::tools::{Tool, ToolError};
use serde_json::json;

/// Calculator tool for evaluating basic arithmetic expressions.
#[derive(Debug, Default)]
pub struct Calculator;

impl Calculator {
    /// Create a new calculator tool.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for Calculator {
    fn name(&self) -> &'static str {
        "calculator"
    }

    fn description(&self) -> &'static str {
        "Evaluates arithmetic expressions. Supports addition (+), subtraction (-), \
         multiplication (*), and division (/). \
         Input: {\"expression\": \"2 + 3 * 4\"}"
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "expression": {
                    "type": "string",
                    "description": "Arithmetic expression to evaluate (e.g., '2 + 3 * 4')"
                }
            },
            "required": ["expression"]
        })
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<String, ToolError> {
        let expr = input["expression"].as_str().ok_or_else(|| {
            ToolError::invalid_input("Missing 'expression' parameter".to_string())
        })?;

        // Tokenize expression
        let tokens: Vec<&str> = expr.split_whitespace().collect();
        if tokens.len() < 3 {
            return Err(ToolError::invalid_input(
                "Expression must have at least: number operator number".to_string(),
            ));
        }

        // Parse first number
        let mut result = tokens[0]
            .parse::<f64>()
            .map_err(|e| ToolError::invalid_input(format!("Invalid number: {e}")))?;

        // Process operator-number pairs
        let mut i = 1;
        while i + 1 < tokens.len() {
            let op = tokens[i];
            let next = tokens[i + 1]
                .parse::<f64>()
                .map_err(|e| ToolError::invalid_input(format!("Invalid number: {e}")))?;

            match op {
                "+" => result += next,
                "-" => result -= next,
                "*" => result *= next,
                "/" => {
                    if next == 0.0 {
                        return Err(ToolError::execution_failed("Division by zero".to_string()));
                    }
                    result /= next;
                }
                _ => {
                    return Err(ToolError::invalid_input(format!(
                        "Unknown operator: {op}. Use +, -, *, /"
                    )));
                }
            }
            i += 2;
        }

        // Format result cleanly (remove trailing .0 for whole numbers)
        #[expect(
            clippy::cast_possible_truncation,
            reason = "Already checked that result.fract() == 0.0, safe to cast to i64"
        )]
        let output = if result.fract() == 0.0 {
            format!("{}", result as i64)
        } else {
            format!("{result:.2}") // Round to 2 decimal places
        };

        Ok(output)
    }
}

// Rust guideline compliant 2026-05-27
