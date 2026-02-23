# Koca
[![Crates.io](https://img.shields.io/crates/v/koca)](https://crates.io/crates/koca)
[![npm](https://img.shields.io/npm/v/%40koca-build%2Fcli)](https://www.npmjs.com/package/@koca-build/cli)
[![GHCR](https://img.shields.io/badge/ghcr-koca-blue)](https://github.com/koca-build/koca/pkgs/container/koca)

**The universal build, package, and publishing tool.**

Koca is a modern, universal, and system-native package manager designed to simplify the process of building and distributing applications across multiple platforms. It is so powerful that it even builds and packages itself!

## Why Koca?

- **Universal:** Build for any operating system—Windows, macOS, and Linux distributions (Ubuntu, Debian, Red Hat)—using a single build file.
- **System Native:** Leverage native packaging systems to output familiar formats like `.exe`, `.pkg`, `.deb`, `.rpm`, and more.
- **Lightning Fast:** Optimized for speed with a Rust-based backend, ensuring your builds are as fast as possible.
- **Developer First:** Focus on building your application, not the packaging tooling around it.

## Example: Packaging Claude Code

Koca uses a simple, bash-like syntax for its build files. Here is an example of how you can package Claude Code using Koca:

```bash
pkgname=claude-code
pkgver=2.1.50
pkgrel=1
pkgdesc='Terminal-based AI coding assistant'
arch=('x86_64')

build() {
    # Download the pre-built Claude Code binary, using the version we defined above with Bash variable syntax
    curl -L "https://storage.googleapis.com/claude-code-dist-86c565f3-f756-42ad-8dfa-d59b1c096819/claude-code-releases/${pkgver}/linux-x64/claude" -o claude
    chmod +x claude
}

package() {
    # Koca provides the $pkgdir environment variable to stage your package
    install -Dm 755 "claude" "${pkgdir}/usr/bin/claude"
}
```

To create a package from this script:

```bash
# Create a .deb and .rpm package for Claude Code
koca create claude-code.koca --output-type all
```

## Getting Started

### Installation

You can install the Koca CLI via `cargo` or the [GitHub releases](https://github.com/koca-build/koca/releases/).

```bash
# Via Cargo
cargo install koca-cli
```

### Usage

The `create` command is the primary way to build packages:

```bash
# Create a .deb package (default)
koca create your-app.koca

# Create an .rpm package
koca create your-app.koca --output-type rpm
```

## License

Koca is released under the [FSL-1.1-MIT](LICENSE) license.
