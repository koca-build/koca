set positional-arguments

build *ARGS:
    cargo build "${@}"

check *ARGS:
    cargo check "${@}"

clippy *ARGS:
    cargo check "${@}"

fmt *ARGS:
    cargo +nightly fmt "${@}"

doc *ARGS:
    RUSTDOCFLAGS='--cfg docsrs' cargo +nightly doc "${@}"

# vim: set sw=4 expandtab:
