# Releasing

A release is a `vX.Y.Z` tag on `main`. Pushing it runs `.github/workflows/release.yml`, which:

1. checks that every published version matches the tag (`node scripts/version.mjs --check X.Y.Z`);
2. builds the `modebox` CLI and the Node binding natively on macOS arm64, macOS x64, Linux x64 and Linux arm64, and smoke-tests each;
3. publishes `modebox-core` and `modebox` to crates.io;
4. publishes `modebox-darwin-arm64`, `modebox-darwin-x64`, `modebox-linux-x64-gnu` and `modebox-linux-arm64-gnu` to npm, then `modebox`, with provenance;
5. creates a GitHub Release with a tarball and SHA-256 checksum per target, using that version's section of `CHANGELOG.md` as the notes.

Steps 3–5 only run for tags. To rehearse the builds, run the workflow by hand: Actions → Release → Run workflow.

## Steps

1. Set the version everywhere: `node scripts/version.mjs X.Y.Z`, then `cargo update -w` to refresh `Cargo.lock`.
2. Move the `Unreleased` notes in `CHANGELOG.md` under `## [X.Y.Z]`.
3. Open a PR with both, and merge it once CI is green.
4. Tag the merge commit and push the tag:

   ```bash
   git tag vX.Y.Z && git push origin vX.Y.Z
   ```

A version with a suffix (`0.2.0-rc.1`) is a prerelease: a prerelease on GitHub, and the `next` dist-tag on npm, so `npm install modebox` keeps getting the last stable version.

## Secrets

The workflow needs two repository secrets (Settings → Secrets and variables → Actions):

- `CARGO_REGISTRY_TOKEN`: a crates.io API token with the `publish-new` and `publish-update` scopes.
- `NPM_TOKEN`: an npm automation token for an account that can publish `modebox` and the four platform packages.

After the first release, both registries can switch to trusted publishing from this workflow, which needs no stored tokens.

## Compatibility

Continuation tokens and saved sessions only resume on the build that made them, because they're bound to the guest image. Any release that changes the guest (`guest/` or its dependencies) makes older tokens fail with "created by a different sandbox image". Say so in the CHANGELOG when it happens.
