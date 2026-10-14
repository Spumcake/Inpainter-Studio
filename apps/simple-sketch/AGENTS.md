# Simple Sketch

Simple Sketch is the pressure-sensitive sketching sub-application.

## Rules for this sub-application

- It is its own program. Its crate depends on `protocol` only, never on the host or on another sub-application.
- Everything the host shows for it is declared through the protocol: its tabs, menu rows, tools, properties, and the shared components it uses. Nothing about it is hard-coded in the host.
- It owns its document: how the work is arranged, how it is saved, and its undo.
- It runs with no host, driven only through its standard input and standard output.
- It speaks the protocol defined in `protocol/` and nothing else. When it needs something the protocol cannot say, change the protocol, the host, and the scaffolding skill together. Do not work around the protocol.
- `manifest.json` registers it with the host. Its id, folder, crate, and program names are the same.
