# Nexa Commerce agent workflow

Read [README.md](README.md) for the project map. For service ownership and checkout invariants, consult [technical design](docs-desige/04_Documentation/Nexa_Commerce_Technical_Design.md). For client or API changes, consult the [API draft](project/contracts/nexa_gateway.openapi.yaml) and the [runnable workspace](project/README.md).

## Agent loop

1. **Select.** Identify the requested outcome and choose the smallest unfinished slice that moves it forward. Record the entry point and an observable acceptance check. Before delegating shared API work, settle the path, payload, and error behavior.
2. **Implement.** The lead handles small or tightly coupled changes and delegates independent work or review with a clear deliverable. Use at most two concurrent subagents by default, one delegation level, and one writer per path. Assign shared contracts, lockfiles, and migrations to a single owner. Serialize checks sharing a database or port, or isolate each worker.
3. **Verify.** The lead reviews the integrated changes and reruns the relevant checks. For Catalog changes, check the API response against the contract and both consumers. Record each command, result, and any missing prerequisite.
4. **Repeat or finish.** If an acceptance check fails, use the evidence to make or assign a focused fix, then verify the integrated result again. If the slice passes, select the next unfinished slice within the request. Finish when all requested work is delivered and its acceptance checks pass, the user stops the work, or a concrete blocker prevents further progress. Report the verified outcomes and any remaining work with its blocker.

## Subagents

Read the assignment and applicable repository instructions. Edit only owned paths and coordinate interface changes through the lead. Return changed files, commands and results, and any blockers or assumptions. For read-only review, return findings with file locations and reproduction evidence where available.

## Model routing

Keep the lead session's configured model. For delegated tasks, use the first matching role below and set the model and reasoning effort at spawn time when supported.

| Assignment | Model | Effort |
| --- | --- | --- |
| File lookup, link checks, or a prescribed verification command | `gpt-6-luna` | low |
| Visual review of React or Tauri screenshots | `gpt-6.1-terra` | low |
| Bounded implementation, code review, or debugging in one component | `gpt-6.2-sol` | medium |
| Checkout saga, database migrations, concurrency, or cross-service invariants | `gpt-6.2-astra` | high |

Use the inherited model when the selected override is unavailable. Escalate with the concrete failed check or unresolved invariant so the next agent can resume from the evidence.

For scoped assignments, spawn with `fork_turns="none"`. Include the goal, relevant paths, accepted interface decisions, write scope, and verification command. Point to existing files for source evidence. Use inherited conversation history only when essential context cannot be captured in a short handoff. Keep tool output focused with `rg`, targeted reads, and output limits. Summarize results with file references and exit statuses.
