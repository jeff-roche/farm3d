//! Connection hints farm3d corrects on top of OrcaSlicer's profiles.
//!
//! OrcaSlicer's `host_type` names the upload target its own UI offers, not
//! the protocol the printer speaks: at v2.4.2 every Elegoo FDM model says
//! `elegoolink`, a label OrcaSlicer uses for several unrelated surfaces
//! (SDCP V3 over WebSocket, the Centauri Carbon 2's MQTT broker, Klipper
//! behind nginx). In farm3d, `elegoolink` means SDCP V3 only (#8, #26).
//!
//! `gen-catalog` applies [`HOST_OVERRIDES`] after ingestion, so a later
//! `just gen-catalog` keeps them. Each entry replaces `suggestedHostType`
//! (and sets `suggestedPort`) on every variant of one model, and records its
//! evidence and source. An entry that matches no model, or more than one,
//! fails the generator rather than silently going stale on a tag bump.

use crate::catalog::CatalogModel;

/// How a correction is backed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Evidence {
    /// A real-hardware capture.
    Observed,
    /// Public, third-party sources only; nobody has checked real hardware.
    Unverified,
}

#[derive(Clone, Copy, Debug)]
pub struct HostOverride {
    pub vendor: &'static str,
    /// Keyed together with `vendor`: `modelId` alone is not unique across
    /// the catalog (see `resolve_catalog_ref`).
    pub model_id: &'static str,
    pub host_type: Option<&'static str>,
    /// Pre-filled instead of the Connection kind's usual port.
    pub port: Option<u16>,
    pub evidence: Evidence,
    pub source: &'static str,
}

const fn elegoo(
    model_id: &'static str,
    host_type: Option<&'static str>,
    port: Option<u16>,
    evidence: Evidence,
    source: &'static str,
) -> HostOverride {
    HostOverride {
        vendor: "Elegoo",
        model_id,
        host_type,
        port,
        evidence,
        source,
    }
}

const SDCP_CAPTURE: &str = "A0.3 read-only capture of one Centauri Carbon: SDCP V3 over \
     WebSocket on port 3030 (#8, issuecomment-5838095991)";
const CC2_MQTT: &str = "Not SDCP: the printer runs its own MQTT broker with an access code in \
     LAN Only Mode (koen01/carbon_copy, OrcaSlicer ElegooLink.cpp, OrcaSlicer #13934). No \
     farm3d adapter yet";
const NEPTUNE4_NGINX: &str = "Klipper with Moonraker, reachable through nginx on port 80; 7125 \
     is closed on stock firmware (raspberry.tips Neptune 4 Home Assistant guide, SimplyPrint \
     setup guide, Elegoo wiki)";
const MARLIN_NO_NETWORK: &str = "Marlin (gcodeFlavor marlin); no network surface found in any \
     source checked for #26";

/// Every Elegoo FDM model at v2.4.2, including the two whose value is
/// unchanged, so an upstream change to any of them can't slip through.
pub const HOST_OVERRIDES: &[HostOverride] = &[
    elegoo(
        "Elegoo-CC",
        Some("elegoolink"),
        None,
        Evidence::Observed,
        SDCP_CAPTURE,
    ),
    elegoo(
        "Elegoo-C",
        Some("elegoolink"),
        None,
        Evidence::Unverified,
        "Assumed to match the Centauri Carbon (same product line); no source describes it \
         separately",
    ),
    elegoo("Elegoo-CC2", None, None, Evidence::Unverified, CC2_MQTT),
    elegoo(
        "Elegoo-C2",
        None,
        None,
        Evidence::Unverified,
        "Assumed to match the Centauri Carbon 2 (same product line)",
    ),
    elegoo(
        "Elegoo-OS-Giga",
        Some("moonraker"),
        Some(80),
        Evidence::Unverified,
        "Stock Klipper with Moonraker answering on port 80 (printer-hub.ru OrangeStorm Giga \
         mods guide; a single source, consistent with the Neptune 4's MKS board setup)",
    ),
    elegoo(
        "Elegoo-N4",
        Some("moonraker"),
        Some(80),
        Evidence::Unverified,
        NEPTUNE4_NGINX,
    ),
    elegoo(
        "Elegoo-N4Pro",
        Some("moonraker"),
        Some(80),
        Evidence::Unverified,
        NEPTUNE4_NGINX,
    ),
    elegoo(
        "Elegoo-N4Plus",
        Some("moonraker"),
        Some(80),
        Evidence::Unverified,
        NEPTUNE4_NGINX,
    ),
    elegoo(
        "Elegoo-N4Max",
        Some("moonraker"),
        Some(80),
        Evidence::Unverified,
        NEPTUNE4_NGINX,
    ),
    elegoo(
        "Elegoo-N3",
        None,
        None,
        Evidence::Unverified,
        MARLIN_NO_NETWORK,
    ),
    elegoo(
        "Elegoo-N3Pro",
        None,
        None,
        Evidence::Unverified,
        MARLIN_NO_NETWORK,
    ),
    elegoo(
        "Elegoo-N3Plus",
        None,
        None,
        Evidence::Unverified,
        MARLIN_NO_NETWORK,
    ),
    elegoo(
        "Elegoo-N3Max",
        None,
        None,
        Evidence::Unverified,
        MARLIN_NO_NETWORK,
    ),
    elegoo(
        "Elegoo-NX",
        None,
        None,
        Evidence::Unverified,
        MARLIN_NO_NETWORK,
    ),
    elegoo(
        "Elegoo-N2S",
        None,
        None,
        Evidence::Unverified,
        MARLIN_NO_NETWORK,
    ),
    elegoo(
        "Elegoo-N2D",
        None,
        None,
        Evidence::Unverified,
        MARLIN_NO_NETWORK,
    ),
    elegoo(
        "Elegoo-N2",
        None,
        None,
        Evidence::Unverified,
        MARLIN_NO_NETWORK,
    ),
    elegoo(
        "Elegoo-N1",
        None,
        None,
        Evidence::Unverified,
        MARLIN_NO_NETWORK,
    ),
];

