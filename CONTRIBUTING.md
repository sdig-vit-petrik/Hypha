# Contributing to hypha

hypha is a small, safety-oriented data plane. A change is welcome when it makes hypha
more correct, simpler or better evidenced; a new mechanism needs a reason a lean
framework would accept. Simple and robust comes before clever or fast.

## How a change is made

1. **An issue first.** Every change has one, except a typo-sized fix. It states the
   problem, the proposal, what was rejected and when it is done. The issue forms ask for
   exactly that.
2. **A branch from `develop`**, named `<type>/<issue>-<slug>` (`fix/126-invalid-data`).
   `main` holds releases only ([docs/release-policy.md](docs/release-policy.md) §7).
3. **The smallest complete change.** Reproduce a defect in a test before fixing it.
   Replace an old mechanism instead of keeping a compatibility path. Update the
   documents the change affects in the same pull request.
4. **Commits** name the issue in a trailer: `Refs #N` while the work goes on,
   `Fixes #N` in the commit that finishes it.
5. **A pull request into `develop`** that says what changed, which checks ran with
   their results, and what did not run.

## Checks

```sh
./scripts/format.sh   # format tracked files
./check.sh            # style, generated files, links, licences and advisories
./test.sh rust        # clippy, the Rust suite, the doctests
./test.sh             # everything: Rust, C++, the installed SDK, the examples
```

`./test.sh` cleans only its own test domains (`hypha clean --domain t_`), so it runs
beside a running hypha system. A change to the C ABI, the transport, the build or the
installation needs the full `./test.sh`.

## Moving the Rust toolchain

Two versions, each in one place that `check.sh` keeps consistent:

- **The compiler of development and CI** is pinned: edit `channel` in `rust-toolchain.toml`
  and `ARG HYPHA_RUST_VERSION` in `docker/Dockerfile` in one change, fix what the new lints
  report, and run `./check.sh` and `./test.sh`. The changed `docker/` builds the image once
  in CI.
- **The MSRV** (`rust-version` in `Cargo.toml`) is the highest `rust-version` of the
  dependencies: no older than they allow, no newer than needed. `check.sh` fails when a
  dependency update raises it; then set `rust-version` and `ARG HYPHA_RUST_MSRV` (its
  `x.y.0`) to the new value. `test.sh` and CI build the workspace with the MSRV, so code
  that uses a newer standard-library API than the MSRV fails there (`rustup toolchain
  install <msrv>` once, locally). The pinned compiler is never older than the MSRV.

## Rules worth knowing before you start

- The layering and the C ABI: [docs/architecture.md](docs/architecture.md),
  [docs/interface-specification.md](docs/interface-specification.md). An ABI change
  moves the header, `abi.revision`, the SDKs and the tests together.
- Rust style and lints: [docs/rust-style.md](docs/rust-style.md). `unsafe` lives only
  in the crates named there.
- A new public API, key, message field or flag answers one question in
  [docs/features.md](docs/features.md).
- Recurring traps of the transport and the tools:
  [docs/engineering-traps.md](docs/engineering-traps.md).
- Tests are deterministic: none asserts on how fast the host is.
- A vulnerability, or a defect that lets wrong data through without an indication, is
  reported privately: [SECURITY.md](SECURITY.md).

The same rules, written for coding agents, are in [AGENTS.md](AGENTS.md).

## Licence

Contributions are accepted under the licence of the repository,
[Apache-2.0](LICENSE).
