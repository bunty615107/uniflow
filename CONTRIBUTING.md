# Contributing to UniFlow

Thank you for your interest in contributing to UniFlow! We welcome contributions from the community. To ensure a smooth process and high-quality codebase, please review the following guidelines.

## 1. Rust Code Style

UniFlow strictly follows the standard Rust formatting and linting rules. Before committing your code, you must ensure that it passes both `cargo fmt` and `cargo clippy`.

### Formatting
Run the standard Rust formatter:
```bash
cargo fmt --all
```
Your code must pass `cargo fmt --all -- --check` in the CI pipeline.

### Linting
We use `clippy` to catch common mistakes and improve Rust code. Run clippy and address all warnings:
```bash
cargo clippy --all-targets --all-features -- -D warnings
```
Your code must compile without any warnings.

## 2. Branch Naming Conventions

Please use descriptive and structured branch names. We recommend the following prefixes:

- `feature/` - For new features or significant enhancements (e.g., `feature/p2p-transport`)
- `bugfix/` - For resolving bugs or issues (e.g., `bugfix/ui-refresh-loop`)
- `docs/` - For documentation updates (e.g., `docs/update-readme`)
- `refactor/` - For code refactoring without adding features or fixing bugs (e.g., `refactor/domain-models`)
- `test/` - For adding or fixing tests (e.g., `test/job-scheduler`)

Example: `feature/aws-s3-transport`

## 3. Commit Messages (Conventional Commits)

We require the use of [Conventional Commits](https://www.conventionalcommits.org/) for all commit messages. This helps us maintain a clean history and automate release notes.

Format:
```
<type>[optional scope]: <description>

[optional body]

[optional footer(s)]
```

### Allowed Types
- `feat`: A new feature
- `fix`: A bug fix
- `docs`: Documentation only changes
- `style`: Changes that do not affect the meaning of the code (white-space, formatting, missing semi-colons, etc)
- `refactor`: A code change that neither fixes a bug nor adds a feature
- `perf`: A code change that improves performance
- `test`: Adding missing tests or correcting existing tests
- `chore`: Changes to the build process or auxiliary tools and libraries

Example:
```
feat(transport): add basic sftp transport support
```

## 4. Running Tests

Before submitting a Pull Request, please ensure all tests pass. UniFlow includes comprehensive unit and integration tests.

Run the full test suite:
```bash
cargo test
```

If you are adding a new feature or fixing a bug, please include relevant tests to verify your changes.

## 5. Pull Request Process

1. **Fork the repository** and create your branch from `main`.
2. **Implement your changes**, ensuring you follow the code style, commit message conventions, and have added necessary tests.
3. **Run local checks**: Ensure `cargo fmt`, `cargo clippy`, and `cargo test` all pass.
4. **Open a Pull Request**: Use the provided PR template. Fill out the description thoroughly.
5. **Code Review**: The maintainers will review your PR. Be prepared to respond to feedback and make adjustments if necessary.
6. **Merge**: Once approved and all CI checks pass, your PR will be merged into `main`.

Thank you for contributing!