/// Applies `overrides` to every variant of the model each one names.
pub fn apply_host_overrides(
    models: &mut [CatalogModel],
    overrides: &[HostOverride],
) -> Result<(), String> {
    for o in overrides {
        let mut matched = models
            .iter_mut()
            .filter(|m| m.vendor == o.vendor && m.model_id == o.model_id);
        let model = matched
            .next()
            .ok_or_else(|| format!("host override {}/{} matches no model", o.vendor, o.model_id))?;
        if matched.next().is_some() {
            return Err(format!(
                "host override {}/{} matches more than one model",
                o.vendor, o.model_id
            ));
        }
        for variant in &mut model.variants {
            variant.suggested_host_type = o.host_type.map(str::to_string);
            variant.suggested_port = o.port;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{BedShape, CatalogVariant};

    fn variant(host_type: Option<&str>) -> CatalogVariant {
        CatalogVariant {
            variant: "V 0.4 nozzle".to_string(),
            printer_variant: "0.4".to_string(),
            bed_shape: BedShape::Rectangular {
                width_mm: 220.0,
                depth_mm: 220.0,
                origin_x_mm: 0.0,
                origin_y_mm: 0.0,
            },
            printable_height_mm: 250.0,
            bed_exclude_areas: vec![],
            default_bed_type: String::new(),
            nozzle_diameter_mm: vec![0.4],
            nozzle_type: String::new(),
            gcode_flavor: "klipper".to_string(),
            has_auxiliary_fan: false,
            supports_air_filtration: false,
            supports_multi_filament: false,
            suggested_host_type: host_type.map(str::to_string),
            suggested_port: None,
        }
    }

    fn model(vendor: &str, model_id: &str, host_type: Option<&str>) -> CatalogModel {
        CatalogModel {
            model_id: model_id.to_string(),
            vendor: vendor.to_string(),
            model: format!("{vendor} {model_id}"),
            variants: vec![variant(host_type), variant(host_type)],
        }
    }

    const N4: HostOverride = HostOverride {
        vendor: "Elegoo",
        model_id: "Elegoo-N4",
        host_type: Some("moonraker"),
        port: Some(80),
        evidence: Evidence::Unverified,
        source: "test",
    };

    #[test]
    fn rewrites_every_variant_of_the_named_model_only() {
        let mut models = vec![
            model("Elegoo", "Elegoo-N4", Some("elegoolink")),
            model("Elegoo", "Elegoo-N3", Some("elegoolink")),
            model("Other", "Elegoo-N4", Some("octoprint")),
        ];
        let null_n3 = HostOverride {
            model_id: "Elegoo-N3",
            host_type: None,
            port: None,
            ..N4
        };

        apply_host_overrides(&mut models, &[N4, null_n3]).unwrap();

        for v in &models[0].variants {
            assert_eq!(v.suggested_host_type.as_deref(), Some("moonraker"));
            assert_eq!(v.suggested_port, Some(80));
        }
        for v in &models[1].variants {
            assert_eq!(v.suggested_host_type, None);
            assert_eq!(v.suggested_port, None);
        }
        assert_eq!(
            models[2].variants[0].suggested_host_type.as_deref(),
            Some("octoprint")
        );
    }

    #[test]
    fn an_override_matching_no_model_fails() {
        let mut models = vec![model("Elegoo", "Elegoo-N3", None)];
        let err = apply_host_overrides(&mut models, &[N4]).unwrap_err();
        assert!(err.contains("matches no model"), "{err}");
    }

    #[test]
    fn an_override_matching_two_models_fails() {
        let mut models = vec![
            model("Elegoo", "Elegoo-N4", None),
            model("Elegoo", "Elegoo-N4", None),
        ];
        let err = apply_host_overrides(&mut models, &[N4]).unwrap_err();
        assert!(err.contains("more than one model"), "{err}");
    }

    #[test]
    fn names_each_model_once() {
        let mut keys: Vec<_> = HOST_OVERRIDES
            .iter()
            .map(|o| (o.vendor, o.model_id))
            .collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), HOST_OVERRIDES.len());
    }
}
