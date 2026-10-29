# How Rackett was built

Rackett was written almost entirely by AI coding agents, directed by its human owner,
[@Vladimir-0001](https://github.com/Vladimir-0001), over 27–29 October 2026.

## Who did the work

| Contributor | Role |
| --- | --- |
| **@Vladimir-0001** (human) | Started and directed the project, set priorities, made product decisions, approved publication |
| **Claude Opus 5.5** (Anthropic) | Lead and integrator agents, engine work, reviews, merges, documentation |
| **GPT-6.1-Sol** (OpenAI Codex) | Engine features, audits of Jackett's source, backend and frontend work, reviews |
| **GPT-6-Luna** (OpenAI Codex) | Small, fast tasks, mostly fixtures and test cases for individual tracker sites |
| **GPT-6-Astra** (OpenAI Codex) | One focused task |

Agent sessions, from the orchestration records:

| Model | Agent threads |
| --- | --- |
| Claude Opus 5.5 | 53 |
| GPT-6.1-Sol | 32 |
| GPT-6-Luna | 20 |
| GPT-6-Astra | 1 |
| **Total** | **106** (103 agent-started, 3 started by the owner) |

## How the agents worked together

- A **lead agent** split the work into small, independent tasks and gave each to
  a separate agent working in its own git worktree. Later in the project, each
  agent also had its own restricted account.
- Agents reserved the exact files they would change, so parallel work did not
  collide.
- **Small agents** each took a handful of tracker sites. They wrote a synthetic
  copy of each site's pages from Jackett's pinned definition, and a test case
  proving login, search, caching and downloads end to end.
- The **integrator agent** reviewed and merged each contribution, fixed what the
  combined test run caught, and did the larger engine work itself. For example, it
  added legacy Windows/ISO character encodings, found and fixed a double-decoding
  bug, and fixed flaky test deadlines.
- Changes reached the main branch only after automated checks (format, lint and
  the full test suite) passed on a CI server.

## What this means for you

- The code is AI-generated. It is tested extensively against simulated sites
  (several hundred automated tests), but it has not been line-by-line reviewed by a
  human. Treat it as an early preview.
- Rackett builds on [Jackett](https://github.com/Jackett/Jackett): the site
  definitions are the work of Jackett's human community, used unchanged under
  GPL-2.0. See [NOTICE](NOTICE).

This repository starts from a single squashed commit, so this file is the record of
the AI work. The development history lives in a separate private repository.
