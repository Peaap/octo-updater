# Octo Updater Desktop

The Rust/Tauri/Svelte successor to the legacy Python `octo_updater.py`.

## Current foundation

- Tauri desktop shell with a Svelte UI.
- Per-user persisted game-directory profile.
- Typed Rust updater status exposed through narrow Tauri commands.
- Atomically written update journals and a startup-safe recovery action.
- A `prepare_update` operation that records an update transaction before later download, validation, and apply phases are implemented.

## Development

```powershell
npm install
npm run tauri dev
```

## Validation

```powershell
npm run check
cargo check --manifest-path src-tauri/Cargo.toml
```

## Migration path

1. Persist the exact fetched torrent bytes and build an immutable update plan.
2. Stream bounded downloads into a staging directory and verify integrity.
3. Apply game-file changes through journaled, atomic replacements with rollback.
4. Port executable patching, mod installation, addon installation, and content patches into separate Rust services.
5. Remove the Python implementation only after feature parity and migration testing.
