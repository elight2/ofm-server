# Ship Before You Study
* Ship first, refine later: get a usable artifact out, then add detail.
* Check for prior art before writing, once and cheaply: one `ls`/`grep` over the project, `~/.pi/agent/extensions` and `docs/examples`; extend or replace what exists instead of rewriting it.
* Write code before studying: a build/type error answers an unknown API faster than reading dependency sources; study only what the next line needs.
* Never read a manual, bundle or `.d.ts` end to end: grep the symbol and read only that range. This provider shows no cache reads, so whatever you load is re-sent and billed on every later turn; one 50 KB doc costs about +15k tokens per turn for the rest of the session.
* One end-to-end check per deliverable, with the smallest command that answers the question (no full agent run just to look). Finished work stays finished: no re-verification pass, no re-running a test that passed, no fmt/clippy rounds, no repeated command "to confirm".
* Never prove time-based behavior by waiting: no `sleep` beyond a few seconds, no polling a 1-minute timer across cycles; read the code or shorten the interval through an env var.
* Rename, wording or comment edits: apply, confirm with one `grep`, stop; no server restart and no re-test.
* "Just simple / that's enough / stop" closes the task: note new doubts, don't chase them.
