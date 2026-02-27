set working-directory := "cokra-rs"
set positional-arguments

# Display help
help:
    just -l

# `cokra`
alias c := cokra
cokra *args:
    cargo run --bin cokra -- "$@"

# `cokra exec`
exec *args:
    cargo run --bin cokra -- exec "$@"

# Run the CLI version of the file-search crate.
file-search *args:
    cargo run --bin cokra-file-search -- "$@"

# Build the CLI and run the app-server test client
app-server-test-client *args:
    cargo build -p cokra-cli
    cargo run -p cokra-app-server-test-client -- --cokra-bin ./target/debug/cokra "$@"

# format code
fmt:
    cargo fmt -- --config imports_granularity=Item 2>/dev/null

fix *args:
    cargo clippy --fix --all-features --tests --allow-dirty "$@"

clippy:
    cargo clippy --all-features --tests "$@"

install:
    rustup show active-toolchain
    cargo fetch

# Run `cargo nextest` since it's faster than `cargo test`, though including
# --no-fail-fast is important to ensure all tests are run.
#
# Run `cargo install cargo-nextest` if you don't have it installed.
test:
    cargo nextest run --no-fail-fast

# Build and run Cokra from source using Bazel.
# Note we have to use the combination of `[no-cd]` and `--run_under="cd $PWD &&"`
# to ensure that Bazel runs the command in the current working directory.
[no-cd]
bazel-cokra *args:
    bazel run //cokra-rs/cli:cokra --run_under="cd $PWD &&" -- "$@"

[no-cd]
bazel-lock-update:
    bazel mod deps --lockfile_mode=update

[no-cd]
bazel-lock-check:
    ./scripts/check-module-bazel-lock.sh

bazel-test:
    bazel test //... --keep_going

bazel-remote-test:
    bazel test //... --config=remote --platforms=//:rbe --keep_going

build-for-release:
    bazel build //cokra-rs/cli:release_binaries --config=remote

# Run the MCP server
mcp-server-run *args:
    cargo run -p cokra-mcp-server -- "$@"

# Regenerate the json schema for config.toml from the current config types.
write-config-schema:
    cargo run -p cokra-core --bin cokra-write-config-schema

# Regenerate vendored app-server protocol schema artifacts.
write-app-server-schema *args:
    cargo run -p cokra-app-server-protocol --bin write_schema_fixtures -- "$@"

# Tail logs from the state SQLite database
log *args:
    if [ "${1:-}" = "--" ]; then shift; fi; cargo run -p cokra-state --bin logs_client -- "$@"
