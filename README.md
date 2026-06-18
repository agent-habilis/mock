# agent-habilis-mock 🥸

agent-habilis-mock is an HTTP mock server for development and stable E2E tests.
It is a
[self-initializing fake](https://martinfowler.com/bliki/SelfInitializingFake.html)
implemented as a [reverse proxy](https://en.wikipedia.org/wiki/Reverse_proxy):
it records HTTP interactions to the filesystem so they can be replayed later
during local development or in
[broad-stack tests](https://martinfowler.com/bliki/BroadStackTest.html), removing
the need to reach the real upstream service.

It is written in Rust and ships as a single binary, `ahm`. Command-line flags
are idiomatic kebab-case.

## Installation

```bash
# Cargo (any platform; builds from source)
cargo install --git https://github.com/agent-habilis/mock --locked

# Or, from a checkout
cargo build --release    # binary at target/release/ahm
```

## Usage

```bash
ahm serve --origin http://example.com --mocks-dir ./mocks --mode read-write
```

`ahm` has two subcommands: `serve` runs the mock server (all flags below live
under it) and `man` prints the complete manual. Run `ahm serve --help` for the
full flag list. Point your client at the server (port `8273` by default)
instead of the real origin.

### Modes (`--mode`, default `pass`)

| Mode         | Behavior                                                              |
| ------------ | -------------------------------------------------------------------- |
| `pass`       | Always fetch from origin (no mock read or write).                    |
| `read`       | Serve a mock if one exists, otherwise `404`.                         |
| `write`      | Always fetch from origin and overwrite the mock.                     |
| `read-write` | Serve a mock if one exists, otherwise fetch, save, and serve.        |
| `read-pass`  | Serve a mock if one exists, otherwise fetch (no save).               |
| `pass-read`  | Fetch from origin; on a 5xx or network error, fall back to a mock.   |

### Options

| Flag                            | Default      | Description                                                              |
| ------------------------------- | ------------ | ----------------------------------------------------------------------- |
| `--origin <url>`                | —            | Origin base URL (defaults to `http://localhost` in `read` mode).        |
| `--mocks-dir <path>`            | `.`          | Directory to read/write mocked responses.                               |
| `--port <n>`                    | `8273`       | Port the server listens on.                                             |
| `--mode <mode>`                 | `pass`       | Record/playback mode (see table above).                                 |
| `--mock-keys <a,b,...>`         | `method,url` | Request attributes used to key mocks: `url`, `method`, `headers`, `body`, or dotted `body.*` / `header.*` paths. |
| `--retries <n>`                 | `0`          | Max retries to origin while the response is not a 2xx.                   |
| `--delay <ms>`                  | `0`          | Synthetic delay added to every response (max 3,600,000 ms).             |
| `--throttle <bytes/s>`          | `Infinity`   | Synthetic throughput cap; the response is streamed in paced chunks (`Infinity` = no cap). |
| `--logging <level>`             | `verbose`    | One of `silent`, `error`, `warn`, `verbose`.                            |
| `--cors`                        | off          | Send CORS headers between client and proxy (bare flag).                 |
| `--proxy <url>`                 | —            | Upstream HTTP proxy to forward origin requests through.                 |
| `--redacted-headers <json>`     | `{}`         | Header names to redact from mocks and logging.                          |
| `--overwrite-request-headers <json>`  | `{host}` | Request headers to overwrite on the way to origin.                  |
| `--overwrite-response-headers <json>` | `{}`     | Response headers to overwrite with the given values.                |
| `--rewrite-path <a=>b,...>`     | —            | Rewrite matching path prefixes before lookup/proxy (`prefix=>replacement`, comma-separated, first match wins; query string preserved). See note below. |
| `--update <off\|startup\|only>` | `off`        | Accepted for compatibility; the bulk mock-refresh pass is not yet implemented (mocks are left unchanged). |

Every response carries `x-powered-by: mocker`, `x-mocker-request-id`,
`x-mocker-response-from` (`Mock` or `Origin`), and — when served from disk —
`x-mocker-mock-path`. Health checks live under `/.well-known/live` and
`/.well-known/ready`.

#### `--rewrite-path`

Each rule is `prefix=>replacement`; rules are comma-separated and tried in
order, first match wins. Notes:

- **Matching is segment-aware.** `prefix` matches the request path only at a
  segment boundary: `/api` matches `/api` and `/api/...`, but not `/apidocs`.
- **Both halves must be absolute paths** starting with `/`; a prefix of exactly
  `/` is rejected (it would match everything). Bad rules fail at startup.
- **The matched prefix is swapped** for the replacement and the remainder is
  kept, e.g. `/api/federated-gateway-public/graphql=>/graphql` turns
  `/api/federated-gateway-public/graphql?op=Foo` into `/graphql?op=Foo`. The
  query string is preserved verbatim.
- **No escaping:** a `prefix` or `replacement` cannot itself contain `,` or `=>`.
- Paths are matched in their **percent-encoded** form (no decoding/normalizing),
  so a rule must use the same encoding the client sends.
- The rewrite is applied **after** the health check (so a rule can never shadow
  `/.well-known/live` or `/.well-known/ready`) and feeds both the mock-key lookup
  and the proxied request — recorded mocks are keyed under the rewritten path.

### Mock file format

Each interaction is one pretty-printed JSON file named `{hash}-{label}.json`:

```json
{
  "request":  { "method": "GET", "url": "/users", "headers": {}, "body": null },
  "response": { "statusCode": 200, "headers": {}, "body": { "id": 1 } }
}
```

JSON bodies are stored inline; binary bodies are stored base64-encoded as
`{ "encoding": "base64", "data": "..." }`.

## Library

The crate is also a library: `serve` drives a server in-process, which is how
the integration tests exercise it.

```rust,no_run
use agent_habilis_mock::args::{Cli, Command};
use clap::Parser;

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
if let Command::Serve(serve_args) = Cli::parse().command {
    agent_habilis_mock::serve(serve_args.validate()?).await?;
}
# Ok(())
# }
```

## Development

A small `cargo task` runner wraps the common workflows:

```bash
cargo task ci          # fmt check + clippy (-D warnings) + tests
cargo task test        # unit + integration tests
cargo task proptest    # property-based (proptest) tests only
cargo task fmt         # format
cargo task lint        # clippy --all-targets -D warnings
cargo task coverage    # cargo-llvm-cov report
cargo task release     # build target/release/ahm (no args)
```

Cutting a release wraps [`cargo-release`](https://github.com/crate-ci/cargo-release):

```bash
cargo task release patch              # dry run: preview a patch bump
cargo task release patch --execute    # bump Cargo.toml, commit, tag vX.Y.Z, push
```

Pushing the `vX.Y.Z` tag triggers `.github/workflows/release.yml`, which builds
the binaries, creates the GitHub release, and updates `Formula/ahm.rb`.

Testing has three layers:

- **Unit tests** alongside each module.
- **Property-based tests** (`prop_*`, via `proptest`) fuzz the parsing/encoding
  surface — body parse/serialize round-trips, decompression, header
  redaction, mock-path generation, and argument parsing — asserting invariants
  (no panics, bounded `.json` filenames, idempotent round-trips). They run as
  part of `cargo test`; `cargo task proptest` runs just them.
- **Integration tests** under `tests/` start a real `ahm` server and a stub
  origin in-process and assert protocol- and proxy-level behavior (including
  RFC 9112 §3.2.2 absolute-form request-targets through an upstream proxy, and
  paced throttled delivery). They are the primary guard on the external contract.
