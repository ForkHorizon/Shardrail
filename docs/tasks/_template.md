# Task <ID>: <Short Title>

- **Repository**: `Shardrail_Core` / `Shardrail_Web`
- **Branch**: `task/<id>-<slug>`
- **Author/Agent**: <Agent ID>
- **Date**: YYYY-MM-DD
- **Status**: Draft / In Progress / Completed

## 1. Research & Context
- **Official Documentation**: <Links / citations>
- **Agent / Tooling Versions**: <Specific installed versions tested>
- **Established Facts**:
  - <Bullet points of verified runtime behavior>
- **Unknowns / Hypotheses**:
  - <Things to be verified during development>

## 2. Design & Acceptance Criteria
- **Scope & Changes**:
  - <Minimal set of changes to be implemented>
- **Acceptance Criteria**:
  - [ ] Criterion 1
  - [ ] Criterion 2
- **Decisions Required from Owner**:
  - <Explicit list of choices; do not guess>

## 3. Implementation Summary
- **Files Modified / Added**:
  - `<path>`: <Description>
- **Architecture & Invariants**:
  - <How contracts, append-only journal, or fail-closed rules are preserved>

## 4. Tests & Verification Evidence
- **Automated Test Results**:
  - Unit / Integration tests: `<command>` -> `<result>`
  - Format / Clippy / Lints: `<command>` -> `<result>`
  - Dependency / License checks: `<command>` -> `<result>`
- **Live / Fixture Verification**:
  - Fake agent / testrepo runs: `<evidence>`

## 5. Self-Review & Unverified Scope
- **Review Checklist**:
  - [ ] Security (no leaked secrets, fail-closed approvals)
  - [ ] Error handling (honest failure modes, no silent swallows)
  - [ ] No extraneous refactors or drive-by edits
- **Explicitly Unverified**:
  - <List anything not verified locally, and why>

## 6. Deployment / Smoke Verification
- **Binary / Build Installed**: `<path or version>`
- **Smoke Test Command**: `<command>`
- **Smoke Test Output**: `<result>`

## 7. Plan & Git Status
- **Commits**: `<hashes and summaries>`
- **PR**: `<PR link/number>`
- **Tag**: `<tag name if applicable>`
- **PLAN.md Updated**: Yes / No
