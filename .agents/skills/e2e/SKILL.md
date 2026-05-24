---
name: e2e
description: End-to-end test authoring and execution for web, mobile, React Native, or Flutter apps. Discovers user flows, sets up the E2E framework if needed, writes tests covering the golden path and critical edge cases, runs them, and fixes failures. Supports Playwright (web/browser) and Maestro (iOS/Android/React Native/Flutter + web beta). Use when asked to write E2E tests or verify a feature works from the user's perspective.
argument-hint: <feature, flow, or area to cover>
---

# E2E

You are writing end-to-end tests that simulate real user behavior. Work through each phase in order. Do not skip phases.

**Target:** {{args}}


## Phase 0: Auto-Update

*Skip unless `{{args}}` contains `--update`, or `SKILLS_AUTO_UPDATE: true` is set in your project CLAUDE.md.*

```bash
npx --yes skills update e2e -y 2>/dev/null || true
```

If the skill was updated, stop here and tell the user: **"This skill was just updated. Re-run your command to use the new version."** Otherwise continue silently.

## Phase 1: Discover

Spawn **3 parallel subagents** to map what needs testing:

| Subagent | Focus | What to find |
|----------|-------|-------------|
| 1 | **App entry points** | How the app starts, what port/URL it runs on (web) or bundle ID/package name (mobile), how to launch it in test mode, any seed/fixture scripts |
| 2 | **User flows** | Routes, screens, forms, actions, and navigation paths relevant to the target area |
| 3 | **Existing tests** | Current E2E framework (Playwright, Maestro, Cypress, Detox, etc.), test structure, helper utilities, existing coverage gaps |

Synthesize into:
- **Platform**: web browser, iOS, Android, React Native, Flutter, or mixed
- **Launch command**: how to start the app under test
- **Base URL or App ID**: where to point the test runner
- **Flow inventory**: numbered list of user flows to cover, with the UI elements involved
- **Framework decision**: see below

### Framework Decision

If a framework already exists, use it. If not, choose based on platform:

| Platform | Recommended framework | Why |
|----------|-----------------------|-----|
| Web only | **Playwright** | Full browser automation, rich selector API, multi-browser |
| iOS / Android (native) | **Maestro** | Native device automation, no code injection, framework-agnostic |
| React Native | **Maestro** | Works at the UI layer, handles both iOS and Android with one flow |
| Flutter | **Maestro** | Framework-agnostic UI automation, no SDK integration required |
| Web + mobile | Ask the user: Playwright for web, Maestro for mobile, or Maestro for both (web support is beta) |

**Always confirm the framework choice with the user before setup.**


## Phase 2: Framework Setup (if needed)

### Playwright (web)

1. Install: `bun add -d @playwright/test`
2. Install browser: `bunx playwright install --with-deps chromium`
3. Create a minimal `playwright.config.ts` following project conventions
4. Add to `package.json` scripts: `"test:e2e": "playwright test"`
5. Verify the setup by writing one trivial passing test and running `bun run test:e2e` (running with no test files reports "No tests found" rather than success)

### Maestro (mobile / React Native / Flutter)

1. Install the Maestro CLI (requires Java 17+):
   ```bash
   curl -fsSL "https://get.maestro.mobile.dev" | bash
   ```
2. Verify: `maestro --version`
3. Confirm a device or emulator is available: `maestro devices`
4. Create a `maestro/` folder at the project root for flow files
5. Write a trivial smoke flow to `maestro/smoke.yaml` (launch the app and assert it opens), then verify it passes: `maestro test maestro/smoke.yaml`

If a framework already exists, read its config and confirm the test directory and launch settings before proceeding.


## Phase 3: Plan Test Scenarios

For the target area, define test scenarios at two levels:

**Golden path**: the happy path a user follows when everything works:
- List each step as a user action (tap, type, swipe, navigate, submit)
- Define the expected outcome after each step
- One scenario per major flow

**Critical edge cases**: the failures a user will actually hit:
- Empty states (no data, first use)
- Validation errors (bad input, required fields)
- Auth boundaries (logged out, wrong role)
- Error recovery (server error, network failure)
- Navigation correctness after actions

