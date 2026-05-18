alias a := audit
alias b := build
alias c := check
alias cs := check-sigs
alias d := doc
alias do := doc-open
alias f := fmt
alias l := lock
alias t := test
alias z := zizmor
alias p := pre-push

_default:
    @echo "> halfin"
    @echo "> A regtest runner for \`bitcoind\` and \`utreexod\`\n"
    @just --list

[doc: "Run `cargo audit`"]
audit:
    cargo audit

[doc: "Build `halfin`"]
build:
    RBMT_LOG_LEVEL=progress cargo rbmt run build

[doc: "Check code formatting, compilation, and linting"]
check:
    RBMT_LOG_LEVEL=progress cargo rbmt fmt --check
    RBMT_LOG_LEVEL=progress cargo rbmt lint
    RBMT_LOG_LEVEL=progress cargo rbmt docsrs

[doc: "Checks whether all commits in this branch are signed"]
check-sigs:
    bash contrib/check-signatures.sh

[doc: "Generate documentation"]
doc:
    RBMT_LOG_LEVEL=progress cargo rbmt docsrs

[doc: "Generate and open documentation"]
doc-open:
    RBMT_LOG_LEVEL=progress cargo rbmt docsrs --open

[doc: "Format code"]
fmt:
    RBMT_LOG_LEVEL=progress cargo rbmt fmt

[doc: "Regenerate Cargo-recent.lock and Cargo-minimal.lock"]
lock:
    RBMT_LOG_LEVEL=verbose cargo rbmt lock

[doc: "Run tests across all toolchains and lockfiles"]
test:
    RBMT_LOG_LEVEL=verbose cargo rbmt test

[doc: "Run Zizmor Static Analysis"]
zizmor:
    uvx zizmor .

[doc: "Run pre-push checks"]
pre-push:
    @just lock
    @just check
    @just doc
    @just test
    @just audit
    @just zizmor
    @just check-sigs
