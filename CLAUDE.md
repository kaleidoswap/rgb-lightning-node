# Claude Review Guidance

This repository contains RLN (RGB Lightning Node), a Rust daemon and SDK surface.

When reviewing pull requests, focus on:

- Correctness and regressions in channel lifecycle, payment state, and persistence.
- Safety around wallet/accounting invariants and RGB transfer state transitions.
- Error handling in daemon APIs and background tasks to avoid partial state writes.
- Security-sensitive boundaries (auth tokens, signing keys, network IO, serialization).
- Test coverage for behavior changes, especially around regtest and integration flows.

Repository conventions:

- Keep changes minimal and scoped to the PR goal.
- Prefer explicit errors over silent fallback behavior.
- Avoid introducing breaking API changes unless the PR explicitly requires it.
- Keep CI/workflow changes conservative and deterministic.
