# Contributing to uniflow

Thank you for considering contributing to the uniflow Rust web app! We appreciate your support and help in making this project better.

## Code of Conduct
Please ensure a welcoming and inclusive environment for all.

## Branch Naming
We follow a structured branch naming convention:
- `feature/` - For new features
- `bugfix/` - For resolving issues
- `docs/` - For documentation changes
- `chore/` - For maintenance tasks

Example: `feature/new-api-endpoint`

## Commit Messages
We strictly use **Conventional Commits**. This is crucial for maintaining a readable history.
Format: `<type>(<scope>): <subject>`

Examples:
- `feat: add new user authentication`
- `fix(api): resolve panic on missing header`
- `docs: update setup instructions`

## Rust Code Style
We enforce standard Rust formatting and linting rules. Ensure your code passes both before submitting a pull request:

1. **Formatting:** Run `cargo fmt` to automatically format your code.
   ```bash
   cargo fmt --all
   ```
2. **Linting:** Use `clippy` to catch common mistakes and improve code quality.
   ```bash
   cargo clippy --all-targets --all-features -- -D warnings
   ```

## How to Run Tests
Before opening a PR, ensure all tests pass:
```bash
cargo test
```
Please add tests for any new features or bug fixes you introduce.

## Pull Request Process
1. Fork the repository and create your branch from `main`.
2. Make your changes and ensure they follow the **Rust Code Style** (rustfmt, clippy).
3. Ensure all tests pass.
4. Open a Pull Request using the provided template.
5. Address any feedback from reviewers.
