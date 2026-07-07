# ethercrab — Codex local guardrails

Pinned fork of the EtherCrab EtherCAT crate; motiond depends on it by relative path (sibling layout is load-bearing — never move or rename folders). Global method: `~/.codex/AGENTS.md`; also read `../AGENTS.md` (the Nuanse bridge).

- Treat as a stable dependency: no casual upstream syncs, no API churn, no version bumps. Changes only with a concrete motiond-side reason.
- After ANY change here, verify motiond still builds: `cargo check` in `../motiond`.
- This crate sits in the real-time EtherCAT path of a machine that moves people — motiond's plan-first conservatism applies here too.
