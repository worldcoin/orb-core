use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Report {
    pub check_results: HashMap<String, CheckResult>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CheckResult {
    pub enabled: bool,
    pub result: Option<bool>,
}

impl Report {
    pub fn fraud_detected(&self) -> bool {
        self.check_results
            .values()
            .map(|result| result.result.as_ref().unwrap_or(&true))
            .any(|&x| x)
    }

    pub fn to_datadog_tags(&self) -> impl Iterator<Item = String> + '_ {
        self.check_results.iter().map(|(tag, result)| {
            format!(
                "{}:{}",
                tag,
                result.result.as_ref().map_or("none".to_owned(), |b| b.to_string())
            )
        })
    }

    pub fn fraud_detected_on_enabled_checks(&self) -> bool {
        self.check_results
            .values()
            .filter(|result| result.enabled)
            .map(|result| result.result.as_ref().unwrap_or(&true))
            .any(|&x| x)
    }

    pub fn to_datadog_tags_only_enabled_checks(&self) -> impl Iterator<Item = String> + '_ {
        self.check_results.iter().filter(|(_, result)| result.enabled).map(|(tag, result)| {
            format!(
                "{}:{}",
                tag,
                result.result.as_ref().map_or("none".to_owned(), |b| b.to_string())
            )
        })
    }
}
