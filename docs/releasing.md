# Releasing

A version tag builds every installer, signs them for the updater, scans them, attaches them to a
draft GitHub Release (installers live on GitHub Releases) and registers the release with
api.rmgn.net, where it lands **paused at 0%**. Nobody gets it until the owner publishes the draft
and, signed in to the dashboard, activates it and raises the rollout step by step. CI can only
register; every rollout decision is a person's.

```
bump ─► PR, merge ─► tag vX.Y.Z ─► Release workflow ──────────────────────► owner
                                    ├ build: setup.exe + MSI, x64 and arm64    ├ publish the GitHub draft
                                    ├ .sig for each (updater signature)        ├ dashboard: activate at 5%
                                    ├ VirusTotal (≤ 2 engines)                 ├ watch a day
                                    ├ build provenance attestation             ├ 25%, then 100%
                                    ├ draft GitHub Release (installers,        └ pause or block if needed
                                    │   .sig, SHA256SUMS.txt)
                                    └ POST /admin/releases (paused, 0%)
```

The dashboard is <https://api.rmgn.net/dashboard/?product=unflub>, behind Cloudflare Access. Every
rollout change there produces a new signed manifest on dl.rmgn.net, which is all the app reads. The
API contract is `D:\Code\rmgn-api\docs\unflub-contract.md`.

## Versions and channels

- **SemVer, tagged `vX.Y.Z`.** Before 1.0, a new feature or a breaking change bumps the minor
  (0.2.0) and a fix bumps the patch (0.1.1).
