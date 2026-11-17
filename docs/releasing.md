# Releasing

A version tag builds every installer, signs them for the updater and opens a **draft** GitHub
Release. Nothing reaches users until someone reviews the draft and publishes it. Publishing is
also what moves the updater, and (once enabled) what opens the winget pull request.

```
bump version ─► PR, merge ─► tag vX.Y.Z ─► Release workflow ─► draft release ─► review ─► publish
                                              │                                          │
                                              ├ setup.exe + MSI, x64 and arm64           ├ updater sees it within a day
                                              ├ .sig for each (updater)                  └ winget PR (if enabled)
                                              ├ latest.json, SHA256SUMS.txt
                                              └ notes from Conventional Commits
```

## Cut a release

1. **Bump the version** in all five places it is written (Cargo workspace, Cargo.lock,
   `tauri.conf.json`, `package.json`, `package-lock.json`):

   ```sh
   git switch main && git pull
   git switch -c chore/release-0.2.0
   node scripts/bump-version.mjs 0.2.0
   node scripts/bump-version.mjs --check     # all files agree
   git commit -am "chore(release): 0.2.0"
   ```

   Open the PR, and merge it (squash) once it is approved.

2. **Dry run (optional, recommended for the first few releases).** Actions > Release > Run
   workflow on `main` with "dry run" ticked. It builds everything, writes `latest.json` and
   `SHA256SUMS.txt`, and attaches them to the run as the `release-assets` artifact without
   touching any release.

3. **Tag** the merged commit:

   ```sh
   git switch main && git pull
   git tag v0.2.0
   git push origin v0.2.0
   ```

   The workflow refuses a tag that doesn't match the version in the files. A version with a
   hyphen (`0.3.0-beta.1`) becomes a pre-release, which the updater and winget ignore.

4. **Review the draft** (the run summary links to it). Check that:
   - the notes read well (edit them on GitHub if not; they come from commit subjects),
   - the assets are there: `unflub_0.2.0_x64-setup.exe`, `unflub_0.2.0_arm64-setup.exe`, both
     MSIs, a `.sig` for each, `latest.json` and `SHA256SUMS.txt`,
   - `latest.json` has `windows-x86_64`, `windows-x86_64-nsis`, `windows-x86_64-msi` and the
     same three for `windows-aarch64`,
   - the x64 setup installs, starts, shows the new version under About, and uninstalls cleanly
     (Windows Sandbox is the quickest clean machine).

5. **Publish** the draft. From that moment `releases/latest/download/latest.json` points at it,
   and installed copies pick it up at their next daily check.

> [!IMPORTANT]
> The updater endpoint and every download link need anonymous access, so the repository has to
> be public before v0.1.0 is published. While it is private, update checks report an error on the
> About page and nothing else breaks.

## Secrets and variables

Settings > Secrets and variables > Actions. Only the first two are needed to release.

