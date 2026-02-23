set shell := ["bash", "-euo", "pipefail", "-c"]

default:
	@just --list

# Usage: just bump-version 0.1.2
bump-version version:
	@if ! [[ "{{version}}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then echo "error: version must match X.Y.Z (got: {{version}})" >&2; exit 1; fi
	@sed -i -E "0,/^version = \"[^\"]+\"$/s//version = \"{{version}}\"/" Cargo.toml
	@sed -i -E "s/^koca = \{ path = \"crates\/koca\", version = \"[^\"]+\" \}$/koca = { path = \"crates\/koca\", version = \"{{version}}\" }/" Cargo.toml
	@sed -i -E "s/^pkgver=[0-9]+\.[0-9]+\.[0-9]+$/pkgver={{version}}/" koca.koca
	@sed -i -E "s/^  \"version\": \"[^\"]+\",$/  \"version\": \"{{version}}\",/" npm/cli/package.json
	@rg '^version = "{{version}}"$$' Cargo.toml >/dev/null
	@rg '^koca = \{ path = "crates/koca", version = "{{version}}" \}$$' Cargo.toml >/dev/null
	@rg '^pkgver={{version}}$$' koca.koca >/dev/null
	@rg '^  "version": "{{version}}",$$' npm/cli/package.json >/dev/null
	@echo "Updated versions to {{version}}"
