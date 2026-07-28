use serde::{Deserialize, Serialize};

pub const APP_MATCH_FIELD_NAME: &str = "nodewarden:app-match";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerMode {
    Prompt,
    Hotkey,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppMatch {
    pub process: String,
    pub trigger: TriggerMode,
}

impl AppMatch {
    pub fn to_field_value(&self) -> String {
        serde_json::to_string(self).expect("AppMatch always serializes")
    }

    pub fn from_field_value(value: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_name_matches_spec() {
        assert_eq!(APP_MATCH_FIELD_NAME, "nodewarden:app-match");
    }

    #[test]
    fn round_trips_through_json() {
        let original = AppMatch {
            process: "RockstarGamesLauncher.exe".to_string(),
            trigger: TriggerMode::Prompt,
        };
        let json = original.to_field_value();
        let parsed = AppMatch::from_field_value(&json).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn serializes_trigger_as_lowercase() {
        let m = AppMatch { process: "mabl.exe".to_string(), trigger: TriggerMode::Auto };
        assert_eq!(m.to_field_value(), r#"{"process":"mabl.exe","trigger":"auto"}"#);
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(AppMatch::from_field_value("not json").is_err());
    }
}
