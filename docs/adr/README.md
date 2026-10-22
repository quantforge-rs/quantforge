# Architecture decision records

Decisions that shape more than one change, with the context and the
evidence they were made on, so that later work can follow them or knowingly
overturn them.

| ADR | Decision | Status |
| --- | --- | --- |
| [0001](0001-postgres-client-strategy.md) | PostgreSQL client strategy for the synchronous storage ports | Accepted |
| [0002](0002-automation-architecture.md) | Automation architecture and futures capability contract for v0.3 | Accepted |

## Adding one

Copy the outline below into `NNNN-short-title.md`, numbered after the last
record, and add a row to the table above in the same change. A record is
not edited once accepted: a later one supersedes it, and both say so in
their status.

```markdown
# ADR NNNN: Title

- Status: Proposed | Accepted | Superseded by [NNNN](NNNN-title.md)
- Date: YYYY-MM-DD
- Ticket: ALG-NNN

## Context

What forces a decision, with the facts it rests on.

## Decision drivers

## Considered options

## Decision

## Evidence

What was built or measured to back the decision, and how to repeat it.

## Consequences

## Follow-ups
```
