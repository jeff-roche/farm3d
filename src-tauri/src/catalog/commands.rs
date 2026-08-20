use crate::catalog::resolve::resolve_catalog_ref;
use crate::catalog::{Catalog, PrinterProfile};
use crate::printers::CatalogRef;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModelSummary {
    pub model_id: String,
    pub vendor: String,
    pub model: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CatalogVariantSummary {
    pub variant: String,
    pub printer_variant: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CatalogInfo {
    pub generated_at: String,
    pub source_tag: String,
    pub model_count: usize,
    pub variant_count: usize,
}

#[tauri::command]
pub fn list_catalog_models(catalog: State<Arc<Catalog>>) -> Vec<CatalogModelSummary> {
    catalog
        .models
        .iter()
        .map(|m| CatalogModelSummary {
            model_id: m.model_id.clone(),
            vendor: m.vendor.clone(),
            model: m.model.clone(),
        })
        .collect()
}

#[tauri::command]
pub fn list_catalog_variants(
    catalog: State<Arc<Catalog>>,
    model_id: String,
) -> Vec<CatalogVariantSummary> {
    catalog
        .models
        .iter()
        .find(|m| m.model_id == model_id)
        .map(|m| {
            m.variants
                .iter()
                .map(|v| CatalogVariantSummary {
                    variant: v.variant.clone(),
                    printer_variant: v.printer_variant.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tauri::command]
pub fn preview_profile(
    catalog: State<Arc<Catalog>>,
    catalog_ref: CatalogRef,
) -> Result<PrinterProfile, String> {
    let (variant, status) = resolve_catalog_ref(&catalog, &catalog_ref);
    variant.map(PrinterProfile::from).ok_or_else(|| {
        format!("cannot preview an unresolvable catalog reference (status: {status:?})")
    })
}

#[tauri::command]
pub fn catalog_info(catalog: State<Arc<Catalog>>) -> CatalogInfo {
    CatalogInfo {
        generated_at: catalog.generated_at.clone(),
        source_tag: catalog.source_tag.clone(),
        model_count: catalog.models.len(),
        variant_count: catalog.models.iter().map(|m| m.variants.len()).sum(),
    }
}