- **Betas** are `X.Y.Z-beta.N` for the version they lead to (0.2.0-beta.1, 0.2.0-beta.2, then 0.2.0).
  They go to the `beta` channel (Settings > Privacy > Channel), are drafted as GitHub pre-releases,
  and ship the setup.exe only (WiX can't put a pre-release version in an MSI). The beta channel
  also receives stable releases, so a beta tester always ends up on the newest build.
- **Commit and PR titles are Conventional Commits.** The changelog and the release notes are built
  from them: `feat` under New, `fix` under Fixes, `perf` under Faster, `!` or a `BREAKING CHANGE`
  footer under Breaking changes; everything else is counted, not listed. So are changes to the
  release tooling whatever their type: the scopes `release`, `ci`, `build`, `deps` and `packaging`
  (`fix(release): ...` is not a fix users see).
- **Two sets of notes.** The GitHub Release gets Markdown with commit links, the
  internal-change count and install instructions (CHANGELOG.md the same, without install). The
  app's About page gets plain text (`node scripts/release-notes.mjs --app`): only New, Fixes,
  Faster and Breaking changes, without scopes, PR numbers, commit hashes or website (`site`)
  changes. The release PR and the run summary show both.
- **Releases are never rewritten.** A published version is fixed; a mistake gets the next version.

## Cut a release

1. **Prepare the release PR** from an up-to-date `main`:

   ```sh
   git switch main && git pull
   node scripts/prepare-release.mjs 0.2.0 --pr          # or 0.2.0-beta.1
   ```

   It checks the version is newer than the last tag, sets it in all five places (Cargo workspace,
   Cargo.lock, `tauri.conf.json`, `package.json`, `package-lock.json`), adds the CHANGELOG.md section,
   commits `chore(release): 0.2.0` on `release/v0.2.0` and opens the PR. Read the changelog in it.

2. **Dry run (optional, recommended before a stable release).** Actions > Release > Run workflow on
   the release branch with "dry run" ticked. It builds everything, scans with VirusTotal (if the key
   is set), writes `latest.json`, `SHA256SUMS.txt` and the registration body, and attaches them to
   the run as the `release-assets` artifact. Nothing is tagged, drafted or registered.

3. **Rebase-merge the PR.** `.github/workflows/release-tag.yml` sees
   `chore(release): 0.2.0` on `main`, checks the files agree, tags the release commit `v0.2.0` and
   starts the Release workflow on the tag. (Pushing a `v*` tag by hand starts it too.) The workflow
   refuses a tag that doesn't match the version in the files, and a tag that already exists.

4. **What the run does after the builds**, in order, stopping at the first failure:
   - The release job checks it got one `bundle-<name>` artifact per enabled matrix row and in
     each exactly the installers that row should make (the setup per arch, plus the MSI for a
     stable version), named from the version: anything missing, extra or duplicated fails the
     run before signing.
   - The release job signs each installer for the updater (`tauri signer sign --app-version`,
     Tauri's CLI from `app/package-lock.json`, installed with `--ignore-scripts`) and writes its
     `.sig`. The build jobs never get the key: they run npm and Cargo build scripts, and any of
     those could read it from the environment. `scripts/verify-updater-sig.mjs` then checks every
     `.sig` against the public key in `tauri.conf.json`, so a wrong key fails the run instead of
     shipping updates the app rejects.
   - VirusTotal scans each `*-setup.exe` and `*.msi`, and the two executables inside them on
     their own (`unflub.exe` and `unflub-engine.exe`, from the build jobs' `binaries-*`
     artifacts, never published), and fails the run if more than two engines flag one (skipped,
     with a notice, without `VT_API_KEY`). Each build job first checks that both executables
     are in both installers with the same publisher and copyright, and, when signing is on,
     that both are signed.
   - `actions/attest-build-provenance` records which workflow run built each installer
     (`gh attestation verify <file> --repo ReimagineMedias/unflub` checks it).
   - The draft GitHub Release gets the installers, their `.sig` files, `latest.json` and
     `SHA256SUMS.txt`.
   - `POST https://api.rmgn.net/unflub/v1/admin/releases` registers the release with the bearer
     `ADMIN_TOKEN` and the Cloudflare Access service token. The body comes from
     `scripts/register-release.mjs`: version, channel, the app's notes, and per platform the setup file, its
     signature, SHA-256 and size, and the installer the website serves. The API stores it
     **paused at 0%**. A re-run gets `409` and changes nothing. The API's publish step (in
     rmgn-api) writes the signed manifest, each release's `tauri-<target>-<arch>.json` and the
     redirects from `dl.rmgn.net/unflub/<version>/<file>` to the GitHub Release asset.

5. **Check the draft** (the run summary links to it): notes read well, the assets are all there,
   and the x64 setup installs, starts, shows the new version under About, and uninstalls cleanly
   (Windows Sandbox is the quickest clean machine).

6. **Publish the GitHub draft.** Draft assets can't be downloaded by anyone else, so the
   installer URLs in the manifest only work once it is published. Publishing moves
   `releases/latest` (Scoop's `checkver`) and, once enabled, opens the winget pull request; hold
   that PR until the rollout reaches 100% if you want package managers to wait for it.

7. **Roll it out from the dashboard**, signed in through Cloudflare Access:
   1. Set the release **active at 5%**.
   2. Watch for a day: the version counts in the usage stats, crash and `hook_reinstalls`
      counts, Sentry (if set up).
   3. **25%**, watch again, then **100%**.

   **Pause** stops offering a release to anyone who doesn't have it yet. **Block** also offers
   everyone on it the newest active release, older or newer (the app accepts a rollback). Setting
   **min_version** in the policy makes everything below it a critical update that ignores the
   rollout percentage.

8. **Windows Defender.** Submit each new setup to Microsoft for analysis at
   <https://www.microsoft.com/wdsi/filesubmission> ("Software developer", file type "Incorrectly
   detected as malware/malicious", or as a new release before anyone reports a false positive).
   Unsigned installers start with no reputation; a clean verdict there clears Defender's
   heuristics sooner. Attach the VirusTotal link from the run summary.

The website's download buttons go through `https://api.rmgn.net/unflub/v1/download/windows?ref=site`
(after a Turnstile check), which answers with the newest stable release that is active at 100%,
served from `dl.rmgn.net/unflub/latest/windows-x86_64`. Until one is, it falls back to the GitHub
releases page.

## Secrets and variables

Settings > Secrets and variables > Actions. The release secrets belong to the `release`
environment (Settings > Environments > release), so only that job can read them.

| Name | Kind | Needed for | Value |
|---|---|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | secret | every release | Contents of `D:\Code\unflub\.local\updater\unflub-updater.key` |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | secret | every release | Contents of `D:\Code\unflub\.local\updater\unflub-updater.password` |
| `ADMIN_TOKEN` | secret | every release | The API's CI bearer token. It can only register releases (paused, 0%) |
| `CF_ACCESS_CLIENT_ID`, `CF_ACCESS_CLIENT_SECRET` | secrets | every release | Cloudflare Access service token allowed through to `/unflub/v1/admin` |
| `SENTRY_DSN` | secret | crash reports, optional | The Sentry project's DSN. Compiled into the app; without it crash reports are off |
| `SENTRY_AUTH_TOKEN` | secret | crash symbols, optional | Sentry token with `project:releases` scope, for `sentry-cli debug-files upload` |
| `SENTRY_ORG`, `SENTRY_PROJECT` | variables | crash symbols, optional | Sentry organisation and project slugs |
| `VT_API_KEY` | secret | antivirus scan, optional | A VirusTotal API key |
| `PUBLISH_PACKAGES` | variable | winget | `true` turns on `publish-packages.yml`. Anything else, or unset, keeps it off. |
| `WINGET_TOKEN` | secret | winget | Classic personal access token with `public_repo` and `workflow` scopes (fine-grained tokens don't work with the action) |
| `WINGET_FORK_USER` | variable | winget, optional | Owner of the `winget-pkgs` fork, if not `ReimagineMedias` |
| `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET` | secrets | code signing, optional | Azure app registration, see below |
| `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT`, `AZURE_SIGNING_PROFILE` | variables | code signing, optional | Artifact Signing account details, see below |
| `WINDOWS_CERTIFICATE`, `WINDOWS_CERTIFICATE_PASSWORD` | secrets | code signing, optional | A `.pfx` as one line of base64, and its password |

Set secrets from files, so the values never pass through the clipboard or shell history
(PowerShell):

```powershell
Get-Content -Raw D:\Code\unflub\.local\updater\unflub-updater.key | gh secret set TAURI_SIGNING_PRIVATE_KEY --env release
Get-Content -Raw D:\Code\unflub\.local\updater\unflub-updater.password | gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --env release
```

The build jobs run outside the `release` environment, so they get none of its secrets. Keep the
updater key, API, Access and VirusTotal secrets there: only the release job reads them, and only
its "Updater signatures" step sees the updater key. The `SENTRY_*` secrets the build jobs read
have to be repository secrets.

Code signing secrets (`AZURE_*`, `WINDOWS_CERTIFICATE*`) must not go to the build jobs either.
None are set today. Before adding them, see "Keep the signing keys out of the build" under
[Windows code signing](#windows-code-signing).

> [!WARNING]
> Back up both updater files (a password manager is fine). If the private key is lost, every
> installed copy stops accepting updates and has to be reinstalled by hand. The matching public
> key is in `app/src-tauri/tauri.conf.json` (`plugins.updater.pubkey`).

## How the app updates

- Everything is static. `app/src-tauri/src/updates.rs` drives `tauri-plugin-updater` from Rust.
  About two minutes after launch, then every 24 hours plus up to 90 random minutes, and on
  **Check now**, it fetches the signed manifest
  `https://dl.rmgn.net/unflub/manifest/{stable|beta}.json` and its `.json.sig` (with
  `If-None-Match`). Never more often than the manifest's `min_check_interval_h` flag (1 h
  without one).
- The manifest is believed only with a valid Ed25519 signature from a key in
  `app/src-tauri/src/keys.rs`, at most 256 KB, and not older (`generated_at`) than the one
  already cached, and its `expires_at` (7 days, re-signed daily) must be in the future; otherwise
  the last good one stays (`manifest.rs`, `remote.rs`). A signature mismatch is retried once with
  `?t=<unix minute>` on both URLs (the two files are cached apart). An expired cached manifest
  offers no updates; its flags, which can only switch things off, still apply.
- The app decides itself, with the same rules the server used to apply: kill switch
  (`updates_enabled`); a `blocked` version or one below `min_version` gets the newest release
  with `rollout_percent > 0`, even an older one (shown as an important update); otherwise the
  newest newer release whose rollout is above the app's bucket. The bucket (0 to 99, drawn again
  for every installed version) never leaves the computer. Requests carry only
  `User-Agent: unflub/<version>`.
- For the chosen release only, the updater reads
  `https://dl.rmgn.net/unflub/{version}/tauri-{target}-{arch}.json` (Tauri's static format) and
  accepts exactly that version, so a rollback works. Its download URL and signature must be the
  ones in the signed manifest, and the URL must be a GitHub Release asset of this repository or a
  dl.rmgn.net address. Redirects are followed only to that GitHub path and GitHub's asset hosts.
- With **Automatic updates** on (the default), the update downloads in the background, is
  verified against the updater public key and against the SHA-256 and size in the manifest, and
  waits on disk in `%LOCALAPPDATA%\ca.rmgn.unflub\updates` (the profile folder's ACL: the user,
  SYSTEM and Administrators). Its SHA-256 is kept in memory and checked again, with the
  signature, right before it installs. The tray says **Restart to update to vX**, and About shows
  the notes with **Restart now** and **Later**. It installs on Restart now or when unflub quits
  from the tray, never on its own. With the switch off, unflub only tells you.
- Only copies installed by the setup update themselves. winget, Chocolatey, Scoop, Store and
  portable copies show how to update with their tool (`install-source` beside `unflub.exe`, see
  `app/src-tauri/src/install.rs`).
- The `DisableUpdates` machine policy (`HKLM\Software\Policies\RMGN\unflub`) turns all of this
  off. `DisableRemoteConfig` ignores the manifest's flags and message.
- Dev builds never check in the background and refuse to install.
- `latest.json` is still attached to the GitHub Release for transparency; the app doesn't read it.

Build installers with updater signatures locally (PowerShell, not Git Bash):

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = Get-Content -Raw D:\Code\unflub\.local\updater\unflub-updater.key
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = Get-Content -Raw D:\Code\unflub\.local\updater\unflub-updater.password
cd app; npm run tauri build -- --config src-tauri/tauri.release.conf.json
```

Without `--config src-tauri/tauri.release.conf.json`, `npm run tauri build` makes plain
installers and needs no key.

## Crash reports

With `SENTRY_DSN` set, release builds can send crash reports (`app/src-tauri/src/crash.rs`). A
panic writes one scrubbed report to `crash-pending.json`; it is sent at the next start if the
user's switch is on, or after they answer the prompt. With `SENTRY_AUTH_TOKEN`, `SENTRY_ORG` and
`SENTRY_PROJECT` set, the build keeps line tables in a separate PDB (`CARGO_PROFILE_RELEASE_DEBUG=
line-tables-only`, strip off; the exe doesn't grow) and uploads it with `@sentry/cli` 3.8.2, so
stacks show function names.

## Workflow security

- Every action in `.github/workflows/` is pinned to a full commit SHA, with its version as a
  trailing comment (`uses: actions/checkout@<sha> # v7.0.1`). A moved or hijacked tag can't change
  what runs next to the signing keys. `dtolnay/rust-toolchain` has no release tags, so it is pinned
  to its `stable` branch commit with `toolchain: stable` set explicitly. Command-line tools are
  pinned by version: wrangler in `app/package-lock.json` (`npx --no-install`), `@sentry/cli@3.8.2`.
- `.github/dependabot.yml` checks weekly and opens PRs that bump each SHA and its comment
  together, plus Cargo and npm (`/app`, `/site`) dependencies. Review action bumps like code.
- To pin a new action, resolve the tag to a commit (an annotated tag points at a tag object first):

  ```sh
  gh api repos/OWNER/REPO/git/ref/tags/TAG --jq '.object.type + " " + .object.sha'
  # if the type is "tag":
  gh api repos/OWNER/REPO/git/tags/SHA --jq .object.sha
  ```

- The updater signing key is in the environment of one step: "Updater signatures" in the release
  job, which runs no build scripts. The build jobs have no environment and never see it.
- Permissions are least privilege: every workflow defaults to `contents: read`. Only the release
  job gets more: `contents: write` for the draft, `id-token: write` and `attestations: write` for
  provenance. It runs in the `release` environment. The CI's API token can only register a
  release, paused; activating, rollout, pause, block and policy need a person signed in through
  Cloudflare Access. The API headers are written to a private temp file for `curl -H @file`, so
  no secret appears on a command line. `publish-packages.yml` uses its own `WINGET_TOKEN` for the
  winget-pkgs fork. That action installs `komac` through `cargo-bins/cargo-binstall@main`
  internally, which we can't pin from here; it never sees the updater or code signing secrets.

## Windows code signing

The installers are not code-signed yet. What that means for users:

- **SmartScreen.** Downloading or opening the setup shows "Windows protected your PC" with
  publisher "Unknown". Users have to choose **More info**, then **Run anyway**. For unsigned
  files SmartScreen reputation belongs to each file, so every release starts again from zero.
  With a certificate, reputation builds up for the publisher and carries over between releases.
- **Smart App Control** (on by default on some clean Windows 11 installs) can block unsigned
  apps outright, with no "Run anyway".
- The MSI's elevation prompt shows "Unknown publisher".
- Updates are not affected: the updater checks its own signature, not Authenticode.

The release workflow signs automatically as soon as one of these is configured. The run summary
says which ("Windows code signing: ..."), and a step checks every `.exe` and `.msi` with
`Get-AuthenticodeSignature`.

### Keep the signing keys out of the build

None of the code signing secrets are set today. Don't add them as they are wired now: the build
job reads them, and its "Build installers" step hands them to `tauri build` (through
`signCommand`), the same process that runs npm and Cargo build scripts. Any of those scripts
could read a client secret or a certificate password from the environment, which is what #122
fixed for the updater key. Do it the way the updater key works instead:

- Keep the secrets in an environment (the `release` one, or a separate signing environment with
  required reviewers), never as repository secrets.
- Sign in a separate step or job that runs no build scripts: the build uploads unsigned files,
  then a signing job on a fresh runner downloads them, signs them and uploads the result. The
  updater signatures must be made after that, from the signed files (the release job already
  does this if it gets the signed files under the same artifact names).
- The exe inside each installer needs signing before it is bundled, which a separate job can't
  do in one pass. Either accept an unsigned inner exe at first, or split the build in two
  (build the exe, sign it in its own job, then bundle without build scripts).

Options 1 and 3 below describe the current `signCommand` wiring; change the workflow as above
before turning either on.

### Option 1: Azure Artifact Signing (recommended)

Formerly "Trusted Signing". Microsoft-run, the certificate names RMGN, a monthly subscription
(check current pricing and which countries are eligible for individual or organisation
validation). It plugs into Tauri's `signCommand`, so the app exe, the installers and the updater
signatures are all made in one build, in the right order.

1. In an Azure subscription, create an **Artifact Signing account**. Note its region endpoint
   (for example `https://eus.codesigning.azure.net`) and account name.
2. Under **Identity validation**, request a Public Trust validation for RMGN. This takes days.
3. Create a **Public Trust certificate profile** on the validated identity. Note its name.
4. In Microsoft Entra ID, create an **app registration** and a client secret. On the signing
   account, give that app the **Certificate Profile Signer** role.
5. Add the secrets `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET` and the variables
   `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT`, `AZURE_SIGNING_PROFILE` to the signing
   environment, once the workflow signs outside the build (see above).
6. Run the Release workflow as a dry run and check the summary and the signature check step.

The workflow installs [`artifact-signing-cli`](https://github.com/Levminer/artifact-signing-cli)
(pinned in `release.yml`) and passes it to Tauri as `bundle.windows.signCommand`.

### Option 2: SignPath Foundation (free for open source)

SignPath signs open-source projects for free. The certificate is issued to "SignPath
Foundation", which is the publisher name users see, not RMGN. Requirements include an OSI
license (MIT qualifies), a public repository, builds on a hosted CI such as GitHub Actions, and a
review of the project.

1. Apply at signpath.org once the repository is public. You get an organisation ID, a project
   slug and a signing policy, and you add an API token as a secret (`SIGNPATH_API_TOKEN`).
2. SignPath signs files after the build, so the release workflow changes shape:
   - `build`: upload the unsigned `*-setup.exe` and `*.msi` with `actions/upload-artifact`.
   - Submit them with `signpath/github-action-submit-signing-request@v2`
     (`wait-for-completion: true`, `output-artifact-directory: dist`).
   - **Updater signatures**: signing changes the files, so the `.sig` must be made from the
     signed file. The release job already signs whatever the `bundle-*` artifacts hold, so
     upload the SignPath output under those names and nothing else changes.
3. The exe inside the NSIS setup stays unsigned unless the SignPath policy also signs it before
   bundling (a two-pass build). Ask SignPath which they support for Tauri.

### Option 3: a certificate from a CA

Since June 2023, code signing keys must live on a hardware token or a cloud HSM, so a `.pfx`
file is only possible for older certificates. If you have one, export it as one line of base64
and add `WINDOWS_CERTIFICATE` and `WINDOWS_CERTIFICATE_PASSWORD`:

```powershell
[Convert]::ToBase64String([IO.File]::ReadAllBytes("unflub.pfx")) | gh secret set WINDOWS_CERTIFICATE
```

The workflow imports it and gives Tauri the thumbprint (SHA-256, DigiCert timestamp). For a key
in Azure Key Vault, Tauri's docs use `relic` as the `signCommand` instead.

## Packages

All three manifests are templates in `packaging/` at version 0.1.0 with placeholder hashes (all
zeros). Never submit those: publishing a release runs `.github/workflows/publish-packages.yml`,
which checks the release is public and stable with both setups and `SHA256SUMS.txt`, then writes
every file with the version and real hashes (`scripts/package-manifests.mjs`, which fails on a
missing hash) to the run's `package-manifests` artifact. Their installer URLs are the GitHub
Release assets, `https://github.com/ReimagineMedias/unflub/releases/download/v{version}/{file}`
(stable URLs, which winget prefers). Package managers download those directly, whatever the
app's rollout percentage says.

Each one tells unflub where it came from, and none of those copies updates itself: winget passes
`/SOURCE=winget` to the setup and Chocolatey `/SOURCE=chocolatey`, and Scoop writes
`install-source` itself. About then says "Update with winget/Chocolatey/Scoop" (see "How the app
updates"). Only a copy from the setup (or an MSI) updates itself.

### winget

The first version is submitted by hand; after that `publish-packages.yml` opens a pull request
for every published release, once enabled.

1. After v0.1.0 is published, download the Publish packages run's `package-manifests` artifact
   and use its `winget\0.1.0` folder (real hashes) below.
2. `winget validate --manifest winget\0.1.0` (no warnings once the hashes differ).
3. Test in Windows Sandbox: `winget install --manifest winget\0.1.0`, then
   `winget uninstall RMGN.unflub`. About must say "Update with winget".
4. Submit with `wingetcreate submit winget\0.1.0`, or open a PR to
   `microsoft/winget-pkgs` adding the three files under `manifests/r/RMGN/unflub/0.1.0/`.
5. Once it is merged: fork `microsoft/winget-pkgs` to the `ReimagineMedias` account, add
   `WINGET_TOKEN`, and set the variable `PUBLISH_PACKAGES` to `true`.

From then on, publishing a release opens the winget PR, with the GitHub Release URLs. Re-run it
for a tag with Actions >
Publish packages > Run workflow. The action ([`winget-releaser`](https://github.com/vedantmgoyal9/winget-releaser),
pinned to a commit) copies the previous manifest and updates the URLs and hashes.

### Scoop

Scoop installs from a bucket, which is a Git repository of manifests.

1. Create `ReimagineMedias/scoop-bucket` with `bucket/unflub.json` from
   `packaging/scoop/unflub.json`, hashes filled in.
2. Users install with:

   ```powershell
   scoop bucket add rmgn https://github.com/ReimagineMedias/scoop-bucket
   scoop install rmgn/unflub
   ```

3. Updates: the manifest's `checkver` reads the latest GitHub release and `autoupdate` takes the
   hash from `SHA256SUMS.txt`. Add Scoop's Excavator workflow (from `ScoopInstaller/GithubActions`)
   to the bucket to bump it on a schedule.

Scoop extracts the setup instead of running it. The manifest writes `install-source` with
`scoop`, so unflub never updates itself there and About says "Update with Scoop". Its
uninstaller removes start at login. Extraction skips the setup's WebView2 bootstrap, so the manifest
notes the runtime (built into Windows 11; Windows 10 may need it), and `pre_install` fails if
`unflub.exe` or `unflub-engine.exe` is missing from the extracted files.

### Chocolatey

1. Take the `chocolatey` folder from the `package-manifests` artifact (real hash filled in).
2. `choco pack chocolatey\unflub.nuspec`
3. `choco push unflub.0.1.0.nupkg --source https://push.chocolatey.org/ --api-key <key>`; the
   community repository moderates new packages by hand.

The package runs the per-user setup, so unflub lands in the profile of the account that runs
`choco` (with UAC "over the shoulder" elevation, that is the admin account).

## Uninstalling

What each uninstall removes. "Delete the application data" is the checkbox on the NSIS
uninstaller's confirm page; it is off by default.

| What | Where | setup.exe uninstall | MSI uninstall |
|---|---|---|---|
| App | `%LOCALAPPDATA%\unflub` (MSI: `Program Files\unflub`) | removed | removed |
| Start menu shortcut | | removed | removed |
| Start at login | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\unflub` | removed | removed for the account that uninstalls only |
| Task Manager's startup switch | `HKCU\...\Explorer\StartupApproved\Run\unflub` | removed | removed for the account that uninstalls only |
| Installer bookkeeping | `HKCU\Software\Reimagine Media (RMGN)\unflub` (from the publisher name) | removed | n/a |
| Settings, privacy switches, crash report waiting | `%APPDATA%\ca.rmgn.unflub` (roams) | with the checkbox | kept |
| Stats, undo counts, usage tally, install facts, rollout bucket, last update check, cached manifest, downloaded update, web view cache | `%LOCALAPPDATA%\ca.rmgn.unflub` (this PC only; moved there from `%APPDATA%` on first start since 0.2) | with the checkbox | kept |

Updates (the updater runs the setup with `/UPDATE`) keep everything. The start-at-login and
bookkeeping rows come from `app/src-tauri/windows/installer-hooks.nsh` (setup.exe) and
`app/src-tauri/windows/uninstall-cleanup.wxs` (MSI); Tauri's own template handles the rest. A
silent uninstall (`uninstall.exe /S`, winget, Chocolatey) keeps the settings.

To remove what is left afterwards (PowerShell):

```powershell
Remove-Item "$env:APPDATA\ca.rmgn.unflub", "$env:LOCALAPPDATA\ca.rmgn.unflub" -Recurse -Force -ErrorAction SilentlyContinue
Remove-ItemProperty HKCU:\Software\Microsoft\Windows\CurrentVersion\Run -Name unflub -ErrorAction SilentlyContinue
Remove-ItemProperty HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run -Name unflub -ErrorAction SilentlyContinue
```

**The MSI only cleans up the account that uninstalls it.** It installs for every account, but
each account that turns on start at login gets its own `Run` entry, which unflub writes to that
account's `HKCU`. An uninstaller can't safely reach other accounts' `HKCU` (a signed-out
account's is not even loaded), so on a shared PC their entries stay behind and point at the
deleted `unflub.exe` (Task Manager can still list unflub under Startup apps). Each of those
accounts removes them with the last two commands above.
Reinstalling the MSI makes the entries work again, since the path is the same. Fixing this for
good means MSI installs start at login from `HKLM\...\Run`, written by the MSI, instead of from
each account's `HKCU`: an app change, tracked in #128.

**Start at login follows Windows.** The toggle in Settings > General reads the `Run` entry and
Task Manager's switch each time the window opens and at startup, so turning unflub off in Task
Manager, or removing the entry, shows there too. unflub never writes the entry back on its own;
only the toggle does. Only an installed copy registers itself (the setup's
`%LOCALAPPDATA%\unflub`, the MSI's Program Files folder, or Scoop's `apps\unflub\current`); a
portable copy or a build run from a checkout shows "Start at login is available in the installed
version" and never touches the registry.

## Rollback

- **Registered, not rolled out:** nothing to do; a paused release at 0% reaches nobody. Leave
  it paused, or block it in the dashboard. Delete the GitHub draft and the tag if they were
  wrong: `gh release delete v0.2.0 --cleanup-tag`.
- **Rolling out and broken:** **pause** it in the dashboard. Copies that haven't updated stay
  where they are.
- **Copies already on it need to go back:** **block** it. Every copy on a blocked version is
  offered the newest active release on its channel at its next check, even an older one, and
  installs it like any update (with automatic updates on, at the next restart or quit). Ship the
  fix as the next version the normal way.
- **Everything below a version is bad:** set `min_version` in the policy. Older copies get the
  newest active release as a critical update, ignoring the rollout percentage.
- **Stop all updates:** turn off `updates_enabled` (the kill switch). Every check gets "no
  update".
- **winget:** close the automatic PR if it hasn't merged. If it has, the next version replaces
  it; ask the winget maintainers to remove a version only if it is harmful.
- Never replace the assets of a published GitHub Release: the manifest's SHA-256, the updater
  signatures, winget, Scoop and `SHA256SUMS.txt` all pin them.
- **Updater key leaked:** generate a new pair (`npx tauri signer generate -w <file>`), put the new
  public key in `tauri.conf.json`, and ship that version signed with the *old* key. Releases after
  it are signed with the new key.
