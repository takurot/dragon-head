---
name: start-task
description: Guides the agent on how to start and execute a development task in the Neural-Browser Runtime (dragon-head) project following strict repository rules. Use this when the user asks you to start a task, pick up the next task, or proceed with development.
---

# Start Task Skill

This skill defines the standard operating procedure for starting and completing a development task in the `dragon-head` (Neural-Browser Runtime) repository. It ensures that planning documents, coding conventions, architectural specifications, and PR rules are strictly followed from start to finish.

## When to use this skill

- When the user asks you to "start a task", "pick up the next task", "implement the next phase", or "continue with development".
- When you are tasked with selecting the next valid task from the `PLAN.md` to work on.

## Strategy: Explore -> Plan -> Implement -> Commit

Follow these steps sequentially to execute a task:

### 1. Explore & Plan
Do not jump straight to coding. Build the necessary context first:
1. **Read the Rules**: Review `docs/PROMPT.md` carefully. This document contains absolute rules for branching, committing, testing, error handling, and coding standards. You MUST adhere to them. 
2. **Review Specifications**: Check both `docs/PLAN.md` and `docs/SPEC.md`. Understand the current project phase and PR dependencies from `PLAN.md`.
3. **Select the Task**: Identify the next logical task/PR to implement that has not been completed yet (e.g., `PR-01`), based on `docs/PLAN.md`.
4. **Acquire Code Context**: Explore the existing codebase (using file viewing, directory listing, or search tools) to fully understand the areas your selected task will modify.
5. **Ask/Formulate**: If the task scope is ambiguous or modifies multiple core files, create a brief implementation plan and ask the user for verification.

### 2. Implement & Verify
Ensure you have a reliable way to verify your work:
1. **Create Branch**: Check out a new branch from the latest `main`. The branch name must follow the `feature/PR-XX-description` convention as defined in `docs/PROMPT.md`.
2. **Test-Driven Development (TDD)**: Perform strict TDD (Red -> Green -> Refactor) as requested. Always start by writing a failing test based on the test tasks in `docs/PLAN.md`.
3. **Write and Execute Code**: Write your implementation code in small, incremental steps.
4. **Local Quality Gates**: Ensure your code passes all mandatory quality checks before concluding.
   - Run tests: `cargo test --workspace` (or `pytest` for Python components, or `just test-all` if available).
   - Run linters: `cargo fmt --all -- --check` and `cargo clippy --workspace -- -D warnings`.

### 3. Commit, PR & Document Update
1. **Update Docs**: Mark the completed task with `[x]` in `docs/PLAN.md`. Update `docs/SPEC.md` or `README.md` if necessary.
2. **Commit**: Ensure any new files are added with `git add`. Use Conventional Commits (`<type>(<scope>): <summary>`) for your commit messages.
3. **Push**: Push your created branch to the remote repository (`git push origin feature/PR-XX-...`).
4. **Create PR**: Open a Pull Request for your branch using the GitHub CLI (`gh pr create`). Ensure the PR title is `[PR-XX] 実装の概要` and the description includes all mandatory sections (Exit Criteria status, test results).
5. **Monitor CI**: Ensure CI passes and fix any issues automatically.
