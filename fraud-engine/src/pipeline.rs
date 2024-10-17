use std::collections::HashMap;

use crate::{
    dsl::Rule,
    report::{CheckResult, Report},
};
use eyre::{eyre, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Default, Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Pipeline {
    pub checks: Vec<Check>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Check {
    pub identifier: String,
    pub enabled: bool,
    pub rule: Rule,
}

impl Pipeline {
    /// Runs the pipeline checks.
    /// Returns report with fraud detection results of each check.
    pub fn run<T: Serialize>(&self, data: &T) -> Result<Report> {
        let serialized_data = serde_json::to_value(data)?;
        let mut check_results = HashMap::with_capacity(self.checks.len());

        for check in &self.checks {
            let result = check.rule.evaluate(&serialized_data);
            tracing::info!("{}: {:?}", check.identifier, result);

            if check_results
                .insert(check.identifier.clone(), CheckResult {
                    enabled: check.enabled,
                    result: result.ok(),
                })
                .is_some()
            {
                return Err(eyre!(
                    "pipeline checks with the same identifier \"{}\" found.",
                    check.identifier
                ));
            }
        }

        Ok(Report { check_results })
    }
}

impl Check {
    pub fn new(identifier: &str, rule: Rule) -> Self {
        Self { identifier: identifier.to_string(), enabled: true, rule }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct FraudResults {
        group_a: GroupA,
        group_b: GroupB,
    }

    #[derive(Serialize)]
    struct GroupA {
        x: f64,
        y: bool,
    }

    #[derive(Serialize)]
    struct GroupB {
        x: bool,
        y: bool,
    }

    fn init_fraud_results() -> FraudResults {
        FraudResults { group_a: GroupA { x: 0.8, y: false }, group_b: GroupB { x: true, y: true } }
    }

    fn init_pipeline(id1: &str, id2: &str) -> Pipeline {
        let rule1 = Rule::new(&[("x", "group_a.x"), ("y", "group_a.y")], "x > 0.5 || y");
        let rule2 = Rule::new(&[("x", "group_b.x"), ("y", "group_b.y")], "x && !y");
        Pipeline { checks: vec![Check::new(id1, rule1), Check::new(id2, rule2)] }
    }

    #[test]
    fn test_pipeline_run() {
        let fraud_results = init_fraud_results();
        let pipeline = init_pipeline("id1", "id2");
        let report = pipeline.run(&fraud_results).unwrap();

        assert!(report.fraud_detected());
        assert!(matches!(report.check_results["id1"].result, Some(true)));
        assert!(matches!(report.check_results["id2"].result, Some(false)));
    }

    #[test]
    fn test_checks_with_same_identifier() {
        let fraud_results = init_fraud_results();
        let pipeline = init_pipeline("id", "id");
        let report = pipeline.run(&fraud_results);
        assert!(report.is_err());
    }
}
