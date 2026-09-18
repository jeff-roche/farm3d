use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::{Config, Dummy, TypeVisitor, TS};

use super::{ContractVersion, JS_MAX_SAFE_INTEGER};

/// A non-negative integer that TypeScript can represent exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, TS)]
#[ts(type = "number", export_to = "event/JsSafeInteger.ts")]
pub struct JsSafeInteger(u64);

impl JsSafeInteger {
    pub fn get(self) -> u64 {
        self.0
    }
}

impl TryFrom<u64> for JsSafeInteger {
    type Error = JsSafeIntegerError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value > JS_MAX_SAFE_INTEGER {
            return Err(JsSafeIntegerError);
        }
        Ok(Self(value))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JsSafeIntegerError;

impl std::fmt::Display for JsSafeIntegerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("integer must be non-negative and within JavaScript's safe range")
    }
}

impl std::error::Error for JsSafeIntegerError {}

impl Serialize for JsSafeInteger {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u64(self.0)
    }
}

impl<'de> Deserialize<'de> for JsSafeInteger {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        JsSafeInteger::try_from(u64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// Stable identity for the object an event concerns.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "event/EventSubject.ts")]
pub struct EventSubject {
    pub kind: String,
    pub id: String,
}

/// Versioned metadata shared by every frontend event.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventEnvelope<T, P> {
    pub contract_version: ContractVersion,
    pub stream_id: String,
    pub sequence: JsSafeInteger,
    pub event_id: String,
    pub occurred_at: String,
    #[serde(rename = "type")]
    pub event_type: T,
    pub subject: EventSubject,
    pub payload: P,
}

impl<T: TS, P: TS> TS for EventEnvelope<T, P> {
    type WithoutGenerics = EventEnvelope<Dummy, Dummy>;
    type OptionInnerType = Self;

    fn docs() -> Option<String> {
        Some("/**\n * Versioned metadata shared by every frontend event.\n */\n".to_string())
    }

    fn ident(_: &Config) -> String {
        "EventEnvelope".to_string()
    }

    fn name(config: &Config) -> String {
        format!("EventEnvelope<{}, {}>", T::name(config), P::name(config))
    }

    fn inline(config: &Config) -> String {
        format!(
            "{{ contractVersion: 1, streamId: string, sequence: number, eventId: string, occurredAt: string, type: {}, subject: {}, payload: {}, }}",
            T::name(config),
            EventSubject::name(config),
            P::name(config)
        )
    }

    fn decl(config: &Config) -> String {
        format!(
            "type EventEnvelope<T extends string, P> = {{ contractVersion: 1, streamId: string, sequence: number, eventId: string, occurredAt: string, type: T, subject: {}, payload: P, }};",
            EventSubject::name(config)
        )
    }

    fn decl_concrete(config: &Config) -> String {
        format!("type {} = {};", Self::ident(config), Self::inline(config))
    }

    fn visit_dependencies(visitor: &mut impl TypeVisitor)
    where
        Self: 'static,
    {
        visitor.visit::<EventSubject>();
    }

    fn visit_generics(visitor: &mut impl TypeVisitor)
    where
        Self: 'static,
    {
        T::visit_generics(visitor);
        visitor.visit::<T>();
        P::visit_generics(visitor);
        visitor.visit::<P>();
    }

    fn output_path() -> Option<PathBuf> {
        Some("event/EventEnvelope.ts".into())
    }
}
