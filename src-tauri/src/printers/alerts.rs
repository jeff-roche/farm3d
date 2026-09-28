//! P8: a Printer's alert defaults (spec "Backend model" module layout,
//! `printers/alerts.rs`). This file holds the value types the Attention
//! observer reads (`AlertDefaults`, `OfflineAlertMinutes`,
//! `NotificationMode`); the `printer_alert_defaults` repository and the
//! `get`/`set` commands land beside them in a later task.

use chrono::Duration;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use ts_rs::TS;

/// The offline grace a Printer's `printer.offline` Condition waits for:
/// 1, 5, or 15 minutes (`printer_alert_defaults.offline_after_minutes`'s
/// CHECK). "Off" is `None` on [`AlertDefaults::offline_after_minutes`].
/// Serialized as the bare number.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OfflineAlertMinutes {
    One,
    Five,
    Fifteen,
}

impl OfflineAlertMinutes {
    pub const ALL: [OfflineAlertMinutes; 3] = [
        OfflineAlertMinutes::One,
        OfflineAlertMinutes::Five,
        OfflineAlertMinutes::Fifteen,
    ];

    pub fn minutes(self) -> i64 {
        match self {
            OfflineAlertMinutes::One => 1,
            OfflineAlertMinutes::Five => 5,
            OfflineAlertMinutes::Fifteen => 15,
        }
    }

    pub fn from_minutes(minutes: i64) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.minutes() == minutes)
    }

    pub fn grace(self) -> Duration {
        Duration::minutes(self.minutes())
    }
}

impl Serialize for OfflineAlertMinutes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_i64(self.minutes())
    }
}

impl<'de> Deserialize<'de> for OfflineAlertMinutes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let minutes = i64::deserialize(deserializer)?;
        OfflineAlertMinutes::from_minutes(minutes).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "offlineAfterMinutes must be 1, 5, or 15 (got {minutes})"
            ))
        })
    }
}

/// Whether a Printer's Events notify (`follow` the class settings) or
/// never do (`muted`).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/NotificationMode.ts")]
pub enum NotificationMode {
    #[default]
    Follow,
    Muted,
}

impl NotificationMode {
    pub const ALL: [NotificationMode; 2] = [NotificationMode::Follow, NotificationMode::Muted];
}

/// A Printer's offline grace, notification muting, and capture toggles.
/// No `printer_alert_defaults` row means [`AlertDefaults::default`]
/// (`5`, `follow`, true, true).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/AlertDefaults.ts")]
pub struct AlertDefaults {
    /// `null` = offline alerts off.
    #[ts(type = "1 | 5 | 15 | null")]
    pub offline_after_minutes: Option<OfflineAlertMinutes>,
    pub notifications: NotificationMode,
    pub snapshot_on_incident: bool,
    pub snapshot_on_completion: bool,
}

impl Default for AlertDefaults {
    fn default() -> Self {
        Self {
            offline_after_minutes: Some(OfflineAlertMinutes::Five),
            notifications: NotificationMode::Follow,
            snapshot_on_incident: true,
            snapshot_on_completion: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_five_minutes_follow_and_both_captures() {
        assert_eq!(
            serde_json::to_value(AlertDefaults::default()).unwrap(),
            serde_json::json!({
                "offlineAfterMinutes": 5,
                "notifications": "follow",
                "snapshotOnIncident": true,
                "snapshotOnCompletion": true,
            })
        );
    }

    #[test]
    fn offline_minutes_round_trip_as_numbers_and_reject_other_values() {
        for minutes in OfflineAlertMinutes::ALL {
            let json = serde_json::to_string(&minutes).unwrap();
            assert_eq!(json, minutes.minutes().to_string());
            assert_eq!(
                serde_json::from_str::<OfflineAlertMinutes>(&json).unwrap(),
                minutes
            );
        }
        for bad in ["0", "2", "10", "60", "-5", "\"5\""] {
            assert!(
                serde_json::from_str::<OfflineAlertMinutes>(bad).is_err(),
                "{bad}"
            );
        }
        let off: AlertDefaults = serde_json::from_value(serde_json::json!({
            "offlineAfterMinutes": null,
            "notifications": "muted",
            "snapshotOnIncident": false,
            "snapshotOnCompletion": false,
        }))
        .unwrap();
        assert_eq!(off.offline_after_minutes, None);
        assert_eq!(off.notifications, NotificationMode::Muted);
    }

    #[test]
    fn grace_is_the_minutes_as_a_duration() {
        assert_eq!(OfflineAlertMinutes::Fifteen.grace(), Duration::minutes(15));
    }
}
