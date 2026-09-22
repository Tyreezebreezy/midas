# MIDAS data ownership

The owner controls all financial data. Runtime facts remain canonical in [the README](../README.md), the Rust database layer, and ordered SQL migrations.

- Canonical store: `%APPDATA%\com.breezy.midas\midas.sqlite3`.
- Source/import provenance: retained in the same database and linked to normalized transactions.
- Network/cloud copy: none.
- Encryption at rest: none; Windows account and disk security are the boundary.
- Backup: SQLite `VACUUM INTO` snapshot selected by the owner.
- Restore: preview and explicit confirmation; the previous database is renamed to a timestamped recovery copy.
- Export: owner-selected CSV is portable output, not a complete backup.
- Deletion/destructive operations: require the application workflow and preserve ledger integrity; this control file grants no filesystem authority.
