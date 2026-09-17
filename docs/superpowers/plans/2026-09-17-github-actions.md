# GitHub Actions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Linux pull-request validation and publish verified Linux packages in a GitHub Release whenever a `v*` tag is pushed.

**Architecture:** Keep validation and publishing in separate workflows so untrusted pull-request code never receives write credentials and packaging cannot run on ordinary pushes. Both workflows use the repository's locked dependencies and existing commands; the tag workflow delegates packaging and archive assertions to `just package` before publishing assets with `gh`.

**Tech Stack:** GitHub Actions, Node.js/npm, Rust/Cargo, Tauri 2, `just`, GitHub CLI, `actionlint`

**Spec:** `docs/superpowers/specs/2026-09-17-github-actions-design.md`

## Global Constraints

- Pull-request validation runs only on `ubuntu-latest` for pull requests targeting `main`.
- Pull-request jobs have `contents: read`, receive no repository secrets, and never package or publish.
- Packaging runs only for pushed tags matching `v*`.
- The tag workflow creates or updates a GitHub Release and attaches `.deb`, `.rpm`, and AppImage assets.
- The release workflow alone has `contents: write`.
- Actions are pinned to immutable commit SHAs.
- Use committed npm and Cargo lockfiles; do not add application dependencies.

---

### Task 1: Pull Request Validation

**Files:**
- Create: `.github/workflows/pr-validation.yml`

**Interfaces:**
- Consumes: `package-lock.json`, `src-tauri/Cargo.lock`, `npm run build`, `npm test`, and `cargo test --manifest-path src-tauri/Cargo.toml`
- Produces: independent `Frontend` and `Rust` required-check candidates for pull requests

- [ ] **Step 1: Verify the workflow is absent**

Run:

```bash
test -f .github/workflows/pr-validation.yml
```

Expected: exit 1 because no pull-request workflow exists.

- [ ] **Step 2: Create the pull-request workflow**

Create `.github/workflows/pr-validation.yml` with:

```yaml
name: Pull Request Validation

on:
  pull_request:
    branches: [main]

permissions:
  contents: read

concurrency:
  group: pr-validation-${{ github.event.pull_request.number }}
  cancel-in-progress: true

jobs:
  frontend:
    name: Frontend
    runs-on: ubuntu-latest
    steps:
      - name: Check out repository
        uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262
      - name: Set up Node.js
        uses: actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020
        with:
          node-version: 22
          cache: npm
      - name: Install dependencies
        run: npm ci
      - name: Build frontend
        run: npm run build
      - name: Test frontend
        run: npm test

  rust:
    name: Rust
    runs-on: ubuntu-latest
    steps:
      - name: Check out repository
        uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262
      - name: Install Tauri system dependencies
        run: |
          sudo apt-get update
          sudo apt-get install --yes \
            build-essential \
            libayatana-appindicator3-dev \
            libdbus-1-dev \
            librsvg2-dev \
            libssl-dev \
            libwebkit2gtk-4.1-dev \
            libxdo-dev \
            pkg-config
      - name: Select stable Rust
        run: |
          rustup toolchain install stable --profile minimal
          rustup default stable
      - name: Restore Cargo cache
        uses: Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6
        with:
          workspaces: src-tauri
      - name: Test Rust backend
        run: cargo test --locked --manifest-path src-tauri/Cargo.toml
```

- [ ] **Step 3: Lint the workflow**

Run:

```bash
go run github.com/rhysd/actionlint/cmd/actionlint@v1.7.7 .github/workflows/pr-validation.yml
```

Expected: exit 0 with no diagnostics.

- [ ] **Step 4: Commit the PR workflow**

```bash
git add .github/workflows/pr-validation.yml
git commit -m "ci: validate pull requests"
```

### Task 2: Tagged Release Publishing

**Files:**
- Create: `.github/workflows/tagged-release.yml`

**Interfaces:**
- Consumes: `just package`, package-content assertions, and a pushed `v*` tag
- Produces: a GitHub Release containing one `.deb`, one `.rpm`, and one AppImage

- [ ] **Step 1: Verify the release workflow is absent**

Run:

