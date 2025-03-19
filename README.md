# Koca
A modern, universal, and system-native package manager.

If you're reading this, you're probably a developer working on the Koca codebase.

**Let's make some damn packages!**

## Project Layout
The codebase is split into these main folders:
- `cmd/`: This contains the logic to run the `koca` CLI utility
- `internal/`: Internal utilities used by the Koca CLI, public library, etc.
- `pkg/`: Public-facing library for the Koca packaging tool.