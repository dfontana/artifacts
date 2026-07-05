---
name: code-complete
description: Run all automated code quality and verification tasks. Use when done making any rust file changes.
---

# Code Complete

## Instructions
1. Run `cargo fmt`
2. Run `cargo clippy`. If warnings exist, stop this skill and address all warnings. Then retry the skill from the beginning.
3. Run `cargo test`. If any tests are failing, fix them or the code regressing the test, depending on the context. Retry the skill from the beginning.