```bash
test -f .github/workflows/tagged-release.yml
```

Expected: exit 1 because no tagged release workflow exists.

- [ ] **Step 2: Create the tagged workflow**

Create `.github/workflows/tagged-release.yml` with:

```yaml
name: Tagged Release

on:
  push:
    tags: ["v*"]

permissions:
  contents: write

concurrency:
  group: tagged-release-${{ github.ref }}
  cancel-in-progress: false

jobs:
  linux-packages:
    name: Linux Packages
    runs-on: ubuntu-latest
    steps:
      - name: Check out repository
        uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262
      - name: Set up Node.js
        uses: actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020
        with:
          node-version: 22
          cache: npm
      - name: Install Tauri system dependencies
        run: |
          sudo apt-get update
          sudo apt-get install --yes \
            build-essential \
            libayatana-appindicator3-dev \
            libdbus-1-dev \
            librsvg2-dev \
            libssl-dev \
            libwebkit2gtk-4.1-dev \
            libxdo-dev \
            pkg-config \
            rpm
      - name: Select stable Rust
        run: |
          rustup toolchain install stable --profile minimal
          rustup default stable
      - name: Restore Cargo cache
        uses: Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6
        with:
          workspaces: src-tauri
      - name: Install just
        run: cargo install just --locked --version 1.58.0
      - name: Install frontend dependencies
        run: npm ci
      - name: Build verified packages
        run: just package
      - name: Publish GitHub Release
        env:
          GH_TOKEN: ${{ github.token }}
        shell: bash
        run: |
          shopt -s nullglob
          assets=(
            src-tauri/target/release/bundle/deb/*.deb
            src-tauri/target/release/bundle/rpm/*.rpm
            src-tauri/target/release/bundle/appimage/*.AppImage
          )
          if [[ ${#assets[@]} -ne 3 ]]; then
            printf 'expected 3 release assets, found %s\n' "${#assets[@]}" >&2
            exit 1
          fi
          if gh release view "$GITHUB_REF_NAME" >/dev/null 2>&1; then
            gh release upload "$GITHUB_REF_NAME" "${assets[@]}" --clobber
          else
            gh release create "$GITHUB_REF_NAME" "${assets[@]}" \
              --verify-tag \
              --generate-notes \
              --title "$GITHUB_REF_NAME"
          fi
```

- [ ] **Step 3: Lint both workflows and assert trigger separation**

Run:

```bash
go run github.com/rhysd/actionlint/cmd/actionlint@v1.7.7 .github/workflows/*.yml
test "$(rg -l 'just package' .github/workflows)" = ".github/workflows/tagged-release.yml"
test "$(rg -l 'gh release (create|upload)' .github/workflows)" = ".github/workflows/tagged-release.yml"
```

Expected: all commands exit 0 with no lint diagnostics.

- [ ] **Step 4: Commit the tagged workflow**

```bash
git add .github/workflows/tagged-release.yml
git commit -m "ci: publish tagged Linux releases"
```

### Task 3: Whole-Branch Verification

**Files:**
- Verify: `.github/workflows/pr-validation.yml`
- Verify: `.github/workflows/tagged-release.yml`
- Verify: `docs/superpowers/specs/2026-09-17-github-actions-design.md`

**Interfaces:**
- Consumes: both completed workflows
- Produces: review-ready branch evidence without creating an external tag or release

- [ ] **Step 1: Run workflow validation**

```bash
go run github.com/rhysd/actionlint/cmd/actionlint@v1.7.7 .github/workflows/*.yml
test "$(rg -l 'just package' .github/workflows)" = ".github/workflows/tagged-release.yml"
test "$(rg -l 'contents: write' .github/workflows)" = ".github/workflows/tagged-release.yml"
```

Expected: all commands exit 0.

- [ ] **Step 2: Run project verification**

```bash
just build
just test
source "$HOME/.cargo/env" && just test-rust
```

Expected: build succeeds; all frontend and Rust tests pass.

- [ ] **Step 3: Inspect the final branch**

```bash
git diff --check main...HEAD
```

Expected: no whitespace errors, only intended files, and three focused commits including the design.
