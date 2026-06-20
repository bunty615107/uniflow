# Contributing to UniFlow

Thank you for your interest in contributing to UniFlow! We welcome contributions from the community. To ensure a smooth process and high-quality codebase, please follow these guidelines.

## Code of Conduct

By participating in this project, you agree to abide by our Code of Conduct (if applicable, please link here). Treat everyone with respect and maintain a welcoming environment.

## Getting Started

1.  **Fork the repository** on GitHub.
2.  **Clone your fork** locally: `git clone https://github.com/YOUR_USERNAME/uniflow.git`
3.  **Create a new branch** for your feature or bugfix (see Branch Naming below).

## Branch Naming

We use a structured branch naming convention. Please prefix your branches appropriately:

*   `feature/` - For new features (e.g., `feature/add-sftp-transport`)
*   `bugfix/` - For bug fixes (e.g., `bugfix/fix-memory-leak`)
*   `docs/` - For documentation changes
*   `chore/` - For maintenance tasks, dependency updates, etc.

Example: `feature/rclone-integration`

## Commit Messages

We strictly follow **Conventional Commits**. This helps us automate changelogs and maintain a clear history.

Format: `<type>(<scope>): <subject>`

Types:
*   `feat`: A new feature
*   `fix`: A bug fix
*   `docs`: Documentation only changes
*   `style`: Changes that do not affect the meaning of the code (white-space, formatting, etc.)
*   `refactor`: A code change that neither fixes a bug nor adds a feature
*   `perf`: A code change that improves performance
*   `test`: Adding missing tests or correcting existing tests
*   `chore`: Changes to the build process or auxiliary tools and libraries

Example: `feat(transport): add basic SFTP support`

## Rust Code Style

UniFlow enforces strict Rust code style checks. Before submitting a PR, you **must** ensure your code passes these checks.

1.  **Formatting:** We use `rustfmt`. Run the following command and ensure there are no formatting changes required:
    ```bash
    cargo fmt --all -- --check
    ```
    To automatically format your code, run:
    ```bash
    cargo fmt --all
    ```

2.  **Linting:** We use `clippy` to catch common mistakes and enforce idiomatic Rust. Your code must compile without clippy warnings:
    ```bash
    cargo clippy --all-targets --all-features -- -D warnings
    ```

## Running Tests

UniFlow relies heavily on tests to ensure stability and correctness. You should run tests locally before pushing your changes.

**Note:** You must have the protobuf compiler (`protoc`) installed on your system to compile and run tests.
*   Ubuntu/Debian: `sudo apt-get install -y protobuf-compiler`
*   macOS: `brew install protobuf`

Run the test suite:
```bash
cargo test
```

If you are adding a new feature or fixing a bug, please include relevant unit or integration tests.

## Pull Request Process

1.  **Ensure your code passes all checks** (formatting, clippy, tests).
2.  **Push your branch** to your fork on GitHub.
3.  **Open a Pull Request** against the `main` branch of the upstream repository.
4.  **Fill out the Pull Request Template**. Provide clear descriptions of what you changed, why, and how it was tested.
5.  **Address Review Comments**. Maintainers may request changes. Be prepared to discuss and update your code.
6.  Once approved, a maintainer will merge your PR.

## Security

If you discover a security vulnerability, please do **NOT** open a public issue. See `SECURITY.md` for instructions on how to report it responsibly.

Thank you for contributing!