Present the scenario list to the user and confirm scope before writing any tests. Note which flows are already covered by existing tests.


## Phase 4: Write Tests

### Playwright tests (TypeScript)

1. Read similar existing tests first: match their style, helpers, and assertions exactly
2. Use the Page Object pattern if the project already uses it; otherwise write direct locator calls
3. Use **semantic selectors** in priority order:
   - `getByRole` / `getByLabel` / `getByText` (prefer: match what users see)
   - `data-testid` attributes (add them to the source if needed)
   - CSS selectors only as a last resort
4. Assert the **visible outcome** the user would notice: page content, URL, toast message, element state
5. Never assert implementation details (Redux state, internal API calls, class names)

**Rules:**
- Each test must be independent: no shared state between tests
- Flaky assertions must use `waitFor` or explicit waits, never `sleep`
- If a `data-testid` is missing from the UI, add it to the source file in the same PR

### Maestro flows (YAML)

1. One flow file per scenario: `maestro/<feature>-<flow>.yaml`
2. Start every flow with `appId` and `launchApp`:
   ```yaml
   appId: com.example.app
   ---
   - launchApp:
       clearState: true
   ```
3. Use **visible text first** for element targeting:
   ```yaml
   - tapOn: "Sign In"
   - inputText: "user@example.com"
   - tapOn: { id: "submit_button" }   # fall back to ID if text is ambiguous
   - assertVisible: "Welcome back"
   ```
4. Key commands: `tapOn`, `inputText`, `scrollUntilVisible`, `assertVisible`, `assertNotVisible`, `back`, `takeScreenshot`, `swipe`
5. For environment-specific values, pass them with `--env`: `${APP_URL}`, `${TEST_USER}`
6. Assert **visible text or element presence**: never internal state

**Rules:**
- Each flow must be runnable in isolation (`clearState: true` on launch)
- No fixed waits: Maestro handles timing automatically; use `assertVisible` as the sync point
- If an element has no stable text or ID, add a `testID` to the source and target it with `{ id: "..." }`

Run every flow immediately after writing it. A passing golden-path flow is the baseline.


## Phase 5: Fix Failures

For every failing test or flow:

1. Read the error output carefully: distinguish selector mismatch, timing issue, and actual behavior bug
2. **Selector mismatch**: fix the locator or add a `data-testid` / `testID` to the source
3. **Timing issue** (Playwright): replace fixed waits with `waitFor` targeting a visible element or network idle
4. **Element not found** (Maestro): use `maestro hierarchy` to inspect the current screen state and confirm the element ID or text
5. **Actual bug found**: real find. Report it to the user before deciding whether to fix the app or mark the test as known-failing with a TODO

Do not modify assertions to make tests pass: assertions represent the correct expected behavior.

Fix → run → fix → run. One failure at a time.


## Phase 6: Run Full Suite & Confirm

**Playwright:**
```bash
bun run test:e2e
```

**Maestro:**
```bash
maestro test maestro/
```

All tests must pass. If existing E2E tests are now failing, treat them as regressions and fix the cause before reporting done.

Report:
- How many flows are now covered
- How many tests were written (golden path vs. edge cases)
- Any bugs found in the app during testing
- Any flows intentionally deferred (with reason)


## Completion Checklist

- [ ] Platform confirmed (web / iOS / Android / React Native / Flutter)
- [ ] Framework chosen and confirmed with user (Playwright or Maestro)
- [ ] App launch and base URL / app ID confirmed
- [ ] E2E framework installed and configured
- [ ] Flow inventory created and confirmed with user
- [ ] Golden path test written and passing for each major flow
- [ ] Critical edge cases covered with dedicated tests
- [ ] All tests use semantic selectors or `data-testid` / `testID`
- [ ] No flaky waits: Playwright uses `waitFor`; Maestro uses `assertVisible` as sync
- [ ] Full E2E suite passes with no regressions
- [ ] Any app bugs found are reported to the user
