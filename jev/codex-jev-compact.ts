#!/usr/bin/env bun
// Jev pruning helper for the codex-jev fork (see codex-rs/core/src/compact_jev.rs).
// stdin: the history as a JSON array of Codex response items.
// stdout: {"keep": [index, ...], "replace": {"index": "truncated output"}}.
// Exits 3 when pruning would not cut enough; any other failure exits 1. Either way Codex
// falls back to its regular compaction.
import { readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { compactMessages, reductionRatio, type Message } from './lib/index.ts';

const MIN_REDUCTION = Number(process.env.JEV_MIN_REDUCTION ?? 0.25);

type Item = { type: string; [key: string]: any };
type Call = { id: string; tool: string; input: Record<string, unknown> };

function apiKey(): string | undefined {
  if (process.env.TYPESAFE_API_KEY) return process.env.TYPESAFE_API_KEY;
  // Same key the Claude Code plugin uses.
  try {
    const settings = JSON.parse(readFileSync(join(homedir(), '.claude/settings.json'), 'utf8'));
    return settings.env?.TYPESAFE_API_KEY;
  } catch {
    return undefined;
  }
}

function textOf(content: unknown): string {
  if (typeof content === 'string') return content;
  if (!Array.isArray(content)) return '';
  return content
    .map((part) =>
      typeof part?.text === 'string' ? part.text : String(part?.type).includes('image') ? '[image]' : '',
    )
    .filter(Boolean)
    .join('\n');
}

function inputOf(raw: unknown): Record<string, unknown> {
  if (typeof raw !== 'string') {
    return raw && typeof raw === 'object' && !Array.isArray(raw) ? (raw as Record<string, unknown>) : { value: raw };
  }
  try {
    const value = JSON.parse(raw);
    return value && typeof value === 'object' && !Array.isArray(value) ? value : { value };
  } catch {
    return { input: raw };
  }
}

function callOf(item: Item): Call | undefined {
  switch (item.type) {
    case 'function_call':
      return { id: item.call_id, tool: item.name, input: inputOf(item.arguments) };
    case 'custom_tool_call':
      return { id: item.call_id, tool: item.name, input: inputOf(item.input) };
    case 'local_shell_call':
      return item.call_id ? { id: item.call_id, tool: 'local_shell', input: item.action ?? {} } : undefined;
    case 'tool_search_call':
      return item.call_id ? { id: item.call_id, tool: 'tool_search', input: inputOf(item.arguments) } : undefined;
  }
  return undefined;
}

function outputOf(item: Item): { id: string; text: string } | undefined {
  switch (item.type) {
    case 'function_call_output':
    case 'custom_tool_call_output':
      return item.call_id ? { id: item.call_id, text: textOf(item.output) } : undefined;
    case 'tool_search_output':
      return item.call_id ? { id: item.call_id, text: JSON.stringify(item.tools ?? []) } : undefined;
  }
  return undefined;
}

/** One Jev message per call, output or text item, like Claude Code's transcript. */
function toMessages(items: Item[]): Message[] {
  const messages: Message[] = [];
  for (const item of items) {
    const call = callOf(item);
    if (call) {
      messages.push({ role: 'assistant', text: '', toolUses: [{ tool_use_id: call.id, tool: call.tool, input: call.input }] });
      continue;
    }
    const output = outputOf(item);
    if (output) {
      messages.push({ role: 'user', text: '', toolUses: [], toolResults: [{ tool_use_id: output.id, text: output.text }] });
      continue;
    }
    if (item.type === 'message' || item.type === 'agent_message') {
      const text = textOf(item.content);
      if (text) messages.push({ role: item.role === 'assistant' ? 'assistant' : 'user', text, toolUses: [] });
    } else if (item.type === 'compaction' || item.type === 'context_compaction') {
      messages.push({ role: 'user', text: '[earlier context compacted]', toolUses: [] });
    }
    // Reasoning and other items are never sent to Jev, as in the Claude Code plugin.
  }
  return messages;
}

const items: Item[] = JSON.parse(readFileSync(0, 'utf8'));
const messages = toMessages(items);
const result = await compactMessages(messages, { apiKey: apiKey() });
const ratio = reductionRatio(result);
const { stats } = result;
const summary = `${Math.round(ratio * 100)}% reduction; ${stats.kept} kept, ${stats.resultsDropped} results truncated, ${stats.callsDropped} calls dropped, ${stats.pinned} pinned; ${stats.requests} Jev request(s) in ${stats.ms}ms`;
if (ratio < MIN_REDUCTION) {
  console.error(`below ${MIN_REDUCTION * 100}% minimum: ${summary}`);
  process.exit(3);
}

const original = new Map<string, string>();
for (const message of messages) for (const r of message.toolResults ?? []) original.set(r.tool_use_id, r.text);
const kept = new Set<string>();
const truncated = new Map<string, string>();
for (const message of result.messages) {
  for (const use of message.toolUses) kept.add(use.tool_use_id);
  for (const r of message.toolResults ?? []) {
    kept.add(r.tool_use_id);
    if (r.text !== original.get(r.tool_use_id)) truncated.set(r.tool_use_id, r.text);
  }
}
const seen = new Set(messages.flatMap((m) => [...m.toolUses.map((u) => u.tool_use_id), ...(m.toolResults ?? []).map((r) => r.tool_use_id)]));
const dropped = (id: string | undefined) => id !== undefined && seen.has(id) && !kept.has(id);

const keep: number[] = [];
const replace: Record<string, string> = {};
items.forEach((item, index) => {
  const id = callOf(item)?.id ?? outputOf(item)?.id;
  if (dropped(id)) return;
  if (item.type === 'reasoning') {
    // A reasoning item must not outlive the calls it led to: drop it when every call up to
    // the next reasoning or message item was dropped.
    const calls: string[] = [];
    for (let next = index + 1; next < items.length; next++) {
      const call = callOf(items[next]);
      if (call) calls.push(call.id);
      else if (!outputOf(items[next])) break;
    }
    if (calls.length > 0 && calls.every(dropped)) return;
  }
  keep.push(index);
  const text = id !== undefined && outputOf(item) ? truncated.get(id) : undefined;
  if (text !== undefined && item.type !== 'tool_search_output') replace[index] = text;
});

console.error(summary);
process.stdout.write(JSON.stringify({ keep, replace }));