| Name | Kind | Needed for | Value |
|---|---|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | secret | every release | Contents of `D:\Code\unflub\.local\updater\unflub-updater.key` |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | secret | every release | Contents of `D:\Code\unflub\.local\updater\unflub-updater.password` |
| `PUBLISH_PACKAGES` | variable | winget | `true` turns on `publish-packages.yml`. Anything else, or unset, keeps it off. |
| `WINGET_TOKEN` | secret | winget | Classic personal access token with `public_repo` and `workflow` scopes (fine-grained tokens don't work with the action) |
| `WINGET_FORK_USER` | variable | winget, optional | Owner of the `winget-pkgs` fork, if not `ReimagineMedias` |
| `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET` | secrets | code signing, optional | Azure app registration, see below |
| `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT`, `AZURE_SIGNING_PROFILE` | variables | code signing, optional | Artifact Signing account details, see below |
| `WINDOWS_CERTIFICATE`, `WINDOWS_CERTIFICATE_PASSWORD` | secrets | code signing, optional | A `.pfx` as one line of base64, and its password |

Set the updater secrets from the files, so the values never pass through the clipboard or shell
history (PowerShell):

```powershell
Get-Content -Raw D:\Code\unflub\.local\updater\unflub-updater.key | gh secret set TAURI_SIGNING_PRIVATE_KEY
Get-Content -Raw D:\Code\unflub\.local\updater\unflub-updater.password | gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD
```

> [!WARNING]
> Back up both updater files (a password manager is fine). If the private key is lost, every
> installed copy stops accepting updates and has to be reinstalled by hand. The matching public
> key is in `app/src-tauri/tauri.conf.json` (`plugins.updater.pubkey`).

## How the app updates

- `tauri-plugin-updater` is driven from Rust (`app/src-tauri/src/updates.rs`). Once a day, while
  **Settings > About > Automatic updates** is on (the default), it fetches
  `https://github.com/ReimagineMedias/unflub/releases/latest/download/latest.json`. That check
  (also on **Check now**) and the download after **Restart** are the only network traffic unflub
  makes.
- When there is a newer version, the About page shows **Update available — restart to install**.
  Nothing is downloaded until the user presses Restart, so a pending update costs no memory. Then
  the setup is downloaded, verified against the public key, and run: it closes unflub, upgrades
  it in place and opens it again.
- The updater asks for `windows-<arch>-nsis` or `windows-<arch>-msi` first, so each install keeps
  updating from the installer type it came from. `scripts/updater-manifest.mjs` writes those keys.
- Dev builds never check in the background and refuse to install.

Build installers with updater signatures locally (PowerShell, not Git Bash):

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = Get-Content -Raw D:\Code\unflub\.local\updater\unflub-updater.key
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = Get-Content -Raw D:\Code\unflub\.local\updater\unflub-updater.password
cd app; npm run tauri build -- --config src-tauri/tauri.release.conf.json
```

Without `--config src-tauri/tauri.release.conf.json`, `npm run tauri build` makes plain
installers and needs no key.

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
   `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT`, `AZURE_SIGNING_PROFILE`.
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
   - **Re-sign for the updater**: signing changes the files, so each `.sig` must be made again
     from the signed file: `npx tauri signer sign -f <key file> -p <password> <file>`, with the key
     and password written from the secrets to `$RUNNER_TEMP`.
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

All three manifests are in `packaging/` at version 0.1.0 with placeholder hashes (all zeros).
Fill the hashes from the published release's `SHA256SUMS.txt` before submitting anything.

### winget

The first version is submitted by hand; after that `publish-packages.yml` opens a pull request
for every published release, once enabled.

1. After v0.1.0 is published, put the two setup hashes into
   `packaging/winget/0.1.0/RMGN.unflub.installer.yaml`.
2. `winget validate --manifest packaging\winget\0.1.0` (no warnings once the hashes differ).
3. Test in Windows Sandbox: `winget install --manifest packaging\winget\0.1.0`, then
   `winget uninstall RMGN.unflub`.
4. Submit with `wingetcreate submit packaging\winget\0.1.0`, or open a PR to
   `microsoft/winget-pkgs` adding the three files under `manifests/r/RMGN/unflub/0.1.0/`.
5. Once it is merged: fork `microsoft/winget-pkgs` to the `ReimagineMedias` account, add
   `WINGET_TOKEN`, and set the variable `PUBLISH_PACKAGES` to `true`.

From then on, publishing a release opens the winget PR. Re-run it for a tag with Actions >
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

Scoop extracts the setup instead of running it, so unflub's own updater would install a second,
per-user copy. The manifest's notes tell Scoop users to turn off Automatic updates. Its
uninstaller removes start at login.

### Chocolatey

1. Put the x64 setup hash into `packaging/chocolatey/tools/chocolateyinstall.ps1`.
2. `choco pack packaging\chocolatey\unflub.nuspec`
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
| Start at login | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\unflub` | removed | removed |
| Task Manager's startup switch | `HKCU\...\Explorer\StartupApproved\Run\unflub` | removed | removed |
| Installer bookkeeping | `HKCU\Software\RMGN\unflub` | removed | n/a |
| Settings, last update check | `%APPDATA%\ca.rmgn.unflub` | with the checkbox | kept |
| Web view cache | `%LOCALAPPDATA%\ca.rmgn.unflub` | with the checkbox | kept |

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

**Start at login follows Windows.** The toggle in Settings > General reads the `Run` entry and
Task Manager's switch each time the window opens, so turning unflub off in Task Manager shows
there too. An upgrade that uninstalls the old version first removes the `Run` entry; if the
setting was on, unflub adds it back when it next starts.

## Rollback

- **Draft not published yet:** delete it, and the tag if it was wrong:
  `gh release delete v0.2.0 --cleanup-tag`.
- **Published and broken:** turn it back into a draft (`gh release edit v0.2.0 --draft=true`).
  `releases/latest` then serves the previous release, so copies that haven't updated stay where
  they are. The updater never downgrades, so copies that already updated need a fixed release:
  bump to 0.2.1 and release it the normal way.
- **winget:** close the automatic PR if it hasn't merged. If it has, the next version replaces
  it; ask the winget maintainers to remove a version only if it is harmful.
- Never replace the assets of a published release: winget, Scoop and `SHA256SUMS.txt` all pin
  their hashes.
- **Updater key leaked:** generate a new pair (`npx tauri signer generate -w <file>`), put the new
  public key in `tauri.conf.json`, and ship that version signed with the *old* key. Releases after
  it are signed with the new key.
