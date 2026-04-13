alias c := check
alias cs := check-sigs
alias d := doc
alias do := doc-open
alias f := fmt
alias l := lock
alias t := test
alias p := pre-push

_default:
    @echo "> halfin"
    @echo "> A regtest runner for \`bitcoind\` and \`utreexod\`"
    @just --list

[doc: "Check code formatting, compilation, and linting"]
check:
    RBMT_LOG_LEVEL=quiet cargo rbmt fmt --check
    RBMT_LOG_LEVEL=quiet cargo rbmt lint
    RBMT_LOG_LEVEL=quiet cargo rbmt docs

[doc: "Checks whether all commits in this branch are signed"]
check-sigs:
    #!/usr/bin/env bash
    TOTAL=$(git log --pretty='tformat:%H' origin/master..HEAD | wc -l | tr -d ' ')
    UNSIGNED=$(git log --pretty='tformat:%H %G?' origin/master..HEAD | grep " N$" | wc -l | tr -d ' ')
    if [ "$UNSIGNED" -gt 0 ]; then
        echo "⚠️ Unsigned commits in this branch [$UNSIGNED/$TOTAL]"
        exit 1
    else
        echo "🔏 All commits in this branch are signed [$TOTAL/$TOTAL]"
    fi

[doc: "Generate documentation"]
doc:
    RBMT_LOG_LEVEL=quiet cargo rbmt docs
    RBMT_LOG_LEVEL=quiet cargo doc --no-deps

[doc: "Generate and open documentation"]
doc-open:
    RBMT_LOG_LEVEL=quiet cargo rbmt docs
    RBMT_LOG_LEVEL=quiet cargo doc --no-deps --open

[doc: "Format code"]
fmt:
    RBMT_LOG_LEVEL=quiet cargo rbmt fmt

[doc: "Regenerate Cargo-recent.lock and Cargo-minimal.lock"]
lock:
    RBMT_LOG_LEVEL=quiet cargo rbmt lock

[doc: "Run tests across all toolchains and lockfiles"]
test:
    RBMT_LOG_LEVEL=quiet cargo rbmt test --toolchain stable --lock-file recent
    RBMT_LOG_LEVEL=quiet cargo rbmt test --toolchain stable --lock-file minimal

[doc: "Run pre-push suite: lock, fmt, check, and test"]
pre-push: lock fmt check check-sigs test
