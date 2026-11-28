# Security policy

## Reporting a vulnerability

Report it privately: **Security → Report a vulnerability** on this repository (a GitHub
security advisory). Do not open a public issue for it.

Report privately, too, a defect that could let an application use wrong, old or missing
data without a fault indication, even when no attacker is involved. Its details become
public with the fix, in a GitHub security advisory.

Include the version or commit, the platform, what you did and what happened, and a way to
reproduce it if you have one.

## Supported versions

Before 1.0.0 only the latest release line receives fixes
([docs/release-policy.md](docs/release-policy.md) §6). A fix lands on `develop` and is
released in the next version.

## What is checked automatically

- Dependencies: Dependabot alerts and security updates, and `cargo deny` against the RustSec
  advisory database (`deny.toml`) in every gate and nightly.
- Code: CodeQL code scanning on pushes, pull requests and weekly.
- Secrets: secret scanning with push protection.

## Scope

hypha is a SIL 0 middleware that trusts the processes of one host (the threat model is in
[docs/communication-integrity.md](docs/communication-integrity.md)): another process of the
same user can read and write its shared memory. A report that needs such a process is in
scope when hypha fails to detect what it claims to detect.
