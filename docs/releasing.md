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

If the npm job fails partway (one package's trusted publisher is misconfigured, say), fix the cause and re-run the failed jobs from the run's page. Packages already on npm at that version are skipped. Wait a few minutes first, since npm takes a while to list a version it just accepted.

## Credentials

Both registries use trusted publishing, so the repository stores no publish tokens. Every crate and npm package trusts `release.yml` in `farming-labs/modebox`, and at publish time the job's OIDC identity is exchanged for a short-lived token. A crate or package must already exist before it can trust a workflow, so a new one (a new platform package, say) is published once by hand first.

**crates.io** ([docs](https://crates.io/docs/trusted-publishing)): on `modebox-core` and `modebox`, Settings → Trusted Publishing → Add → GitHub, with owner `farming-labs`, repository `modebox`, workflow `release.yml` and no environment. The workflow gets its token from `rust-lang/crates-io-auth-action`. Once a release has gone out this way, "Require trusted publishing for all new versions" on the same page makes crates.io reject API tokens for that crate.

**npm** ([docs](https://docs.npmjs.com/trusted-publishers)): each of the five packages has a GitHub Actions trusted publisher with the same values. Its allowed actions must include publishing directly, because the workflow runs `npm publish`; with staged publishing alone, the publish fails with a 404. To set them all up from the command line, run this as a maintainer with 2FA (npm 11.15 or later; a token that bypasses 2FA is refused):

```bash
for p in modebox-darwin-arm64 modebox-darwin-x64 modebox-linux-x64-gnu modebox-linux-arm64-gnu modebox; do npm trust github "$p" --repo farming-labs/modebox --file release.yml --allow-publish --yes; sleep 2; done
```

The same setting is under each package's Settings → Trusted Publisher on npmjs.com.

## Compatibility

Continuation tokens and saved sessions only resume on the build that made them, because they're bound to the guest image. Any release that changes the guest (`guest/` or its dependencies) makes older tokens fail with "created by a different sandbox image". Say so in the CHANGELOG when it happens.
