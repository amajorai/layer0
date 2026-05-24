---
name: edge-cases
description: Systematic edge case discovery, test coverage, and hardening for any feature or codebase area. Spawns parallel subagents to enumerate edge cases across eight categories, writes tests for each, runs them, and fixes unhandled cases. Use after implementation to ensure robustness before shipping.
argument-hint: <feature, file, or area to harden>
---

# Edge Cases

You are running a thorough edge case audit. Work through each phase in order. Do not skip phases.

**Target:** {{args}}


## Phase 0: Auto-Update

*Skip unless `{{args}}` contains `--update`, or `SKILLS_AUTO_UPDATE: true` is set in your project CLAUDE.md.*

```bash
npx --yes skills update edge-cases -y 2>/dev/null || true
```

If the skill was updated, stop here and tell the user: **"This skill was just updated. Re-run your command to use the new version."** Otherwise continue silently.

## Phase 1: Catalog

Spawn **8 parallel subagents**, one per category. Each enumerates every edge case it can find in the target area. Return a numbered list of cases with a one-line description and a risk rating (LOW / MEDIUM / HIGH / CRITICAL).

### Category assignments

| Subagent | Category | What to look for |
|----------|----------|-----------------|
| 1 | **Boundary values** | min/max, zero, one, empty, off-by-one, integer overflow/underflow, float precision |
| 2 | **Null / missing / undefined** | null inputs, undefined fields, missing keys, unset env vars, absent config |
| 3 | **Invalid input types & formats** | wrong type coercions, malformed strings, bad dates/times, invalid enums, unexpected encoding |
| 4 | **Error states & exception paths** | network failures, timeouts, partial writes, disk full, DB unavailable, external service 500s |
| 5 | **Concurrency & ordering** | race conditions, double-submit, out-of-order events, stale reads after writes, cache invalidation |
| 6 | **Large / adversarial data** | very large payloads, deeply nested objects, extremely long strings, binary/emoji/RTL in text fields, injection attempts |
| 7 | **State machine violations** | calling operations in wrong order, acting on deleted/expired/cancelled resources, re-entrancy |
| 8 | **Auth & permission boundaries** | unauthenticated access, cross-tenant data leakage, privilege escalation, token expiry mid-request |

Each subagent must read the relevant source files before enumerating: do not guess based on category alone.


## Phase 2: Prioritize & Deduplicate

Consolidate all findings into a single master list:

1. Deduplicate near-identical cases (keep the more specific one)
2. Sort by **risk × likelihood**:
   - **P0**: CRITICAL risk or near-certain to occur in production
   - **P1**: HIGH risk, plausible in production
   - **P2**: MEDIUM risk, possible but uncommon
   - **P3**: LOW risk, theoretical

Present the prioritized list to the user and confirm before writing any tests. Note which cases are already handled and which are unhandled gaps.


## Phase 3: Write Tests

For each unhandled P0 and P1 case (and any P2/P3 cases the user flags):

1. Write a focused test that **proves** the edge case is handled correctly
2. Follow the existing test patterns in the repo (read the test files first)
3. Name each test so its failure message clearly identifies the edge case
4. Group tests by category using describe blocks or equivalent

**Rules:**
- Tests must be runnable without manual setup
- Do not mock behavior that the real code exercises: only mock external I/O (network, DB, filesystem) if needed
- Each test should assert the *correct* outcome, not just that no exception is thrown

Run the tests immediately after writing them. A failing test is the goal for unhandled cases: it proves the gap is real.


## Phase 4: Harden

For every test that fails (unhandled edge case confirmed):

1. Identify the minimal code change needed to handle it correctly
2. Implement the fix: no speculative hardening beyond what the failing test requires
3. Re-run the test: it must pass before moving on
4. Verify no existing tests regress after each fix

Do not batch fixes. Fix → test → fix → test. One at a time.


## Phase 5: Full Test Run & Coverage Report

Run the full test suite using the project's configured test command (check `package.json` scripts or the test runner the repo already uses; e.g. `bun test`, `bun run test`, `npm test`, `pnpm test`, `pytest`, `go test ./...`):

```bash
# Example for a Bun project — substitute the project's actual command
bun test
```

All tests, original and new, must pass. If any pre-existing test is now failing, treat it as a regression and fix it before proceeding.

Summarize:
- How many edge cases were found (by category and priority)
- How many were already handled vs. newly hardened
- How many new tests were written
- Any P2/P3 cases intentionally deferred (with rationale)


## Phase 6: Document Remaining Gaps

For any edge cases intentionally **not** handled (deferred P2/P3 or out-of-scope), create a brief tracking comment or TODO in the relevant source file:

```
// Edge case not handled: <description>. Risk: LOW. Deferred because: <reason>.
```

Do not write TODO comments for cases that are now handled: they just add noise.


## Completion Checklist

- [ ] All 8 edge case categories searched by dedicated subagent
- [ ] Master list deduplicated and prioritized
- [ ] User confirmed scope before test writing
- [ ] Tests written for all P0 and P1 unhandled cases
- [ ] All new tests initially fail (gap confirmed) then pass (fix verified)
- [ ] No regressions in existing test suite
- [ ] Remaining gaps documented inline
