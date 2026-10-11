//! Prompts for each role. The task doc format is the contract between roles.

use crate::claude::BuildSummary;

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

pub fn refine(slug: &str) -> String {
    format!(
        r#"You are the Analyst in a Harness run, continuing an analysis. Analysis only: do not modify any source file.

Read `.ai-tasks/{slug}.md`. The human has answered some Open questions directly in the doc
(an answer follows the question on the same line, or is written under it).

Update the doc in place:
- Mark each answered question as `Decided: <answer>` and keep it in Open questions.
- Re-check the repository where an answer changes the picture, and update Analysis, Conclusion,
  Proposed fix and Acceptance criteria to match the decisions.
- Unanswered questions stay open. Add new questions only if the answers raise them.
- Set the Status line to `**Status:** Ready for implementation` when no open questions remain,
  otherwise keep `**Status:** Analysis`.
- Keep every other section and the overall format unchanged.

Rules:
- Run one plain command per Bash call: no pipes, `&&`, `;`, redirects or subshells.
- Never run git commands that change state (commit, push, reset, stash, checkout, rebase).
- If a tool call is denied, read the reason and continue without it.
- Finish with one sentence per decision you applied.
"#
    )
}

const COMMON_RULES: &str = "Rules:
- Run one plain command per Bash call: no pipes, `&&`, `;`, redirects or subshells.
- Never run git commands that change state (commit, push, reset, stash, checkout, rebase). Harness owns git.
- If a tool call is denied, read the reason and continue without it.";

pub fn implement(slug: &str, round: u32, build_command: Option<&str>) -> String {
    let build = match build_command {
        Some(cmd) => format!("When you finish, Harness runs `{cmd}` itself and the reviewer sees the result. You may run it yourself to check your work."),
        None => "No build command is configured; check your work with the tests the task doc names.".to_string(),
    };
    format!(
        r#"You are the Implementer in a Harness run (round {round}).

Read `.ai-tasks/{slug}.md` and implement it:
- If the Review log has open findings (`- [ ] R<N>-<k> …`), fix each one. After fixing one, tick it (`- [x]`)
  and add an indented line right under it: `  Fixed: <what you changed>`. Skip findings already marked Verified.
- Otherwise implement the Proposed fix, step by step, within the task's scope.
- Update the `## Implementation notes` section: what you changed, where, and where you deviated from the plan and why.
- Do not change the `**Status:**` line. Harness sets it.
- Do not edit `.ai-tasks/policy.toml` or anything under `.ai-tasks/runs/`.

{build}

{COMMON_RULES}

Finish with a short summary of what you changed.
"#
    )
}

pub fn review(slug: &str, round: u32, diff_path: &str, build: Option<&BuildSummary>) -> String {
    let date = crate::journal::today();
    let build = match build {
        Some(b) => format!(
            "Harness ran the build `{}`: **{}**. Last lines of its output:\n```\n{}\n```",
            b.command,
            if b.ok { "green" } else { "RED" },
            b.tail
        ),
        None => "No build command is configured, so Harness did not run a build.".to_string(),
    };
    format!(
        r#"You are the Reviewer in a Harness run (round {round}). You start fresh: you did not write this change.
Do not modify any source file; you may edit only `.ai-tasks/{slug}.md`.

Review the change for `.ai-tasks/{slug}.md`:
- The full change since implementation started (new files included) is in `{diff_path}`. Read it.
- Check it against the Problem, Proposed fix, Acceptance criteria and Implementation notes. Read source files as needed.
- {build}

Update the task doc:
- For findings from earlier reviews that the implementer ticked, append ` Verified ✔` if the fix holds,
  or untick them and append ` Reopened: <reason>`.
- Under `## Review log`, append a section `### Review {round} ({date})` with new findings as a checklist with stable IDs:
  `- [ ] R{round}-<k> [Blocker|Major|Minor] what's wrong (path:line) - expected: …`
  If there are no new findings, write `No new findings.`
- Tick the acceptance criteria that are met.
- Set the Status line: `**Status:** Done` only if the build is green (or none is configured) and no Blocker or Major finding is open;
  otherwise `**Status:** Changes requested`. Never approve a red build.

{COMMON_RULES}

Finish with one line: the verdict and the number of open findings by severity.
"#
    )
}
