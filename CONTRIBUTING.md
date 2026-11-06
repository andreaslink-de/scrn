# Contributing

Thanks for your interest in SCRN. Bug reports, ideas and pull requests are
welcome.

## Before you start

- For larger changes, open an issue first to discuss the idea.
- Security problems: follow [SECURITY.md](SECURITY.md), not the issue
  tracker.

## Development

You need a stable Rust toolchain and Docker.

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
docker compose up --build    # run locally on http://127.0.0.1:8081
```

All three checks must pass; CI runs them on every pull request, together
with `cargo deny check` for advisories and licenses.

## Guidelines

- Code, comments, commit messages, docs, log and API error messages are in
  English. UI text lives in the catalogs under
  `crates/scrn-server/locales` (German and English), never in templates or
  code; add every new key to both catalogs.
- No `unwrap()` or `expect()` in request paths; use typed errors.
- Every new behavior gets tests: unit tests and integration tests against a
  temporary SQLite database (see `crates/scrn-server/tests`).
- Keep dependencies minimal and explain each new crate in the commit
  message. Licenses must be accepted by `deny.toml`.
- Never edit an applied migration; add a new one.
- Use conventional commits: `feat:`, `fix:`, `chore:`, `docs:`, `test:`.

The full rules, including the security rules, are in
[CLAUDE.md](CLAUDE.md).

## License

By contributing you agree that your contributions are licensed under the
[MIT License](LICENSE).
