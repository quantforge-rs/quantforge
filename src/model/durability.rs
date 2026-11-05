//! What a store found when it read its commit settings back
//! (`docs/storage-durability.md`).

use std::fmt;

/// One setting a commit relies on, as the store found it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurabilitySetting {
    /// The setting's own name, such as `synchronous` or `fsync`.
    pub name: &'static str,
    pub effective: String,
    pub required: &'static str,
    pub met: bool,
}

/// The settings a store's commits rely on, read back from a connection, and
/// what an operator does about any that fall short. It holds setting names
/// and values only, never a connection string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurabilityReport {
    settings: Vec<DurabilitySetting>,
    remedy: &'static str,
}

impl DurabilityReport {
    pub fn new(settings: Vec<DurabilitySetting>, remedy: &'static str) -> Self {
        Self { settings, remedy }
    }

    pub fn settings(&self) -> &[DurabilitySetting] {
        &self.settings
    }

    /// Whether every setting meets the contract.
    pub fn is_met(&self) -> bool {
        self.settings.iter().all(|setting| setting.met)
    }

    /// What falls short and the remedy, or `None` when nothing does.
    pub fn problem(&self) -> Option<String> {
        let unmet: Vec<String> = self
            .settings
            .iter()
            .filter(|setting| !setting.met)
            .map(|setting| {
                format!(
                    "{} is {}, and needs {}",
                    setting.name, setting.effective, setting.required
                )
            })
            .collect();
        (!unmet.is_empty()).then(|| format!("{}; {}", unmet.join(", "), self.remedy))
    }
}

/// Every setting as `name=value`, as `store status` prints them.
impl fmt::Display for DurabilityReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let shown: Vec<String> = self
            .settings
            .iter()
            .map(|setting| format!("{}={}", setting.name, setting.effective))
            .collect();
        f.write_str(&shown.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setting(name: &'static str, effective: &str, met: bool) -> DurabilitySetting {
        DurabilitySetting {
            name,
            effective: effective.to_string(),
            required: "on",
            met,
        }
    }

    #[test]
    fn a_report_with_every_setting_met_has_no_problem() {
        let report = DurabilityReport::new(vec![setting("fsync", "on", true)], "fix it");
        assert!(report.is_met());
        assert_eq!(report.problem(), None);
        assert_eq!(report.to_string(), "fsync=on");
    }

    #[test]
    fn a_problem_names_each_setting_that_falls_short_and_the_remedy() {
        let report = DurabilityReport::new(
            vec![
                setting("fsync", "off", false),
                setting("full_page_writes", "on", true),
                setting("synchronous_commit", "off", false),
            ],
            "fix it",
        );
        assert!(!report.is_met());
        assert_eq!(
            report.problem().as_deref(),
            Some("fsync is off, and needs on, synchronous_commit is off, and needs on; fix it")
        );
    }
}
