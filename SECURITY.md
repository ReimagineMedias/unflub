# Security

unflub watches your keyboard to fix typos, so we treat security reports as our top priority.

## Reporting a problem

Please report a vulnerability privately through
[GitHub's private vulnerability reporting](https://github.com/ReimagineMedias/unflub/security/advisories/new),
not in a public issue. Include the unflub version (Settings > About), your Windows version, and the steps that
show the problem. We aim to reply within a few days and will credit you in the release notes unless you'd
rather we didn't.

## What we care about most

- Anything that lets typed text leave the computer, be written to disk, or reach another process.
- Typing into password fields or other fields unflub should leave alone.
- Ways to make unflub type, delete or change text it shouldn't.
- Tampering with updates: the signed manifest on dl.rmgn.net, the updater signature, or the installer download.

## Supported versions

Only the newest release gets security fixes. unflub updates itself, so most people are already on it.

## How unflub is built to be safe

- The keyboard engine (`unflub-engine.exe`) contains no networking code; a CI check fails the build if any appears.
- Update manifests are signed with Ed25519 and installers with the Tauri updater key; both are checked before
  anything is installed.
- Installers are built in GitHub Actions with build provenance you can check:
  `gh attestation verify <file> --repo ReimagineMedias/unflub`.
