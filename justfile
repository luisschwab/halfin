alias a := audit
alias b := bisectability
alias c := check
alias cov := coverage
alias d := docs
alias do := docs-open
alias f := fmt
alias l := lock
alias t := test
alias sc := shellcheck
alias z := zizmor
alias p := pre-push

stable := `cargo rbmt toolchains --stable`
export RBMT_LOG_LEVEL := env("RBMT_LOG_LEVEL", "progress")

_default:
    @echo "> halfin"
    @echo "> A runner for bitcoin nodes and indexers\n"
    @just --list

# Quality

[doc: "Audit Cargo Dependencies"]
[group("Quality")]
audit:
    @echo "Auditing Cargo.lock"
    cargo audit --file Cargo.lock
    @echo "\nAuditing Cargo-maximum.lock"
    cargo audit --file Cargo-maximum.lock
    @echo "\nAuditing Cargo-recent.lock"
    cargo audit --file Cargo-recent.lock
    @echo "\nAuditing Cargo-minimal.lock"
    cargo audit --file Cargo-minimal.lock

[doc: "Assert Commit Bisectability"]
[group("Quality")]
bisectability baseline="master":
    cargo rbmt run --baseline "{{ baseline }}" -- build --quiet

[doc: "Check Formatting, Linting and Documentation"]
[group("Quality")]
check:
    cargo rbmt fmt --check
    cargo rbmt lint
    cargo rbmt docs

[doc: "Format Code"]
[group("Quality")]
fmt:
    cargo rbmt fmt

[doc: "Run Pre-Push Checks"]
[group("Quality")]
pre-push:
    # Generate Lockfiles
    cargo rbmt lock --lockfiles minimal,recent,maximum
    # Check Formatting
    cargo rbmt fmt --check
    # Check Linting
    cargo rbmt lint
    # Check Documentation
    cargo rbmt docs
    # Run Tests
    RBMT_LOG_LEVEL=verbose cargo rbmt test --toolchain stable --lockfile recent
    #RBMT_LOG_LEVEL=verbose cargo rbmt test --toolchain stable --lockfile minimal
    #RBMT_LOG_LEVEL=verbose cargo rbmt test --toolchain msrv --lockfile minimal
    # Audit Cargo Dependencies
    @just audit
    # Audit Shell Scripts
    @just shellcheck
    # Audit CI Files
    @just zizmor

[doc: "Run ShellCheck"]
[group("Quality")]
shellcheck:
    @command -v shellcheck >/dev/null 2>&1 || { echo "shellcheck was not found on \$PATH" && exit 1; }
    git ls-files -z '*.sh' | xargs -0 shellcheck

[doc: "Run Zizmor"]
[group("Quality")]
zizmor:
    zizmor .github

# Documentation

[doc: "Generate Documentation"]
[group("Documentation")]
docs:
    cargo rbmt docs

[doc: "Generate and Open Documentation"]
[group("Documentation")]
docs-open:
    cargo rbmt docs --open

# Testing

[doc: "Generate Coverage Report"]
[group("Testing")]
[env("CARGO_LLVM_COV_SETUP", "yes")]
coverage:
    cargo +{{ stable }} llvm-cov \
        --all-features \
        --html \
        --ignore-filename-regex '(^|/)test[.]rs$'
    cargo +{{ stable }} llvm-cov report \
        --lcov \
        --output-path target/llvm-cov/lcov.info \
        --ignore-filename-regex '(^|/)test[.]rs$'

[doc: "Run Tests with Specific Features"]
[group("Testing")]
[env("RBMT_LOG_LEVEL", "verbose")]
test features="":
    @echo "Test: toolchain=stable lockfile=recent"
    {{ if features == "" { \
        "cargo rbmt test --toolchain stable --lockfile recent" \
    } else { \
        "cargo rbmt run --toolchain stable --lockfile recent -- test" + \
            " --no-default-features --features " + quote(features) \
    } }}
    @echo "Test: toolchain=stable lockfile=minimal"
    {{ if features == "" { \
        "cargo rbmt test --toolchain stable --lockfile minimal" \
    } else { \
        "cargo rbmt run --toolchain stable --lockfile minimal -- test" + \
            " --no-default-features --features " + quote(features) \
    } }}
    @echo "Test: toolchain=msrv lockfile=minimal"
    {{ if features == "" { \
        "cargo rbmt test --toolchain msrv --lockfile minimal" \
    } else { \
        "cargo rbmt run --toolchain msrv --lockfile minimal -- test" + \
            " --no-default-features --features " + quote(features) \
    } }}

# Build

[doc: "Compile Binaries with a Cargo Example"]
[group("Build")]
compile-bins example_name:
    RBMT_LOG_LEVEL=versbose cargo rbmt run -- run --example "{{ example_name }}"

[doc: "Build `halfin`"]
[env("RBMT_LOG_LEVEL", "verbose")]
[group("Build")]
build:
    cargo rbmt run -- build

# Dependencies

[doc: "Regenerate Lockfiles"]
[group("Dependencies")]
lock:
    cargo rbmt lock --lockfiles minimal,recent,maximum

# Setup

[doc: "Install Tools and Toolchains"]
[group("Setup")]
install-tools-toolchains:
    cargo rbmt tools
    cargo rbmt toolchains

[doc: "Update Tools and Toolchains"]
[group("Setup")]
update-tools-toolchains:
    cargo rbmt tools --update
    cargo rbmt toolchains --update-stable
    cargo rbmt toolchains --update-nightly
