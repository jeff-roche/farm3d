# Printer connectivity is pluggable, not a single protocol

Printers a user owns may already expose different control surfaces:
network print-server APIs (OctoPrint, Moonraker/Klipper, Bambu-style) or a
directly-attached USB/serial connection. Rather than standardizing on one,
each Printer's Connection type is chosen independently, so farm3d can
manage a mixed farm. The cost is maintaining multiple connection
implementations behind a shared interface instead of just one.
