# Progress

## Current state

This worktree is mid-flight. The biggest active area is package-manager progress and download handling across:

- Debian/Ubuntu APT backend
- Arch pacman backend
- CLI/TUI progress rendering

The repo is dirty well beyond just this work. Do not assume every modified file is part of one clean feature branch.

## What was done

### APT backend

- Replaced the old pure `APT::Status-Fd` download path with a manual downloader split into:
  - `crates/backends/apt/src/handler.rs`
  - `crates/backends/apt/src/download.rs`
- Kept streamed install/remove on spawned `apt-get` with `APT::Status-Fd` and `Dpkg::Use-Pty=0`.
- Added manual APT download support for:
  - `apt-get --print-uris` parsing
  - staging downloads into `/var/cache/apt/archives/partial`
  - moving completed files into `/var/cache/apt/archives`
  - hash verification with `md5` / `sha1` / `sha256`
- Added fallback metadata parsing from `apt-cache show --no-all-versions` for cases where `apt-get --print-uris` omits the hash.
- Added parser tests for:
  - `apt-get -s` `Inst ...` lines
  - `apt-get --print-uris` lines with hash
  - `apt-get --print-uris` lines without hash

### Arch backend

- Fixed the fake `Download::Start { total_bytes: 0 }` startup behavior.
- Added an immediate progress event when a download becomes active.
- Added a progress emit right before `ItemDone` so fast downloads do not stay visually stuck at `0%`.

### CLI/TUI

- Current TUI path is the simplified `ui.rs` implementation, not the older render/viewport split.
- Confirm prompt transitions immediately into download UI.
- Download and install/remove phases are event-driven rather than the older 80ms batch-drain behavior.

### Test fixtures / containers

- `valgrind-dep.koca` was reduced from the huge `gnome-shell` case back to a moderate test:
  - `clang`
  - `gcc`
  - `valgrind`
- Refreshed containers at various points:
  - `koca-trixie`
  - `koca-deb12`
  - `koca-ubu2204`
  - `koca-ubu2404`
  - `koca-arch`

## Verified

### APT local checks

Passed:

- `cargo build -p koca-backend-apt`
- `cargo test -p koca-backend-apt`
- later also `cargo build -p cli -p koca-backend-apt`

APT unit tests currently cover parser behavior only.

### Arch local checks

Passed:

- `cargo build -p koca-backend-pacman`
- `cargo build -p cli -p koca-backend-pacman`

Pacman backend has no meaningful unit coverage right now.

## Known problems

### APT is still not working live

Live Debian container runs in `koca-trixie` still fail.

Observed failures in sequence:

1. `apt-get --print-uris` omitted hashes for some packages.
2. Added `apt-cache show` fallback.
3. Initially called `apt-get show` by mistake; fixed to `apt-cache show`.
4. Current failure:
   - `missing fallback metadata for libpng16-16t64_1.6.48-1+deb13u4_amd64.deb`

Most likely root cause:

- fallback metadata lookup is still not complete for transitive dependencies / filename matching
- `DownloadItem.filename` from `--print-uris` and `Filename:` from `apt-cache show` need to be matched more robustly across the whole resolved dependency set

Practical status:

- APT code builds and parser tests pass
- live Debian install flow is still broken

### Arch still had a UI handoff issue

User observed:

- progress stuck at `0%` for a while
- later progress eventually finished
- after download completion, screen went blank before install output

The `0%` issue was patched by forcing progress emission before `ItemDone`.

The blank-post-download issue is likely in the download/install transition:

- `crates/cli/src/tui/ui.rs`
- `crates/backends/alpm/src/handler.rs` log tail / `DownloadEvent::Done` -> `InstallEvent::Start`

The likely failure mode is:

- download block gets cleared on `DownloadEvent::Done`
- install start/log events arrive later or are not visible immediately

This was not fully re-verified after the latest Arch patch.

## Important changed files

- `Cargo.toml`
- `Cargo.lock`
- `crates/backends/apt/Cargo.toml`
- `crates/backends/apt/src/main.rs`
- `crates/backends/apt/src/handler.rs`
- `crates/backends/apt/src/download.rs`
- `crates/backends/alpm/src/handler.rs`
- `crates/cli/src/tui/ui.rs`
- `valgrind-dep.koca`

There are many other modified files in the worktree unrelated or only partially related to this effort. Check `git status --short` before assuming scope.

## Next steps

### APT

1. Fix fallback metadata resolution for transitive dependencies.
2. Re-test live in `koca-trixie`.
3. If that works, rerun in:
   - `koca-deb12`
   - `koca-ubu2204`
   - `koca-ubu2404`
4. Confirm:
   - download starts immediately after confirm
   - active package names appear
   - bytes/percent move during download
   - handoff into install works

### Arch

1. Re-test `koca-arch` after the latest byte-progress patch.
2. Check whether the `0%` startup issue is actually gone.
3. Check the blank screen after `DownloadEvent::Done`.
4. If blank remains, instrument:
   - `DownloadEvent::Done`
   - `InstallEvent::Start`
   - first pacman log-derived install event

## Useful test commands

### Debian / Ubuntu

```bash
docker exec -it koca-trixie bash -ci "cd /tmp && koca create test.koca --output-type deb"
```

```bash
docker exec -it koca-deb12 bash -ci "cd /tmp && koca create test.koca --output-type deb"
```

```bash
docker exec -it koca-ubu2204 bash -ci "cd /tmp && koca create test.koca --output-type deb"
```

```bash
docker exec -it koca-ubu2404 bash -ci "cd /tmp && koca create test.koca --output-type deb"
```

### Arch

```bash
docker exec -it koca-arch bash -ci "cd /tmp && koca create test.koca --output-type deb"
```

## Notes

- The user explicitly wanted small, isolated patches and a DRY downloader split.
- The APT downloader logic was intentionally moved into `crates/backends/apt/src/download.rs` to keep `handler.rs` from becoming a monolith.
- Do not trust the current worktree as “done”; treat it as an active handoff state.
