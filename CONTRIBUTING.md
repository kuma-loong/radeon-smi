# Contributing

Issues and pull requests are welcome. Please include the GPU's PCI ID,
kernel version, driver name, the command you ran, and the observed output
when reporting a compatibility problem. Do not include private hostnames or
user data in logs.

Keep changes small and read-only. Prefer a capability check and `N/A` over a
model-specific guess. Run `cargo fmt`, `cargo clippy --all-targets -- -D
warnings`, and `cargo test` before opening a pull request. Hardware-specific
changes should be tested on the affected GPU when possible.

Contributions are licensed under Apache-2.0, the project's license.
