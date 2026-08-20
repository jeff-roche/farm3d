# farm3d is single-user and local, not remote or multi-tenant

farm3d runs as a local desktop app for one person managing their own Farm —
no remote/browser access and no shared multi-user state in v1. This keeps
the architecture to "Tauri app talks directly to Printers on the LAN," with
no auth, sync backend, or permissions model to build. Remote or team access
would be a substantial later addition, not an incremental one.
