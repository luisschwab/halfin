alias a := audit
alias b := build
alias c := check
alias cov := coverage
alias d := doc
alias do := doc-open
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

[doc: "Run cargo-audit across all lockfiles and prune stale advisories"]
audit:
    bash contrib/run-cargo-audit.sh
    bash contrib/prune-audit-ignores.sh

[doc: "Build `halfin`"]
build:
    RBMT_LOG_LEVEL=verbose cargo rbmt run build

[doc: "Check Formatting, Linting and Documentation"]
check:
    cargo rbmt fmt --check
    cargo rbmt lint
    cargo rbmt docs

[doc: "Generate Documentation"]
doc:
    cargo rbmt docs

[doc: "Generate and Open Documentation"]
doc-open:
    cargo rbmt docs --open

[doc: "Format Code"]
fmt:
    cargo rbmt fmt

[doc: "Regenerate Lockfiles"]
lock:
    cargo rbmt lock

[doc: "Run Tests"]
[env("RBMT_LOG_LEVEL", "verbose")]
test features="":
    {{ if features == "" { \
        "cargo rbmt test" \
    } else { \
        "cargo +" + stable + \
            " test --no-default-features --features " + quote(features) \
    } }}

[doc: "Run Tests with Lockfile and Toolchain Combos"]
[env("RBMT_LOG_LEVEL", "verbose")]
test-all features="":
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

[doc: "Generate Code Coverage"]
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

[doc: "Update Stable and Nightly Toolchains"]
toolchains:
    cargo rbmt toolchains --update-stable
    cargo rbmt toolchains --update-nightly

[doc: "Install cargo-rbmt Tools"]
tools:
    cargo rbmt tools
    cargo rbmt tools --update

[doc: "Run ShellCheck"]
shellcheck:
    @command -v shellcheck >/dev/null 2>&1 || { echo "shellcheck was not found on \$PATH" && exit 1; }
    git ls-files -z '*.sh' | xargs -0 shellcheck

[doc: "Run Zizmor Static Analysis"]
zizmor:
   zizmor .github

[doc: "Run pre-push checks"]
pre-push:
    @just lock
    @just tools
    @just check
    @just doc
    @just test-all
    @just shellcheck
    @just audit
    @just zizmor
