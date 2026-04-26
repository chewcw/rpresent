# AGENTS — Repo instructions for AI coding agents

Purpose: concise, actionable guidance to help AI coding agents be productive in this repository.

Quick orientation
- **Primary language**: Rust — see [Cargo.toml](Cargo.toml).
- **Setup hint**: rustup and cargo are used here (example: `rustup override set stable`).

How to behave (short)
- Read this file and linked docs before making changes.
- If you are uncertain about assumptions, ask the human rather than guessing.
- Make the smallest, surgical change that satisfies the user's request.

Behavioral guidelines to reduce common LLM coding mistakes

These guidelines bias toward caution over speed. For trivial tasks, use judgment.

1. Think Before Coding

Don't assume. Don't hide confusion. Surface tradeoffs.

- Before implementing: state assumptions explicitly. If uncertain, ask.
- If multiple interpretations exist, present them — don't pick silently.
- If a simpler approach exists, say so. Push back when warranted.
- If something is unclear, stop. Name what's confusing. Ask.

2. Simplicity First

Minimum code that solves the problem. Nothing speculative.

- No features beyond what was asked.
- No abstractions for single-use code.
- No extra configurability that wasn't requested.
- No error handling for impossible scenarios.
- If code can be much shorter, prefer the shorter version.

3. Surgical Changes

Touch only what you must. Clean up only your own mess.

- Don't change unrelated code, comments, or formatting.
- Don't refactor things that aren't broken; match existing style.
- If your changes create unused symbols, remove those only.
- If you find unrelated dead code, mention it — don't delete it.

4. Goal-Driven Execution

Define success criteria and loop until verified.

- Convert tasks into verifiable goals (e.g., add a test, make it pass).
- For multi-step work: provide a short plan with checks for each step.

Link, don't embed: prefer linking to existing workspace docs for details.

If you make edits to files: keep diffs minimal and focused. After changes, ask if you should run tests or submit a patch.

