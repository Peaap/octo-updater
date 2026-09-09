# Octo Updater Rust Migration Tasks

This document tracks the remaining migration from the legacy Python/Tkinter updater (`octo_updater.py`) to the Rust + Tauri + Svelte application in `desktop/`.

## Architecture decisions

- **Mods are packaged as standalone MPQ patch archives**, not files bundled with the game-client torrent. Packaging/reading MPQs is done via a vendored copy of [StormLib](https://github.com/ladislav-zezula/StormLib) (MIT license, pinned at tag `v9.40`, commit `6bb1882`) in `desktop/src-tauri/vendor/stormlib`, compiled from source through `build.rs`/`cc` and exposed to Rust via a small FFI layer in `desktop/src-tauri/src/mpq/`. A mod's patch MPQ is downloaded/verified independently of the client sync and dropped into the client's `Data` folder, matching the pattern the legacy Python updater already used for optional content patches (e.g. raid visuals), generalized to cover mods.
- **Addons are being redesigned as a modular Rust package-management platform**, not ported as a Python folder-replacement workflow. The legacy updater remains the behavioral compatibility baseline for immutable commit-based acquisition, bounded archive handling, replacement safety, supported providers, recommended/custom entries, and pfUI behavior. The current curated GitHub installer is a deliberately narrow safety foundation; Milestone 4.1 evolves it into a package model, resolver, artifact cache, journaled installation engine, profiles, diagnostics, and provider/integration abstractions.

## Working rules

- Keep `octo_updater.py` as the behavioral reference until the Rust application reaches verified feature parity.
- Do not expose a UI action that changes game files until its Rust transaction, recovery, and validation path is complete.
- The updater must consume the exact torrent persisted in an `UpdatePlan`; never re-fetch a mutable remote torrent after planning.
- Treat a network failure as `Offline`/`Failed`, never as `UpdateAvailable`.
- All user-visible background work must emit typed progress/state events, not update the Svelte UI directly from worker code.
- Validate each milestone with targeted Rust tests when they are introduced, `cargo check`, `cargo test`, `npm run check`, `npm run build`, and a manual Tauri smoke test.

## Completed foundation

- [x] Create the Tauri + Svelte desktop shell in `desktop/`.
- [x] Persist a game-folder profile in per-user application data.
- [x] Add atomic JSON persistence and update journals.
- [x] Fetch the client torrent over bounded, allowlisted HTTPS.
- [x] Parse and validate torrent bencode, file paths, and total sizes.
- [x] Persist exact torrent bytes by SHA-256 and create immutable update plans.
- [x] Read-only plan delta: identify missing and wrong-sized files.
- [x] Refuse planning when WoW is running or the Windows path is too long.
- [x] Stage a local-torrent-only aria2 adapter with progress parsing and cancellation support.

---

## Milestone 1 — Journaled update execution

### 1.1 Define the transaction state machine

- [x] Replace the journal's free-form `state` string with a serializable enum: `Prepared`, `Downloading`, `Verifying`, `Applying`, `Complete`, `Cancelled`, `Failed`, `Abandoned` (`updater/model.rs::TransactionState`).
- [x] Store timestamps, last error, selected file indices, local torrent hash/path, selected game directory (`updater/model.rs::UpdateJournal`). Explicit backup/staging paths are still needed once Milestone 2/3/4 transactions exist.
- [x] Add legal transition validation; reject impossible transitions such as `Complete -> Downloading` (`TransactionState::can_transition_to`, enforced in `UpdaterService::transition`, unit-tested).
- [x] Recover every incomplete transaction on startup: `Prepared` -> `Abandoned` (no file touched yet); any other non-terminal state -> `Failed` with a recorded reason, never silently discarded (`recover_interrupted_updates`, unit-tested).

**Acceptance criteria:** Met for the states currently reachable (`Prepared`, `Downloading`, `Verifying`, `Applying`). Re-verify once Milestone 2/3/4 add more write phases inside `Applying`.

### 1.2 Activate aria2 through a transaction executor

- [x] Resolve a verified aria2 binary location (`updater/aria2_provision.rs::ensure_aria2`).
- [x] Download aria2 only through a bounded, HTTPS-only, host-allowlisted path with a pinned SHA-256 (same pin as the legacy Python updater).
- [x] Re-hash the archive on every fresh download; the cached executable is currently only checked for non-emptiness, not re-hashed per call — **follow-up:** store the expected exe hash alongside the cache and verify it every launch, not just on first provision.
- [x] Start `aria2` only from a journal that has just transitioned to `Downloading`, and only with the plan's local torrent file (`updater/executor.rs::execute_update`, `updater/aria2.rs::sync_client`).
- [x] Persist `Downloading` before starting the child process.
- [x] Forward parsed progress to the Tauri event bus (`update-progress`) with plan ID, bytes, speed, and phase. ETA is not yet computed — **follow-up.**
- [x] Add a cancel command (`cancel_update`) that terminates aria2 via a shared `CancelHandle` and results in a `Cancelled` journal state; resume metadata is left untouched by cancellation itself.
- [x] On child exit, classify success, cancellation, and failure distinctly (`Cancelled` vs `Failed` vs success path into `Verifying`).

**Acceptance criteria:** Implemented and unit-tested at the parsing/validation level (progress parsing, executable validation). **Not yet verified end-to-end against a real aria2 process or a real client torrent** — that requires a manual smoke test with network access and is tracked in Milestone 7.

### 1.3 Post-download validation and completion

- [x] Verify every selected path exists, is a regular file, and has the expected length (`executor.rs::verify_selected_files`), run during the `Verifying` phase before `Applying`/`Complete` can be reached.
- [ ] Add full torrent piece-integrity mode using aria2 `--check-integrity`. The adapter supports `check_integrity`, but the executor does not yet expose or schedule an integrity-mode transaction distinct from a normal sync.
- [x] Record a successful client/torrent identity only after validation succeeds (`UpdaterService::record_synced`, called only after `execute_update` returns `Ok`).
- [x] Port realmlist.wtf healing (`client/realmlist.rs::heal_realmlist`, ported from the reference OctoLauncher's `healRealmlist`/`applyRealmlist`): rewrites `realmlist.wtf` — at the client root and any `Data/<locale>/realmlist.wtf` — only when missing, empty, or wrong, via atomic temp-file + rename. Called defensively before *and* after every update transaction (non-fatal on failure, so an unrelated write error never blocks a verified sync from completing) and again immediately before every launch, since an interrupted sync's 0-byte placeholder is exactly the kind of silent-disconnect bug this exists to close. Unit-tested (5 tests): missing, zero-byte, wrong-host, already-correct (verified not rewritten), and locale-scoped.
- [ ] Preserve user-protected files (mods and optional custom `speech.mpq`) across shared-piece writes. Not implemented — depends on a torrent-side protected-file list; the mod-owned-file registry itself now exists (3.1/3.2) but isn't yet cross-referenced against the aria2 shield/unshield logic from the Python reference.
- [x] Clear only the resume metadata associated with the active game folder/torrent identity when stale (`is_stale_resume_context` + `clear_resume_state`, scoped to `.aria2` and `client.torrent` files in the staging directory only).
- [ ] Add stale-client cleanup rules (legacy locale folders, old patch archives) — not yet ported from the Python reference.

**Acceptance criteria:** Partially met. A failed/partial/wrong-size sync correctly cannot reach `Complete` (unit-tested via `verify_selected_files` logic and the transition whitelist). Full integrity-repair mode and protected-file preservation remain open.

---

## Milestone 2 — Client safety, patching, and launch

### 2.1 Per-installation client state

- [x] Replace global pristine executable state with a per-game-directory, per-torrent-hash cache (`client/pristine.rs::pristine_cache_key`, keyed by SHA-256 of `game_directory|manifest_sha256`).
- [x] Record SHA-256 for every cached pristine `WoW.exe` — via the cache-key derivation. **Follow-up:** also store the pristine file's own content hash alongside it for defense-in-depth (currently the pristine-byte marker check is the integrity gate, not a stored hash).
- [x] Reject a pristine cache entry that doesn't verify as pristine (`read_pristine_or_current` re-checks the locale-assert marker byte before trusting a cached file; falls back to the current on-disk exe otherwise), unit-tested.
- [x] Detect game processes before update preparation (`updater/service.rs::wow_process_running`, now backed by the shared `client::is_process_running` guard) and before launch (`client::launch_game_process`).
- [ ] Add an installation lock so two updater instances cannot change the same folder concurrently. Not implemented — tracked as a real gap; two running instances could race on the same game directory.

**Acceptance criteria:** Met for pristine-cache scoping and reuse rejection (unit-tested). Cross-instance locking remains open.

### 2.2 Atomic executable and configuration changes

- [x] Port version detection from `WoW.exe` with bounds checks (`client/version.rs::read_client_version`), unit-tested including a garbage-bytes case.
- [x] Port locale and tweak definitions (`client/locale.rs`, `client/tweaks.rs`), unit-tested (locale slot mapping, FOV aspect-ratio defaults, clamping).
- [ ] Validate expected original bytes before patching each executable offset. Currently only bounds-checks buffer length (`out_of_range`); does not yet assert the pristine byte pattern at each offset before overwriting it — real follow-up before this is production-safe against an unexpected client build.
- [x] Build patched bytes from the validated pristine executable only (`patcher::patch_wow_exe` reads via `read_pristine_or_current`).
- [x] Write `WoW.exe` through a same-volume temporary file, flush, verify, and atomically rename (`patcher::patch_wow_exe`).
- [x] Port `Config.wtf` creation via atomic temp-file + rename (`patcher::write_config_wtf`), unit-tested. Only written when missing — matches the Python reference's non-destructive default; the update/reconcile variant that rewrites specific keys on an existing file is not yet ported.
- [ ] Port realmlist healing. Not implemented — `realmlist.wtf` is not yet written/healed by the Rust client module.

**Acceptance criteria:** Partially met. Atomic write safety is real and tested; pre-patch byte validation and realmlist healing remain open.

### 2.3 Safe launcher behavior

- [x] Port VanillaFixes-aware game launch selection (`client/launch.rs::select_launch_executable`), unit-tested. **Follow-up:** once Milestone 3's mod state exists, also check that VanillaFixes is *enabled*, not just present on disk (matching the Python reference more closely).
- [x] Add optional WDB clearing on launch (`client/launch.rs::clear_wdb`, gated by a persisted `clear_wdb_on_launch` preference exposed via `set_clear_wdb_on_launch`). Minimize-on-launch is not yet ported.
- [x] Prevent duplicate launch clicks: `launch_game` checks the process list before spawning and errors if the target executable is already running.
- [x] Confirm the selected launch executable exists before spawning it (`select_launch_executable` only returns a path that exists).

**Acceptance criteria:** Met for the ported behaviors (unit-tested selection logic; process-running guard blocks a duplicate launch). Minimize-on-launch and enabled-mod-aware selection remain open.

---

## Milestone 3 — Mods as MPQ patches

### 3.0 MPQ engine (StormLib integration)

- [x] Vendor StormLib at a pinned tag (`v9.40`) into `desktop/src-tauri/vendor/stormlib`, stripped of unrelated build-system files (VS projects, docs, test harness), with its MIT `LICENSE` retained.
- [x] Compile the vendored C/C++ sources directly via `cc` in `build.rs` (two static libs: C++ sources and C/bundled-zlib/bzip2/LibTomCrypt/LibTomMath sources), so `cargo build` is self-contained and does not require a separate CMake invocation.
- [x] Add a minimal, explicit FFI surface (`mpq/ffi.rs`) covering only the StormLib functions actually used: open/create/close/flush archive, add file, has-file, open/read/close file, find-first/next/close.
- [x] Wrap the FFI in a safe `MpqArchive` type (`mpq/archive.rs`) that always closes/flushes on `Drop`, stages file additions through a temporary source file, and rejects overwriting an existing archive path on create.
- [x] Add `pack_mpq` (`mpq/pack.rs`): builds a new MPQ from in-memory files and atomically installs it via a same-directory staging file + rename, returning the packaged archive's SHA-256.
- [x] **Found and fixed a real bug during testing, not just compilation:** the initial `SFileOpenArchive` call passed `SFILE_OPEN_HARD_DISK_FILE` (a legacy `dwPriority` value) as the `dwFlags` argument, which corrupted the open call and crashed inside StormLib (`STATUS_ACCESS_VIOLATION`) on every archive re-open. Diagnosed via targeted `eprintln!` tracing across the FFI boundary (create → add → close → **open crashes here** → has_file → read → close) until the exact failing call was isolated, then fixed by passing `0` for both priority and flags.
- [x] Add unit tests that actually exercise StormLib (not just compile against it): create/add/close/reopen/has-file/read round trip, list-files, refuse-to-overwrite-existing-path, refuse-to-package-zero-files, and a partial-failure case that confirms no half-written archive is left behind (7 tests in `mpq::tests` and `mpq::pack::tests`).

**Acceptance criteria:** Met and verified by running (not just compiling) the tests above; the previously-crashing open path now round-trips correctly. StormLib's LICENSE is preserved in the vendored tree; attribution still needs to be added to the release build's third-party notices (tracked under 6.2).

### 3.1 Mod registry and release metadata

- [x] **Correction from an earlier draft of this plan:** the legacy mods (VanillaFixes, ClassicAPI, Nampower, TransmogFix, etc.) are DLL/EXE files placed in the client root and registered in `dlls.txt` — they are not MPQ archives. The registry (`mods/model.rs::ModSource`) now models two real kinds: `DllPatch` (one or more files placed at client-root-relative paths, optionally registered in `dlls.txt`) and `MpqPatch` (a standalone MPQ verified against a `.sha256` sidecar, for content-only mods).
- [x] Port the curated mod registry into typed Rust data (`mods/registry.rs::mod_registry`), separating the immutable registry from mutable per-mod install state (`ModInstallState`, persisted in `UpdaterProfile.mods`).
- [x] Populate the registry with real entries ported from the legacy Python `MODS_REGISTRY`: `TransmogFix` and `No1600x1200` (both `direct_file` DLL sources — a single pinned URL, no archive extraction) plus the one real MPQ content patch, Octo Raid Visuals. Unit-tested for unique ids and an HTTPS+allowlisted-host URL on every entry.
- [ ] Add bounded, allowlisted release/API fetches with clear rate-limit and offline errors — **not done**. The release-asset mods that need GitHub/Codeberg API resolution and zip/tar extraction (VanillaFixes, ClassicAPI, DXVK, Nampower, SuperWoW, UnitXP_SP3, VanillaHelpers, VanillaMultiMonitorFix, AuctionQueryThrottle) are not yet in the registry; only `direct_file`-equivalent sources are ported so far.
- [ ] Define trusted release asset selection rules and supported archive layouts — blocked on the API-resolver work above.
- [ ] Add artifact hashes/signatures where upstream projects publish them — the two ported DLL mods use pinned direct URLs without a published hash (matching their Python source entries, which also have no hash check); MPQ mods are hash-verified via their sidecar.

**Acceptance criteria:** Partially met. The two-kind model and the entries that don't require archive/API resolution are real, wired, and tested. The seven release-asset mods remain unported.

### 3.2 Transactional mod install/uninstall

- [x] Add `dlls_txt` (`mods/dlls_txt.rs`): atomic, case-insensitive, deduplicated `add_dll`/`remove_dll`, deleting the file entirely once empty. **`dlls.txt` is never exposed to the user as free-text editing** — every mutation goes through these two functions, tied to a specific mod's install/uninstall, so the file always reflects exactly what this updater installed and can always be safely reconstructed on uninstall. Unit-tested (5 tests): create, idempotent/case-insensitive add, delete-when-empty, keep-other-entries, no-op on a missing file.
- [x] Add `install_mod` (`mods/install.rs`): downloads every file for a `DllPatch` mod into app-data staging first; only after every download succeeds are files moved into the game directory and (if applicable) the DLL registered — a failed download touches nothing in the game directory, and a failure partway through moving a multi-file mod rolls back the files already moved. For an `MpqPatch` mod, downloads and verifies against its `.sha256` sidecar before an atomic `.part`-staged install into `Data`.
- [x] Add `uninstall_mod`: removes only the paths recorded in a mod's persisted `ModInstallState.installed_files` and its `dlls.txt` entry if any — never a path the current registry happens to claim, so a changed/stale registry entry can't cause deletion of a file this updater didn't actually install for that mod. Unit-tested.
- [x] Add `published_sha256_for` for update-availability checks on `MpqPatch` mods (returns `None`, not an error, when the sidecar is unreachable, and always `None` for `DllPatch` mods — a pinned direct-file URL has no reliable remote version signal, matching the legacy updater's `mod_supports_update_check` returning `false` for `direct_file` sources).
- [x] Wire real Tauri commands (`get_mod_statuses`, `install_mod`, `uninstall_mod` in `lib.rs`) and a Mods tab in the Svelte UI (install/remove buttons, essential/update badges), replacing the placeholder `#[allow(dead_code)]` library code from the prior draft.
- [x] Refuse to install/uninstall a mod while `WoW.exe` is running (reuses the shared `client::is_process_running` guard used by update preparation and launch).
- [ ] Preserve and restore mod-owned root files during torrent syncs — depends on cross-referencing `ModInstallState.installed_files` with the aria2 shield/unshield logic in the executor (tracked under 1.3).
- [ ] Explicit backup/rollback for an `MpqPatch` mod when a *newer* install fails after an older one already exists (today a failed download simply never reaches the atomic-rename step, so the old file survives untouched — but there's no distinct "rollback" path being exercised/tested for that case specifically).

**Acceptance criteria:** Met for the two-kind install/uninstall pipeline (unit-tested, atomic per-file-set, staged-before-move for DLL mods) and its Tauri/UI wiring. Full registry coverage (3.1) and cross-referencing with torrent-sync file protection remain open.

---

## Milestone 4 — Addons and content patches

### 4.1 Addon management platform

**Decision:** redesign the legacy addon workflow into a modular Rust package-management platform. The legacy Python system is the compatibility baseline—not the target architecture. The manager must preserve safe immutable acquisition and rollback behavior while adding an explicit package model, desired-state reconciliation, dependency-aware planning, artifact caching, profiles, diagnostics, and pluggable repository/game integrations.

#### 4.1.1 Safety foundation — complete, provisional

- [x] Add a curated GitHub-only registry derived from legacy recommended addons, with stable ids, typed metadata, and no user-supplied repository URLs (`addons/registry.rs`). `pfUI` is deliberately excluded until its profile integration is separately designed.
- [x] Persist exact addon-owned, game-directory-relative file paths and the resolved immutable Git commit SHA in `UpdaterProfile.addons`; uninstall never trusts a mutable registry definition to decide what to delete.
- [x] Resolve a GitHub commit SHA, download a commit-pinned ZIP through exact HTTPS host allowlists (`api.github.com`, `github.com`, `codeload.github.com`), enforce archive and extraction limits, reject traversal/absolute paths, stage files, preserve/restore an existing destination when replacement fails, and install atomically.
- [x] Refuse addon mutation while `WoW.exe` runs; expose `get_addon_statuses`, `install_addon`, and `uninstall_addon` Tauri commands with structured activity logs and a sidebar Addons page.
- [x] Validate the initial foundation with Rust unit tests, `cargo fmt`, `cargo check`, `cargo test`, `npm run check`, and `npm run build` (62 Rust tests passing; frontend check/build passing).
- [ ] Add integration tests with temporary game folders and fixture ZIPs for valid install, traversal rejection, locked target, replacement rollback, exact-owned-file removal, and recovery after an interrupted replacement.

**Acceptance criteria:** The provisional curated installer cannot write outside `Interface/AddOns`, cannot remove unowned user files, and preserves a prior working folder if a replacement fails. It is not yet the complete addon manager.

#### 4.1.2 Package model and catalog engine

- [ ] Define versioned, schema-validated `AddonPackage` metadata: id, display metadata, kind/capabilities, compatible game builds, repository source, artifact constraints, dependencies, optional dependencies, conflicts, provides/replaces, load ordering, and integration hooks.
- [ ] Define an explicit catalog document format with schema versioning, expiry/ETag cache metadata, signature/hash policy, source provenance, and migration handling for older cached catalog versions.
- [ ] Port the legacy recommended-addon list as a first-party catalog layer; support server catalogs only after schema and trust validation are complete.
- [ ] Support discovery/search/filtering without treating a failed catalog refresh as an update signal; cached content remains distinguishable from verified-current content.

**Acceptance criteria:** Catalog entries are independently validated and versioned; a malformed, expired, or untrusted catalog cannot change the desired addon state.

#### 4.1.3 Repository-provider and artifact engine

- [ ] Introduce a `RepositoryProvider` abstraction that resolves a repository reference into a provider-neutral immutable `ResolvedArtifact` (package id/version or revision, commit SHA, archive URL, integrity information, provenance).
- [ ] Implement GitHub first, then GitLab, Gitea, and Codeberg behind the same abstraction; provider-specific URL/API handling must not leak into the package or installation engines.
- [ ] Retain strict HTTPS and redirect-host validation, bounded streaming downloads, archive-entry/path/count/uncompressed-size limits, explicit rate-limit/offline errors, and supported archive-layout validation.
- [ ] Add a content-addressed artifact cache with verified metadata, archive hash, access timestamps, bounded retention, and safe cache eviction. Cached artifacts must support offline reinstall and rollback without changing provenance.
- [ ] Prefer published hashes/signatures when a trusted upstream makes them available; otherwise persist the computed artifact hash with the resolved immutable source.

**Acceptance criteria:** Every install plan references an immutable, verified artifact and can explain where it came from. Provider failure is a typed offline/error result, never an unverified update claim.

#### 4.1.4 Dependency resolution and desired-state planning

- [ ] Persist separate **desired state** (profiles/package intent, enabled state, selected version constraints) and **actual state** (installed revision, artifact hash, files, health, transaction provenance).
- [ ] Implement deterministic dependency resolution for required/optional dependencies, conflicts, provides/replaces, compatibility, and load-order constraints; report actionable unsatisfiable/conflict explanations.
- [ ] Generate an immutable addon install plan before writing: packages to acquire, filesystem changes, backups, enable/disable transitions, integration actions, verification steps, and estimated storage requirements.
- [ ] Add read-only reconciliation/repair planning so the manager can identify missing, damaged, unmanaged, or stale addons without changing files.

**Acceptance criteria:** A request such as profile application yields an inspectable plan, never implicit piecemeal installs. No filesystem change begins until the complete dependency/conflict solution is accepted.

#### 4.1.5 Journaled installation and recovery engine

- [ ] Generalize addon operations into typed journals: resolve, acquire, verify, stage, validate, backup, apply, post-integrate, verify-installation, commit-state, cleanup, rollback, failed, recovered.
- [ ] Ensure staged package trees are validated before any destination mutation; use same-volume atomic moves where possible and explicit durable backup/restore paths otherwise.
- [ ] Verify installed files, expected addon metadata/TOCs, dependency/load-order health, and state persistence before committing a transaction.
- [ ] Implement startup recovery and idempotent rollback/cleanup for every incomplete state, plus bounded transaction history and diagnostic log references.
- [ ] Add cross-instance/game-directory locking so simultaneous launcher processes cannot modify the same addon tree.

**Acceptance criteria:** An interrupted or locked operation can neither lose the previous working addon nor leave persisted state claiming an unverified install.

#### 4.1.6 Profiles, health, and integrations

- [ ] Introduce first-class profiles (Default, PvE, PvP, Minimal, Custom) that declare desired package versions, enabled states, ordering, and integration configuration; profile switches are planned/reconciled transactions.
- [ ] Inspect WoW addon metadata (`.toc`) to report folder validity, title/version/interface compatibility, enabled/disabled status, declared dependencies, load order, and unmanaged local addons.
- [ ] Define an `AddonIntegration` abstraction for game-specific hooks. Implement the WoW metadata integration first.
- [ ] Reimplement pfUI customization as a narrowly scoped, idempotent, version-aware integration with explicit preview/backup/rollback behavior—not an untracked post-install text mutation.
- [ ] Add controlled custom-addon source support only after the provider, catalog, trust, and transaction model is complete; never allow arbitrary unvalidated URLs directly from the UI.
- [ ] Provide health checks, repair plans, diagnostics, import/export, and a transaction/history view. Add update-all only once plan resolution and remote revision comparisons are complete.

**Acceptance criteria:** Profiles can be safely applied and repaired; integrations are independently testable and cannot bypass the package transaction engine.

#### 4.1.7 Shared CLI and UI

- [ ] Extract the package engine behind a UI-independent Rust API and provide a CLI for inspect/search/plan/install/remove/profile/verify/repair/rollback operations.
- [ ] Evolve the Svelte Addons page from per-item install/remove into package search, plan review, dependency/conflict display, profile controls, health states, transaction progress/history, and recoverable errors.
- [ ] Emit typed background events for long-running addon transactions instead of blocking UI commands; preserve actionable logs in the common session/activity stream.

**Acceptance criteria:** CLI and GUI invoke the same core behavior and produce equivalent plans/state changes.

### 4.2 MPQ content patches

- [x] MPQ packaging/verification primitives now exist and are shared with mods (Milestone 3.0/3.2): bounded download, sidecar SHA-256 verification, atomic `.part`-staged install.
- [x] Octo Raid Visuals is now ported and reachable end-to-end as an `MpqPatch` mod entry (`mods/registry.rs`), sharing the same install/uninstall/status pipeline and Mods tab as DLL mods — this covers the one real content patch the legacy updater ships.
- [ ] Port a distinct content-patch-specific registry/UI section if more non-mod content patches are ever added; today the one real content patch (raid visuals) is folded into the general Mods tab rather than a separate MPQ-patches section, which is an acceptable simplification for now but a design choice worth revisiting if the catalog grows.
- [ ] Port optional patch enable/disable — currently install/uninstall is the only lifecycle; there is no "installed but disabled" state.

**Acceptance criteria:** Met for the one real content patch that exists (Octo Raid Visuals), via the shared mod pipeline. A dedicated MPQ-patches UI section and an enable/disable-without-uninstall lifecycle remain open.

---

## Milestone 5 — Tauri/Svelte UI parity

### 5.1 Typed backend events and stores

- [x] Define Rust event payloads for update progress (`UpdateProgressEvent`, emitted as `update-progress`) and structured logs (`log_events.rs::LogEntry`/`LogLevel`, emitted as `app-log` and buffered server-side in a capped ring buffer so a viewer opened after startup still sees recent history). Mods, addons, and self-update checks don't have dedicated event streams yet — mod state changes are returned directly from their Tauri commands instead, which is sufficient for the current single-action-at-a-time UI but would need real events if mod installs ever run in the background like updates do.
- [ ] Create Svelte stores that own UI state only — **not done**; `+page.svelte` currently holds all state directly in one file's `$state` variables rather than extracted stores. Functionally fine at the current size, but this is a real gap if the UI keeps growing (tracked here rather than silently accepted).
- [x] Render explicit transaction states in the update card (`prepared`/`downloading`/`verifying`/`applying`/`complete`/`cancelled`/`failed`/`abandoned` via the phase badge), plus a distinct `recovering`/`needsConfiguration` app-level state. There is no dedicated `Offline` distinction yet — a network failure during prepare currently surfaces as a generic error message, not a typed offline state (tracked as a follow-up, consistent with the still-open "network failure vs update-needed" gap in 1.3-adjacent work).
- [x] Add a session log viewer backed by structured Rust log events (`get_recent_logs` command + live `app-log` event subscription, collapsible panel, capped client-side history).

### 5.2 User-facing screens

- [x] Build update progress, cancel, and recovery controls (progress bar, byte/speed readout, cancel button, recover-interrupted-transactions button). Retry after a failure is currently just "prepare again" (there is no one-click "retry the same plan" distinct from re-preparing) and there is no dedicated integrity-check trigger in the UI yet (the executor doesn't schedule integrity mode at all — see 1.3).
- [x] Port relevant Settings: game directory (with native picker), WDB-clear-on-launch toggle, tweaks. Essential-mods auto-install, recommended-addon auto-install, custom-speech preservation, and Defender guidance are not ported (they depend on Milestone 3.1's full registry and Milestone 4's addon port).
- [x] Port an initial Addons screen: curated GitHub addon statuses plus install/remove controls, game-directory/update-in-progress disabling, per-addon busy state, and structured backend activity logs. This is intentionally a provisional safety UI; catalog search, profiles, dependency/conflict review, health states, transaction progress/history, custom addons, and pfUI configuration move to Milestone 4.1.7.
- [ ] Add accessible focus states, keyboard navigation, clear empty/loading/error states beyond what exists today, and high-DPI checks — not audited.
- [x] Add a native directory picker (`@tauri-apps/plugin-dialog` + `tauri-plugin-dialog`, `dialog:default` capability) alongside the existing manual text field, rather than replacing manual entry (a user who already knows the path can still type/paste it).

**Acceptance criteria:** Met for update execution, tweaks, launch, and mods — each has real UI, progress/log feedback, and error surfacing. Addons, MPQ-patch-specific UI, news, accessibility auditing, and store extraction remain open.

---

## Milestone 6 — Self-update, packaging, and release

### 6.1 Updater self-update

- [ ] Select a signed/pinned release metadata format.
- [ ] Port update checking with cache TTL, version comparison, and explicit user consent.
- [ ] Use Tauri's supported updater mechanism or a separately journaled updater replacement process.
- [ ] Never self-update during an active client transaction.

### 6.2 Build and distribution

- [ ] Configure Tauri Windows bundle identity, icon, version metadata, and installer behavior.
- [ ] Bundle or securely provision aria2 with license notices.
- [ ] Generate SBOM/license attribution for Rust, Tauri, frontend, and bundled binaries.
- [ ] Add CI for Rust formatting/linting, frontend checking, builds, and release artifacts.
- [ ] Document installation, migration from Python config, backup locations, diagnostics, and recovery steps.

**Acceptance criteria:** A clean Windows machine can install, launch, update safely, recover after interruption, and report diagnostics without requiring Python or Node.js.

---

## Milestone 7 — Verification and cutover

- [x] Add unit tests for file selection, journal transitions, progress parsing, aria2 option validation, locale patching, tweak clamping/FOV defaults, version parsing, pristine-cache scoping, Config.wtf writing, launch-executable selection, realmlist healing, MPQ archive read/write (real StormLib round trips, not mocks), MPQ packaging, `dlls.txt` atomic add/remove, mod registry validity, mod install/uninstall, and the log ring buffer (58 tests total, all passing, zero compiler warnings). Bencode-validation and config-serialization (JSON) tests are still outstanding.
- [ ] Add integration tests using temporary directories and a fake aria2 executable for success, cancellation, non-zero exit, and crash-recovery cases.
- [ ] Add manual smoke-test scripts for clean install, existing client, interrupted download, integrity repair, mod upgrade failure, addon replacement failure, and game-running rejection.
- [ ] Compare Rust behavior against the Python updater feature by feature.
- [ ] Run a limited beta with backup/recovery telemetry or user-provided logs.
- [ ] Freeze Python feature work after Rust parity is accepted.
- [ ] Archive or remove the Python implementation only after a separately approved cutover.

**Final acceptance criteria:** The Rust/Tauri application has verified parity for supported features, safely recovers from expected failures, and can replace the Python updater without data-loss regressions.
