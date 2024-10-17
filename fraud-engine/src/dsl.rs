use evalexpr::{eval_boolean_with_context, ContextWithMutableVariables, HashMapContext};
use eyre::{eyre, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Rule {
    /// Mapping of operand `names` and `paths`.
    /// Paths should be dot-separated strings that define
    /// the path of the operand into the json serialized data.
    pub operand_mapping: HashMap<String, String>,

    /// Arithmetic expression to be evaluated on the serialized data.
    pub expression: String,
}

impl Rule {
    pub fn new(mapping: &[(&str, &str)], expression: &str) -> Rule {
        let operand_mapping: HashMap<String, String> =
            mapping.iter().map(|mp| (mp.0.to_owned(), mp.1.to_owned())).collect();
        let expression = expression.to_owned();
        Rule { operand_mapping, expression }
    }

    pub fn evaluate(&self, serialized_data: &serde_json::Value) -> Result<bool> {
        let context = self.build_evalexpr_context(serialized_data)?;
        tracing::debug!("evaluating expression: {} with context: {:?}", self.expression, context);
        Ok(eval_boolean_with_context(&self.expression, &context)?)
    }

    fn build_evalexpr_context(
        &self,
        serialized_data: &serde_json::Value,
    ) -> Result<HashMapContext> {
        let mut context = HashMapContext::new();
        for (name, path) in &self.operand_mapping {
            let serde_value = extract_value_from_serialized_data(path, serialized_data)
                .ok_or_else(|| eyre!("failed to extract operand ({}, {})", name, path,))?;
            let evalexpr_value = serde_to_evalexpr_value(serde_value)?;
            context.set_value(name.to_owned(), evalexpr_value)?;
        }

        Ok(context)
    }
}

fn extract_value_from_serialized_data<'a>(
    path: &str,
    serialized_data: &'a serde_json::Value,
) -> Option<&'a serde_json::Value> {
    let mut current_value = serialized_data;
    for key in path.split('.') {
        current_value = current_value.get(key)?;
    }
    Some(current_value)
}

fn serde_to_evalexpr_value(serde_value: &serde_json::Value) -> Result<evalexpr::Value> {
    match serde_value {
        serde_json::Value::Null => Ok(evalexpr::Value::Empty),
        serde_json::Value::Bool(bit) => Ok(evalexpr::Value::Boolean(*bit)),
        serde_json::Value::Number(num) => {
            if num.is_f64() {
                Ok(evalexpr::Value::Float(
                    num.as_f64().ok_or_else(|| eyre!("failed to extract f64"))?,
                ))
            } else if num.is_u64() {
                Ok(evalexpr::Value::Int(
                    num.as_u64().ok_or_else(|| eyre!("failed to extract u64"))?.try_into()?,
                ))
            } else {
                Ok(evalexpr::Value::Int(
                    num.as_i64().ok_or_else(|| eyre!("failed to extract i64"))?,
                ))
            }
        }
        serde_json::Value::String(string) => Ok(evalexpr::Value::String(string.clone())),
        serde_json::Value::Array(arr) => {
            let mut tuple = evalexpr::TupleType::with_capacity(arr.capacity());
            for serde_value in arr {
                tuple.push(serde_to_evalexpr_value(serde_value)?);
            }
            Ok(evalexpr::Value::Tuple(tuple))
        }
        serde_json::Value::Object(_) => Err(eyre!(
            "conversion failed: {} is not a fundamental data-type",
            serde_value.to_string()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[derive(Serialize)]
    struct Example {
        field_a: Nested,
        field_b: u32,
    }

    #[derive(Serialize)]
    struct Nested {
        nested_x: bool,
        nested_y: u64,
        nested_z: Option<i32>,
        nested_m: String,
        nested_n: Vec<i32>,
        nested_o: f32,
    }

    fn example_init() -> Example {
        Example {
            field_a: Nested {
                nested_x: true,
                nested_y: 18,
                nested_z: None,
                nested_m: "text".to_owned(),
                nested_n: vec![1, 2, 3],
                nested_o: 4.321,
            },
            field_b: 1000,
        }
    }

    #[test]
    fn test_extract_value_from_serialized_data() {
        let example = example_init();
        let data_serialized = serde_json::to_value(example).unwrap();

        assert!(matches!(
            extract_value_from_serialized_data("field_b", &data_serialized),
            Some(&serde_json::Value::Number(_))
        ));

        assert!(matches!(
            extract_value_from_serialized_data("field_a.nested_x", &data_serialized),
            Some(&serde_json::Value::Bool(_))
        ));

        assert!(matches!(
            extract_value_from_serialized_data("field_a.nested_y", &data_serialized),
            Some(&serde_json::Value::Number(_))
        ));

        assert!(matches!(
            extract_value_from_serialized_data("field_a.nested_z", &data_serialized),
            Some(&serde_json::Value::Null)
        ));

        assert!(matches!(
            extract_value_from_serialized_data("field_a.nested_m", &data_serialized),
            Some(&serde_json::Value::String(_))
        ));

        assert!(matches!(
            extract_value_from_serialized_data("field_a.nested_n", &data_serialized),
            Some(&serde_json::Value::Array(_))
        ));

        assert!(extract_value_from_serialized_data("field_c.nested_x", &data_serialized).is_none(),);
    }

    #[test]
    fn test_rule_evaluation() {
        let data_serialized = serde_json::to_value(example_init()).unwrap();

        // correctly formatted return true
        let rule = Rule::new(
            &[("b", "field_b"), ("x", "field_a.nested_x"), ("y", "field_a.nested_y")],
            "b > y && x",
        );
        assert!(matches!(rule.evaluate(&data_serialized), Ok(true)));

        // correctly formatted returning false
        let rule =
            Rule::new(&[("x", "field_b"), ("fp_operand", "field_a.nested_o")], "fp_operand > x");
        assert!(matches!(rule.evaluate(&data_serialized), Ok(false)));

        // undefined operand
        let rule = Rule::new(&[], "x > 5");
        assert!(rule.evaluate(&data_serialized).is_err());

        // using non-existent operand path
        let rule = Rule::new(&[("x", "non_existent.non_existent")], "x > 5");
        assert!(rule.evaluate(&data_serialized).is_err());
    }
}
