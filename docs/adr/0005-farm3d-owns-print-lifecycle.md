# farm3d dispatches and tracks Jobs itself, not just monitors or prepares files

farm3d sends a Sliced Job to its target Printer and tracks it through
completion, rather than being a read-only fleet monitor or a G-code-prep
tool that stops short of printing. This means farm3d needs write access to
each Printer's Connection and must handle job-state tracking end-to-end,
not just status polling.
