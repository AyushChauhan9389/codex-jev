# codex-jev

[OpenAI Codex](https://github.com/openai/codex) `rust-v0.160.0` with
[fast-jev-compaction](https://github.com/tamaratran/fast-jev-compaction) wired into its
compaction, so a ChatGPT-subscription session prunes stale tool calls verbatim instead of
summarizing them.

## What changed from upstream

- `codex-rs/core/src/compact_jev.rs` (new): pipes the history to a helper, rebuilds it from the
  original items, and rejects any answer that would leave a tool call without its output (or the
  reverse).
- `codex-rs/core/src/compact_remote_v2.rs`: tries Jev pruning first in the remote (ChatGPT login)
  compaction path; on any failure, timeout, or a cut below the minimum it falls through to the
  stock `/responses/compact` summary.
- `jev/codex-jev-compact.ts`: the helper. Maps Codex response items to Jev messages, asks Jev
  which calls and results are still needed, and drops a reasoning item together with the calls it
  led to. Reasoning is never sent to Jev.
- `jev/lib/`: the fast-jev-compaction library (MIT, see `jev/lib/LICENSE`), vendored at
  `e3f262a` plus upstream PR #110 (no lone surrogates in truncated results).

Without `CODEX_JEV_COMPACT` set, the binary behaves exactly like stock Codex.

## Use

Needs [bun](https://bun.sh) and a TypeSafe key in `TYPESAFE_API_KEY` (or in the `env` block of
`~/.claude/settings.json`, where the Claude Code plugin keeps it).

```sh
cd codex-rs && cargo build --release -p codex-cli --bin codex
ln -s "$PWD/../jev/codex-jev" ~/.local/bin/codex-jev   # CLI with pruning on
```

The Linux ChatGPT desktop app runs `codex app-server` and honors `CODEX_CLI_PATH`;
`jev/chatgpt-jev.desktop` launches it against this build (edit the paths for your home, quit the
running app first). The fork must match the Codex version the app bundles
(`/usr/lib/chatgpt/resources/codex --version`).

Knobs: `JEV_MIN_REDUCTION` (default `0.25`) for the helper; Codex's own
`model_auto_compact_token_limit` decides when compaction runs. Keep that limit well above the
base context, or a prune that stays over it triggers another compaction straight away.
