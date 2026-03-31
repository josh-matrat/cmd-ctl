# Contributing to CMD CTL

Thanks for your interest in contributing to CMD CTL!

## Getting Started

1. Fork the repository
2. Clone your fork: `git clone https://github.com/<your-username>/cmd-ctl.git`
3. Create a branch: `git checkout -b my-feature`
4. Make your changes
5. Ensure it builds: `cargo build`
6. Run clippy: `cargo clippy -- -D warnings`
7. Commit and push your branch
8. Open a pull request

## Development

CMD CTL requires macOS with Metal support. The project uses a Cargo workspace — you can build individual crates or the whole workspace:

```bash
# Build everything
cargo build

# Build just the main app
cargo build -p cmdctl-app

# Run clippy on the workspace
cargo clippy --workspace
```

## Guidelines

- Keep PRs focused on a single change
- Follow existing code style and conventions
- Test your changes on macOS before submitting
- For larger changes, open an issue first to discuss the approach

## Reporting Issues

If you find a bug or have a feature request, please [open an issue](https://github.com/joshrutkowski/cmd-ctl/issues).

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE).
