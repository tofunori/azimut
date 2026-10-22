# Contributing to Azimut

Thank you for helping. A few rules keep the project reliable and legally clean.

## Public sources only

- Every rule about a file format or device behaviour must cite a public source in
  [SOURCES.md](SOURCES.md): a manufacturer manual, a published standard, a paper, an
  open-source project, or your own measurement on your own data (describe it).
- Never decompile, disassemble or inspect the internals of closed-source software.
- Do not add outputs, reports, screenshots, interface text or templates produced by
  closed-source tools to the repository. Comparisons with such tools stay outside the repo.

## Sign-off

Sign each commit with `git commit -s` (Developer Certificate of Origin,
https://developercertificate.org/). It states that you wrote the change or have the right to
submit it under the project license.

## Data

Never commit real survey data unless its owner agreed to publish it under an open license.
Test fixtures should be synthetic (`azimut_io::synthetic`) or small, licensed extracts.

## Code

- `cargo fmt --all`, `cargo clippy --all-targets --all-features -- -D warnings` and
  `cargo test --all` must pass (CI checks them).
- Read metadata from headers only; never decode pixels unless a feature needs them.
- Add a test for each pitfall you fix, ideally with a synthetic case that reproduces it.
