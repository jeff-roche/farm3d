# Slicing wraps OrcaSlicer instead of a custom or shelled-out engine

farm3d performs Slicing by invoking OrcaSlicer from the Rust backend,
rather than writing a slicing engine from scratch (no mature Rust-native
slicing core exists to build on) or leaving slicing out of scope for v1.
OrcaSlicer is AGPL-3.0 (upstream moved to `OrcaSlicer/OrcaSlicer` on GitHub);
keeping it a separate invoked process rather than
linking it in-process avoids license entanglement with farm3d's own code.
