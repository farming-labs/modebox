# Contributing to modebox

Thanks for helping. Bug reports, fixes, docs and new integrations are all welcome.

## Reporting a bug

Open an issue with the bug report form. It asks for:
- your OS and CPU (for example macOS arm64, or Linux x64 with its distribution);
- the modebox version, from `modebox --version`, `npm ls modebox` or `pip show modebox`;
- how you run it: the CLI, an MCP server in a harness (which one), or the Node, Python or Rust API;
- the smallest script and `modebox.json` that show the problem;
- what you expected, and what happened instead, with the full error.

If modebox doesn't work in a harness, run `modebox -c modebox.json types` in the same folder and include its output. [docs/integrations.md](docs/integrations.md) covers the usual causes.

## Reporting a security problem

Don't open a public issue. [SECURITY.md](SECURITY.md) says how to report privately.

## Making a change

1. For anything larger than a small fix, open an issue first, so we can agree on the approach before you write it.
2. Set up and run the checks under [Development](#development) below. Building needs Rust 1.96 or later. CI runs the same checks on Linux and macOS.
3. Keep each pull request to one change, and add a test for it.
4. Use a [conventional commit](https://www.conventionalcommits.org/) prefix for the title: `fix:`, `feat:`, `docs:`, `perf:`, `test:`, `ci:` or `chore:`.
5. Add a line under `Unreleased` in [CHANGELOG.md](CHANGELOG.md) for anything users will notice.

**Changing the guest** (`guest/`): rebuild it with `./scripts/build-guest.sh` (needs wasi-sdk 24) and commit the new `modebox_guest.wasm`. CI rebuilds it and checks that the result matches byte for byte. A new guest also stops older continuation tokens from resuming, so say so in the CHANGELOG.

## Development

Build from source:

```bash
cargo build --release -p modebox      # ./target/release/modebox
pip install ./crates/modebox-python   # the Python API (needs Rust 1.96+)
./scripts/build-guest.sh              # only if you change guest/: rebuilds the QuickJS wasm (needs wasi-sdk 24)
```

The layout:

```
guest/                 QuickJS guest (wasm32-unknown-unknown): event loop, codec, prelude
crates/modebox-core    engine, run loop, snapshots, sessions, tools, TS, d.ts, shell
crates/modebox-cli     the `modebox` crate and binary: run / resume / inspect / sh / types / mcp
crates/modebox-node    napi-rs binding; packages/modebox is the JS API
crates/modebox-python  PyO3 binding; python/modebox is the Python API
bench/                 compare.mjs (Run vs modebox), results
tests/fixtures         notes-mcp.mjs (downstream MCP server), drive-mcp.mjs (harness simulator)
```

The checks:

```bash
cargo test --release --workspace                  # release: some tests assert timings
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release -p modebox -p modebox-node -p modebox-python
cp target/release/libmodebox_node.dylib packages/modebox/modebox.node   # .so on Linux
(cd packages/modebox && npm test)
cp target/release/lib_native.dylib crates/modebox-python/python/modebox/_native.abi3.so   # .so on Linux
(cd crates/modebox-python && PYTHONPATH=python python3 -m unittest discover -s tests)
node tests/fixtures/drive-mcp.mjs legacy|elicit|mrtr|deny
node bench/compare.mjs run && node bench/compare.mjs modebox
```

CI (`.github/workflows/ci.yml`) runs these on Linux and macOS (the Python tests on Python 3.10, the oldest supported), checks the workspace builds on Rust 1.96 (the minimum), fails on RustSec advisories against any dependency, and rebuilds the guest to confirm the committed wasm matches byte for byte.

Releases are cut by pushing a `vX.Y.Z` tag; see [docs/releasing.md](docs/releasing.md).

## License

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
