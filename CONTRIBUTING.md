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
2. Set up and run the checks in the README's [Development](README.md#development) section. Building needs Rust 1.96 or later. CI runs the same checks on Linux and macOS.
3. Keep each pull request to one change, and add a test for it.
4. Use a [conventional commit](https://www.conventionalcommits.org/) prefix for the title: `fix:`, `feat:`, `docs:`, `perf:`, `test:`, `ci:` or `chore:`.
5. Add a line under `Unreleased` in [CHANGELOG.md](CHANGELOG.md) for anything users will notice.

**Changing the guest** (`guest/`): rebuild it with `./scripts/build-guest.sh` (needs wasi-sdk 24) and commit the new `modebox_guest.wasm`. CI rebuilds it and checks that the result matches byte for byte. A new guest also stops older continuation tokens from resuming, so say so in the CHANGELOG.

## License

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
