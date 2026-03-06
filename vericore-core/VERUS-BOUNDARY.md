# VeriCore Verus Proof Boundary

## What Verus proves

1. **Label algebra** — ContextTier, IntegrityTier, FlowLabel::join.
   Secrecy monotonicity and integrity monotonicity.

2. **Mindlock artifact state machine** — states: In, Work, Out, PendingZar, Rejected, Promoted(target).
   Only allowed transitions occur; no silent jump from staged to promoted.

3. **Boundary approval invariant** — for any Promoted(target) artifact, one of:
   - valid review receipt exists and hash/target match, or
   - explicit Zar approval provenance exists with audit entry.

4. **Ingress integrity invariant** — for non-trusted ingress:
   - no direct Exec,
   - no direct private/sensitive WriteFile,
   - except allowed staging into mindlock quarantine.

## What Verus models as external oracle

Reviewer/LLM decisions are an assumed interface, not something derived from prompts.

Verus proves:
- if decision = Allow and receipt is recorded -> promotion rules are enforced
- if decision = RequestRevision -> artifact stays staged (in/out/work)
- if decision = RequireZarApproval -> artifact moves to PendingZar
- if decision = Reject -> artifact moves to Rejected

Verus does NOT prove:
- the reviewer made the morally or semantically correct choice
- prompt text sufficiency
- RAG relevance or quality

## Not in scope

- Telegram UX correctness
- Prompt wording or style
- Social policy or agent personality
- tmux bridge or future transport layers
