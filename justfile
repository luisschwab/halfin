alias a := audit
alias b := build
alias c := check
alias d := doc
alias do := doc-open
alias f := fmt
alias l := lock
alias t := test
alias sc := shellcheck
alias z := zizmor
alias p := pre-push

export RBMT_LOG_LEVEL := env("RBMT_LOG_LEVEL", "verbose")

_default:
    @echo "> halfin"
    @echo "> A regtest runner for \`bitcoind\` and \`utreexod\`\n"
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
test:
    cargo rbmt test

[doc: "Run Tests with Lockfile and Toolchain Combos"]
test-all:
    cargo rbmt test --toolchain stable --lockfile recent
    cargo rbmt test --toolchain stable --lockfile minimal
    cargo rbmt test --toolchain msrv --lockfile minimal

[doc: "Update Stable and Nightly Toolchains"]
toolchains:
    RBMT_LOG_LEVEL=progress cargo rbmt toolchains --update-stable
    RBMT_LOG_LEVEL=progress cargo rbmt toolchains --update-nightly

[doc: "Install cargo-rbmt Tools"]
tools:
    RBMT_LOG_LEVEL=progress cargo rbmt tools

[doc: "Run ShellCheck"]
shellcheck:
    @command -v shellcheck >/dev/null 2>&1 || { echo "shellcheck was not found on \$PATH" && exit 1; }
    find . -name '*.sh' -print -exec shellcheck {} +

[doc: "Run Zizmor Static Analysis"]
zizmor:
   zizmor .

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
