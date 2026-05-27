alias a := audit
alias b := build
alias c := check
alias cs := check-sigs
alias d := doc
alias do := doc-open
alias f := fmt
alias l := lock
alias t := test
alias sc := shellcheck
alias z := zizmor
alias p := pre-push

_default:
    @echo "> halfin"
    @echo "> A regtest runner for \`bitcoind\` and \`utreexod\`\n"
    @just --list

[doc: "Run `cargo audit` on all lockfiles and prune ignored advisories"]
audit:
    bash contrib/run-cargo-audit.sh
    bash contrib/prune-audit-ignores.sh

[doc: "Build `halfin`"]
build:
    RBMT_LOG_LEVEL=verbose cargo rbmt run build

[doc: "Check code formatting, compilation, and linting"]
check:
    RBMT_LOG_LEVEL=verbose cargo rbmt fmt --check
    RBMT_LOG_LEVEL=verbose cargo rbmt lint
    RBMT_LOG_LEVEL=verbose cargo rbmt docsrs

[doc: "Checks whether all commits in this branch are signed"]
check-sigs:
    bash contrib/check-signatures.sh

[doc: "Generate documentation"]
doc:
    RBMT_LOG_LEVEL=verbose cargo rbmt docsrs

[doc: "Generate and open documentation"]
doc-open:
    RBMT_LOG_LEVEL=verbose cargo rbmt docsrs --open

[doc: "Format code"]
fmt:
    RBMT_LOG_LEVEL=verbose cargo rbmt fmt

[doc: "Regenerate Cargo-recent.lock and Cargo-minimal.lock"]
lock:
    RBMT_LOG_LEVEL=verbose cargo rbmt lock

[doc: "Run tests with relevant toolchain and lockfile combinations"]
test:
    @just lock
    RBMT_LOG_LEVEL=verbose cargo rbmt test --toolchain stable --lock-file recent
    RBMT_LOG_LEVEL=verbose cargo rbmt test --toolchain stable --lock-file minimal
    RBMT_LOG_LEVEL=verbose cargo rbmt test --toolchain msrv --lock-file minimal

[doc: "Install and/or Update `cargo-rbmt` and Stable and Nightly toolchains"]
toolchains:
    bash contrib/install-cargo-rbmt.sh
    RBMT_LOG_LEVEL=progress cargo rbmt toolchains --update-stable
    RBMT_LOG_LEVEL=progress cargo rbmt toolchains --update-nightly

[doc: "Run ShellCheck"]
shellcheck:
    @command -v shellcheck >/dev/null 2>&1 || { echo "shellcheck was not found on \$PATH" && exit 1; }
    find . -name '*.sh' -print -exec shellcheck {} +

[doc: "Run Zizmor Static Analysis"]
zizmor:
    uvx zizmor .

[doc: "Run pre-push checks"]
pre-push:
    @just lock
    @just check
    @just doc
    @just test
    @just shellcheck
    @just audit
    @just zizmor
    @just check-sigs
