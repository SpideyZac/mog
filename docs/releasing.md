# Releasing and publishing mog

Pushing a tag like `v1.2.0` is all a release takes. GitHub Actions checks the code, builds every
platform, signs the archives, publishes the GitHub release and then updates Homebrew, the AUR,
winget and `cargo binstall`. This page covers the one time setup for each of those and what to do
for every release.

## How a release flows

`.github/workflows/release.yml` runs on every pushed tag that starts with `v`:

| job | does | waits for |
| --- | --- | --- |
| `ci` | Runs `ci.yml`: formatting, clippy, tests on Linux, macOS and Windows, the plugin SDK tests, the minimum Rust version and `cargo deny` | |
| `build` | Builds `mog` for five targets and packs each with the README, the license and `examples/` as `mog-v<version>-<target>.tar.gz` (`.zip` on Windows) with a `.sha256` next to it | `ci` |
| `release` | Writes release notes from the commits since the last tag, checks every target is there, signs every archive with minisign and publishes the GitHub release | `build` |
| `packages` | Renders the Homebrew formula, the AUR `PKGBUILD` and the winget manifests from `packaging/`, attaches them to the release and publishes each one whose secret is set | `release` |

Nothing ships if a check fails or a target is missing, because the updater and `cargo binstall`
would break for the missing platform. Tags with a `-`, like `v1.2.0-rc.1`, become pre-releases
and skip the `packages` job, so package managers only ever see real releases.

