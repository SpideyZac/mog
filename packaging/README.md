# Packaging

Every release renders these templates with `render.sh` and attaches the results to the GitHub
release. The `packages` job in `.github/workflows/release.yml` also publishes each one whose
secret is set, and skips the rest.

| where | template | secret | one time setup |
| --- | --- | --- | --- |
| Homebrew | `homebrew/mog.rb` | `HOMEBREW_TAP_TOKEN` | Create the repo `SpideyZac/homebrew-mog` and a token that can push to it. Then `brew install spideyzac/mog/mog` works. |
| AUR | `aur/PKGBUILD` | `AUR_SSH_PRIVATE_KEY` | Make an AUR account, add the public key to it, and push the first `mog-bin` by hand or let the job create it. Set the `AUR_USERNAME` and `AUR_EMAIL` repository variables for the commit author. |
| winget | `winget/*.yaml` | `WINGET_TOKEN` | Submit the first version to `microsoft/winget-pkgs` by hand with the manifests attached to a release (`wingetcreate submit` or a pull request). After that the job opens a pull request per release. The token is a classic token with `public_repo`. |
| cargo-binstall | `[package.metadata.binstall]` in `crates/mog/Cargo.toml` | none | Nothing, `cargo binstall --git https://github.com/SpideyZac/mog mog` already works and checks the minisign signature. |

Try a template locally:

```sh
packaging/render.sh 0.3.0 path/to/downloaded/archives out
```
