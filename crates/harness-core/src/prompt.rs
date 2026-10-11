//! Prompts for each role. The task doc format is the contract between roles.

pub fn analyze(slug: &str, ticket: &str) -> String {
    format!(
        r#"You are the Analyst in a Harness run. Analysis only: do not modify any source file.

Investigate the repository in the current directory and write the task doc `.ai-tasks/{slug}.md`.
The doc must be self-contained: the implementer never sees this conversation.

Use exactly these sections:

# {slug} — <short title>

**Status:** Analysis

## Problem
## Analysis            (root cause / current behavior, with `path:line` evidence)
## Conclusion
## Proposed fix        (ordered steps, files to change, intent; short snippets only where precision matters)
## Acceptance criteria (testable `- [ ]` checklist)
## Test plan
## Out of scope / constraints
## Open questions      (for the human)
## Implementation notes
## Review log

Rules:
- Run one plain command per Bash call: no pipes, `&&`, `;`, redirects or subshells.
- Never run git commands that change state (commit, push, reset, stash, checkout, rebase).
- If a tool call is denied, read the reason and continue without it.
- Finish with a two-sentence summary of your conclusion.

Ticket:
{ticket}
"#
    )
}
