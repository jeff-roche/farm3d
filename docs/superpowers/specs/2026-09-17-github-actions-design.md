# GitHub Actions Design

**Date:** 2026-09-17

## Goal

Give pull requests fast, consistent validation on the currently supported Linux
platform, and turn version tags into downloadable Linux release packages.
Packaging must never run for a pull request or an ordinary branch push.

## Workflows

### Pull Request Validation

`.github/workflows/pr-validation.yml` runs only for pull requests targeting
`main`. It uses `ubuntu-latest` and has read-only repository permissions.

Two independent jobs provide useful failure boundaries:

- **Frontend** installs the locked npm dependency graph with `npm ci`, then runs
  the same build and test commands exposed by `just build` and `just test`.
- **Rust** installs the Linux libraries required to compile Tauri, restores a
  Cargo build cache, then runs the command exposed by `just test-rust`.

Workflow concurrency is grouped by pull request and cancels superseded runs.
No job receives repository secrets, creates packages, or publishes artifacts.
Windows and macOS are excluded until their support status is verified.

### Tagged Release

`.github/workflows/tagged-release.yml` runs only when a tag matching `v*` is
pushed. It installs the frontend, Rust, Tauri system, and `just` dependencies,
then invokes the canonical `just package` recipe. That recipe owns the Linux
`NO_STRIP=1` workaround and verifies that release archives contain `farm3d` but
not the developer-only `gen-catalog` binary.

After a successful package build, the workflow uses the GitHub CLI already
present on the hosted runner to create a GitHub Release named after the tag and
attach the generated `.deb`, `.rpm`, and AppImage files. The workflow receives
`contents: write`; all other permissions remain unset. Re-running the workflow
must be safe: if the release already exists, assets are uploaded with overwrite
semantics rather than trying to create a duplicate release.

## Dependency And Security Policy

Official setup/cache actions and any third-party Rust cache action are pinned to
immutable commit SHAs. npm and Cargo consume committed lockfiles. The pull
request workflow remains read-only and does not use `pull_request_target`, so
forked code cannot execute with write credentials.

The tagged workflow is the sole packaging and release path. Its write token is
available only after a repository maintainer pushes a matching tag. Checkout
does not persist that write credential, so dependency installation and build
scripts cannot access it; release publication receives the token explicitly as
`GH_TOKEN`.

Application dependency graphs, action revisions, and the installed `just`
version are pinned. The hosted toolchain intentionally tracks `ubuntu-latest`,
Node.js 22, Rust stable, and the selected Ubuntu apt streams. Validation is
therefore repeatable at the dependency-policy level, not bit-for-bit
reproducible across hosted-runner image and toolchain updates.

## Failure Behavior

Frontend and Rust failures are reported independently on pull requests. A
package or package-content assertion failure prevents release creation. If
release publication fails after packaging, the workflow fails visibly and can
be re-run without changing the tag.

## Verification

Before merge:

1. Parse and lint both workflow files with `actionlint`.
2. Run `just build`, `just test`, and `source "$HOME/.cargo/env" && just test-rust`.
3. Confirm event filters and permissions by source inspection.
4. Confirm only the tagged workflow contains `just package` or release-writing
   commands.

The first real tag is the end-to-end release test. No test tag will be pushed as
part of this change because creating tags and releases is an external publishing
side effect.