The release notes come from [conventional commits](https://www.conventionalcommits.org): `feat`
commits go under "New stuff", `fix` under "Fixes" and `perf` under "Faster". mog shows these in
its what's new popup after it updates, so write commit subjects for users.

## One time setup

Everything below is done once per repository. Secrets go in **Settings > Secrets and variables
> Actions > Secrets**, variables in the **Variables** tab next to it.

| what | secret or variable | needed for |
| --- | --- | --- |
| [Signing key](#signing-key) | `MINISIGN_KEY` | every release, the workflow refuses to publish without it |
| [Homebrew tap](#homebrew) | `HOMEBREW_TAP_TOKEN` | `brew install` |
| [AUR](#aur) | `AUR_SSH_PRIVATE_KEY`, variables `AUR_USERNAME` and `AUR_EMAIL` | `yay -S mog-bin` |
| [winget](#winget) | `WINGET_TOKEN` | `winget install SpideyZac.mog` |
| [cargo binstall](#cargo-binstall) | none | `cargo binstall mog` |

Each package job is skipped while its secret is missing, so they can be set up one at a time.

### Signing key

mog's updater and `cargo binstall` only install archives signed by a key mog trusts, so a
release uploaded by anyone else never installs even if its checksum matches.

1. Install [minisign](https://jedisct1.github.io/minisign/) and make a key pair without a
   password, since the workflow cannot type one:

   ```sh
   minisign -G -W -p mog.pub -s mog.key
   ```

2. Put the whole contents of `mog.key` in the `MINISIGN_KEY` secret.
3. Put the public key, the second line of `mog.pub`, in `RELEASE_KEYS` in
   `crates/mog/src/update.rs` and in `pubkey` under `[package.metadata.binstall.signing]` in
   `crates/mog/Cargo.toml`.
4. Keep `mog.key` somewhere safe and offline, like a password manager, and delete the local
   copy.

To rotate the key, ship one release whose `RELEASE_KEYS` has both the old and the new public
key, signed with the old key. Then switch `MINISIGN_KEY` and the binstall `pubkey` to the new
key for the next releases, and drop the old public key from `RELEASE_KEYS` a release or two
later, once most people updated.

### Homebrew

1. Create a public repository called `homebrew-mog` under the same account, with a `Formula`
   folder (an empty `Formula/.gitkeep` is enough).
2. Create a [fine grained token](https://github.com/settings/personal-access-tokens/new) with
   access to only that repository and **Contents: Read and write**.
3. Put it in the `HOMEBREW_TAP_TOKEN` secret.

Every release then commits `Formula/mog.rb` to the tap and this works:

```sh
brew install spideyzac/mog/mog
```

To check a formula by hand before trusting the job, render it (see
[Trying the packages locally](#trying-the-packages-locally)), then
`brew install --formula ./out/homebrew/mog.rb` and `brew test mog`.

### AUR

The AUR package is `mog-bin`, which installs the prebuilt Linux binary. The job uses
[`github-actions-deploy-aur`](https://github.com/KSXGitHub/github-actions-deploy-aur), which also
writes the `.SRCINFO`.

1. Make an account on [aur.archlinux.org](https://aur.archlinux.org).
2. Make an SSH key just for this, without a passphrase:

   ```sh
   ssh-keygen -t ed25519 -C "mog aur" -f aur_key -N ""
   ```

3. Paste `aur_key.pub` into **My Account > SSH Public Key** on the AUR.
4. Put the contents of `aur_key` in the `AUR_SSH_PRIVATE_KEY` secret, then delete both files.
5. Set the `AUR_USERNAME` and `AUR_EMAIL` variables to the name and email the AUR commits should
   have. They default to `SpideyZac` and `mog@users.noreply.github.com`.

The AUR makes a package the first time something is pushed to a new name, so the first release
after this creates `mog-bin`. To push it by hand instead, or to fix a broken release:

```sh
git clone ssh://aur@aur.archlinux.org/mog-bin.git
cp out/aur/PKGBUILD mog-bin/
cd mog-bin
makepkg --printsrcinfo > .SRCINFO
makepkg -si            # builds and installs it, to check it works
git add PKGBUILD .SRCINFO
git commit -m "mog 1.2.0"
git push
```

A fix to the package that does not change mog bumps `pkgrel` in the `PKGBUILD` instead of the
version.

### winget

winget packages live in [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs), and
every version is a pull request there that Microsoft's bots check and a moderator merges. The job
uses [`winget-releaser`](https://github.com/vedantmgoyal9/winget-releaser), which only updates a
package that already exists, so the first version goes in by hand.

1. Fork `microsoft/winget-pkgs` to the account that owns the token. The job pushes a branch to
   that fork and opens the pull request from it.
2. Submit the first version from a published release. With
   [`wingetcreate`](https://github.com/microsoft/winget-create):

   ```powershell
   winget install wingetcreate
   wingetcreate submit --token <token> <folder with the three rendered manifests>
   ```

   The rendered manifests are attached to every release as `SpideyZac.mog*.yaml`. Opening a pull
   request that adds them under `manifests/s/SpideyZac/mog/<version>/` works too.
3. Wait for that pull request to be merged. It can take a few days.
4. Create a [classic token](https://github.com/settings/tokens/new) with the `public_repo` scope
   and put it in the `WINGET_TOKEN` secret.

From then on every release opens a pull request on its own. To check manifests before
submitting:

```powershell
winget validate --manifest out\winget
winget settings --enable LocalManifestFiles   # once, as administrator
winget install --manifest out\winget
```

### cargo binstall

Nothing to set up. `[package.metadata.binstall]` in `crates/mog/Cargo.toml` tells
[`cargo binstall`](https://github.com/cargo-bins/cargo-binstall) where the release archives are
and which minisign key signs them:

```sh
cargo binstall --git https://github.com/SpideyZac/mog mog
```

If the crates are ever published to crates.io, `cargo binstall mog` works without `--git`.

## Making a release

1. Make sure `master` is green in Actions.
2. Bump the version in the root `Cargo.toml`: `version` under `[workspace.package]` and the
   `version` of every `mog-*` entry under `[workspace.dependencies]`. Then run `cargo build` so
   `Cargo.lock` follows.
3. Commit it as `chore: bump the version to 1.2.0` and push.
4. Tag and push the tag:

   ```sh
   git tag v1.2.0
   git push origin v1.2.0
   ```

5. Watch the `release` workflow. When it is done the GitHub release has the archives, their
   `.sha256` and `.minisig` files and the rendered packages, the tap and the AUR are updated and
   a winget pull request is open.
6. Check that `mog --update` on an older build picks the release up.

For a release candidate, tag `v1.2.0-rc.1` instead. It becomes a pre-release that only people who
download it themselves get.

## When something goes wrong

- **A check or build failed.** Nothing was published. Fix it on `master`, delete the tag
  (`git push --delete origin v1.2.0` and `git tag -d v1.2.0`) and tag again.
- **The release is out but a package job failed.** Re-run just the `packages` job from the
  workflow run page, or publish that package by hand with the files attached to the release.
- **`the MINISIGN_KEY secret is missing`.** Add the secret, see [Signing key](#signing-key).
- **The AUR push is rejected.** The public key on the AUR account does not match
  `AUR_SSH_PRIVATE_KEY`, or the package name is taken by someone else.
- **winget-releaser cannot find the package.** The first version is not merged into
  `winget-pkgs` yet, see [winget](#winget).
- **A bad release went out.** Do not move or reuse its tag, since package managers and the
  updater have already seen it. Fix the problem and release the next patch version. Mark the bad
  GitHub release as a pre-release meanwhile, which stops the updater from offering it.

## Trying the packages locally

`packaging/render.sh` fills the templates in with a version and the checksums of the archives:

```sh
# download the archives of a release into dist/ first, for example with the GitHub CLI
gh release download v1.2.0 --dir dist
packaging/render.sh 1.2.0 dist out
```

This writes `out/homebrew/mog.rb`, `out/aur/PKGBUILD` and `out/winget/*.yaml`, the same files the
`packages` job publishes. To change what a package looks like, edit the template in `packaging/`,
not the rendered file.

## Publishing a plugin

Plugins do not go through any of this. A plugin is a folder with a `plugin.toml` (see
[plugins.md](plugins.md)), so publishing one means pushing that folder to a git repository:

```sh
mog plugin install https://github.com/you/mog-vim
```

Keep the plugin's `version` in `plugin.toml` up to date and tag releases in its repository, so
people can see what changed.
