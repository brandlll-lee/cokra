# Contributing to Cokra

Thank you for your interest in contributing to Cokra! This document provides guidelines for contributing to the project.

## 📋 Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Getting Started](#getting-started)
- [Development Workflow](#development-workflow)
- [Coding Standards](#coding-standards)
- [Testing](#testing)
- [Commit Messages](#commit-messages)
- [Pull Requests](#pull-requests)

## 🤝 Code of Conduct

We are committed to providing a welcoming and inclusive environment. Please be respectful and constructive in all interactions.

## 🚀 Getting Started

### Prerequisites

- **Rust**: 1.93.0+ (Edition 2024)
- **Bazel**: 9.0.0
- **Node.js**: 24+
- **PNPM**: 10.28.0+
- **Just**: Command runner

### Setup

1. Fork the repository
2. Clone your fork:
   ```bash
   git clone https://github.com/YOUR_USERNAME/cokra.git
   cd cokra
   ```

3. Install dependencies:
   ```bash
   just install
   pnpm install
   ```

4. Build the project:
   ```bash
   just build
   ```

## 💻 Development Workflow

### Branch Organization

- `main` - Protected branch for production
- `feat/*` - New features
- `fix/*` - Bug fixes
- `refactor/*` - Refactoring
- `docs/*` - Documentation updates

### Making Changes

1. Create a new branch:
   ```bash
   git checkout -b feat/your-feature-name
   ```

2. Make your changes following our [Coding Standards](#coding-standards)

3. Format your code:
   ```bash
   just fmt
   ```

4. Run linters:
   ```bash
   just lint
   ```

5. Run tests:
   ```bash
   just test
   ```

6. Commit your changes (see [Commit Messages](#commit-messages))

7. Push and create a pull request

## 📐 Coding Standards

### Rust

- Follow [AGENTS.md](AGENTS.md) for Rust coding conventions
- Use `cargo fmt` for formatting
- Use `cargo clippy` for linting
- Add tests for new functionality
- Update documentation as needed

### TypeScript

- Use `prettier` for formatting
- Use `eslint` for linting
- Follow the existing code style
- Add type annotations for all functions

### General

- Keep functions focused and small
- Write clear, self-documenting code
- Add comments for "why", not "what"
- Update relevant documentation

## 🧪 Testing

### Running Tests

```bash
# Run all tests
just test

# Run specific package tests
cargo test -p cokra-core

# Run with Bazel
bazel test //...
```

### Writing Tests

- Unit tests should be in the same file as the code
- Integration tests go in the `tests/` directory
- Use snapshot tests for UI/output validation
- Test both success and failure cases

## 📝 Commit Messages

We follow [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>[optional scope]: <description>

[optional body]

[optional footer(s)]
```

**Types:**
- `feat` - New feature
- `fix` - Bug fix
- `refactor` - Code refactoring
- `test` - Adding or updating tests
- `docs` - Documentation changes
- `chore` - Build/tooling changes
- `ci` - CI/CD changes
- `perf` - Performance improvements

**Examples:**
```
feat(cli): add interactive mode
fix(core): resolve race condition in agent spawning
refactor(tui): extract widget rendering logic
docs(readme): update installation instructions
```

## 🔄 Pull Requests

### Before Submitting

- [ ] Code follows coding standards
- [ ] All tests pass
- [ ] Documentation is updated
- [ ] Commit messages follow conventions
- [ ] PR description is clear and comprehensive

### PR Description Template

```markdown
## Summary
Brief description of changes

## Type of Change
- [ ] Bug fix
- [ ] New feature
- [ ] Breaking change
- [ ] Documentation update

## Testing
How was this tested?

## Checklist
- [ ] Tests pass locally
- [ ] Documentation updated
- [ ] No breaking changes (or documented)
```

### Review Process

1. Automated checks must pass
2. At least one maintainer approval
3. All feedback addressed
4. Squash and merge when approved

## 📧 Getting Help

- Open an issue for bugs or feature requests
- Start a discussion for questions
- Check existing issues and documentation

## 🙏 Thank You

Contributions of any size are welcome and appreciated!
