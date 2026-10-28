# Mission Control — sub-application

Mission Control keeps mission books: each book is a list of cells, and the host draws and runs every cell.

## Rules for this sub-application

- It is its own program. Its crate depends on `protocol`, `serde`, and `serde_json` only, never on the host or on another sub-application.
- Everything the host shows for it is declared through the protocol: its tabs, menu rows, tools, and the cells contents. Nothing about it is hard-coded in the host.
- It owns its books: the order of cells, their prompts, their finished results, which cell is selected, how a book is saved, and its undo. It never draws a cell and never calls a network.
- It runs with no host, driven only through its standard input and standard output.
- It speaks the protocol defined in `protocol/` and nothing else. When it needs something the protocol cannot say, change the protocol, the host, and the scaffolding skill together. Do not work around the protocol.
- `manifest.json` registers it with the host. Its id, folder, crate, and program names are the same.
