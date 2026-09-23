//! D8/D11: the payload later P3 operations hand to Task 7's broadcast —
//! which Spools and Printers a listener should reload after commit. This
//! task defines the type only; Task 7 wires up emission through
//! `RuntimeServices` (D11: events and the broadcast fire after commit only,
//! never on rollback or replay).

/// Every entity id one operation touched, for a post-commit reload.
#[derive(Clone, Debug)]
pub struct InventoryChange {
    pub spool_ids: Vec<String>,
    pub printer_ids: Vec<String>,
}
