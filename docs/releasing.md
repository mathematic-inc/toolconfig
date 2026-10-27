# Releases

Versions are `YEAR.MONTH.MICRO`, such as `2026.10.0`: the release's year and month, without a
leading zero, and a counter that starts at 0 each month. They are valid semver, so Cargo orders
them correctly.

1. Update `version` in `Cargo.toml`, the `CHANGELOG.md` entry, and the versioned URLs in
   `README.md` and `examples/pkl`, then merge to `main`.
2. Push a `vVERSION` tag. The release workflow checks that the tag matches `Cargo.toml`.
3. The workflow builds and tests macOS and Windows on amd64 and arm64, plus GNU and musl Linux
   for both architectures. Every archive is checksum-verified, extracted and run against the
   examples on its target platform.
4. It verifies that every build produced the same Pkl package and schema, attests the assets,
   and attaches them to the GitHub release.
5. crates.io trusted publishing publishes `toolconfig` from the `release` environment with a
   short-lived OIDC token. The trusted publisher is GitHub owner `mathematic-inc`, repository
   `toolconfig`, workflow `release.yml`, environment `release`.
6. Fresh runners install the release through Cargo Binstall and mise without compiling, run it,
   and compare the bytes.

## Assets

- `toolconfig-VERSION-TARGET.tar.gz`, or `.zip` on Windows, with an adjacent `.sha256`. The
  archive holds the executable, the license, the README, the docs, the examples and
  third-party license notices.
- `toolconfig@VERSION`, `toolconfig@VERSION.zip` and `toolconfig@VERSION.sha256`: the Pkl
  package metadata, the exact embedded package bytes, and the metadata checksum. Pkl resolves
  `package://github.com/mathematic-inc/toolconfig/releases/download/vVERSION/toolconfig@VERSION`
  from them.
- `toolconfig.schema.json`: the JSON Schema that TOML sources reference with `#:schema`.

Never replace published assets under an existing version.
