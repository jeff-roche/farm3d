use crate::catalog::resolve::resolve_catalog_ref;
use crate::catalog::PrinterProfile;
use crate::contracts::command::{CommandError, CommandSuccess, IncomingContractVersion};
use crate::printers::CatalogRef;
use serde::{Deserialize, Serialize};
use tauri::State;
use ts_rs::TS;

#[derive(Serialize, Deserialize, Clone, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/CatalogModelSummary.ts")]
pub struct CatalogModelSummary {
    pub model_id: String,
    pub vendor: String,
    pub model: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/CatalogVariantSummary.ts"
)]
pub struct CatalogVariantSummary {
    pub variant: String,
    pub printer_variant: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/CatalogInfo.ts")]
pub struct CatalogInfo {
    pub generated_at: String,
    pub source_tag: String,
    pub model_count: usize,
    pub variant_count: usize,
}

#[tauri::command]
pub fn list_catalog_models<R: tauri::Runtime>(
    _app: tauri::AppHandle<R>,
    contract_version: IncomingContractVersion,
    bootstrap: State<crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
) -> Result<CommandSuccess<Vec<CatalogModelSummary>>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let models = services
        .catalog
        .models
        .iter()
        .map(|m| CatalogModelSummary {
            model_id: m.model_id.clone(),
            vendor: m.vendor.clone(),
            model: m.model.clone(),
        })
        .collect::<Vec<_>>();
    if models.is_empty() {
        Err(CommandError::internal())
    } else {
        Ok(CommandSuccess::new(models))
    }
}

/// Looks a model up by its `(vendor, model)` name pair, NOT by `modelId` —
/// `modelId` is not unique in the real catalog (11 models across 5 groups share
/// one with a same-vendor sibling, and 3 Cubicon models share `modelId: ""`),
/// so keying on it would silently return another model's variants.
#[tauri::command]
pub fn list_catalog_variants<R: tauri::Runtime>(
    _app: tauri::AppHandle<R>,
    bootstrap: State<crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
    vendor: String,
    model: String,
) -> Result<CommandSuccess<Vec<CatalogVariantSummary>>, CommandError> {
    contract_version.validate()?;
    if vendor.is_empty() || model.is_empty() {
        return Err(CommandError::validation("Vendor and model are required."));
    }
    let services = bootstrap.ready()?;
    let variants = services
        .catalog
        .models
        .iter()
        .find(|m| m.vendor == vendor && m.model == model)
        .map(|m| {
            m.variants
                .iter()
                .map(|v| CatalogVariantSummary {
                    variant: v.variant.clone(),
                    printer_variant: v.printer_variant.clone(),
                })
                .collect()
        })
        .ok_or_else(|| CommandError::not_found(format!("{vendor}/{model}")))?;
    Ok(CommandSuccess::new(variants))
}

#[tauri::command]
pub fn preview_profile<R: tauri::Runtime>(
    _app: tauri::AppHandle<R>,
    bootstrap: State<crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
    catalog_ref: CatalogRef,
) -> Result<CommandSuccess<PrinterProfile>, CommandError> {
    contract_version.validate()?;
    if catalog_ref.vendor.is_empty()
        || catalog_ref.model.is_empty()
        || catalog_ref.variant.is_empty()
        || catalog_ref.model_id.is_empty()
        || catalog_ref.printer_variant.is_empty()
    {
        return Err(CommandError::validation_at(
            "catalogRef",
            "The catalog reference is incomplete.",
        ));
    }
    let services = bootstrap.ready()?;
    let (variant, status) = resolve_catalog_ref(&services.catalog, &catalog_ref);
    variant
        .map(PrinterProfile::from)
        .map(CommandSuccess::new)
        .ok_or_else(|| CommandError::not_found(format!("{status:?}")))
}

#[tauri::command]
pub fn catalog_info<R: tauri::Runtime>(
    _app: tauri::AppHandle<R>,
    contract_version: IncomingContractVersion,
    bootstrap: State<crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
) -> Result<CommandSuccess<CatalogInfo>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let catalog = &services.catalog;
    Ok(CommandSuccess::new(CatalogInfo {
        generated_at: catalog.generated_at.clone(),
        source_tag: catalog.source_tag.clone(),
        model_count: catalog.models.len(),
        variant_count: catalog.models.iter().map(|m| m.variants.len()).sum(),
    }))
}
