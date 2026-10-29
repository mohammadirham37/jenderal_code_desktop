/** Token usage and context-window fill (as a percentage) for the active
 *  session, shown in the composer toolbar. */
import { useMemo, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { useChat } from "@/stores/chat";
import { useProviders } from "@/stores/providers";
import type { MessageRecord, ModelInfo } from "@/lib/ipc";
import { formatCost, formatTokens } from "@/lib/format";

/** Typical context windows, used only when the provider doesn't report one. */
const KNOWN_WINDOWS: [RegExp, number][] = [
  [/claude/i, 200_000],
  [/gpt-5|o3|o4/i, 400_000],
  [/gpt-4\.1/i, 1_000_000],
  [/gpt-4o|gpt-4-turbo/i, 128_000],
  [/gemini-(1\.5|2|3)/i, 1_000_000],
  [/deepseek/i, 128_000],
  [/llama-?3\.[1-3]|llama-?4/i, 128_000],
  [/qwen/i, 32_768],
  [/mistral-large|codestral|devstral/i, 128_000],
  [/grok/i, 256_000],
];
const DEFAULT_WINDOW = 128_000;

export function contextWindowFor(model: ModelInfo | undefined, name: string | null | undefined) {
  const reported = model?.capabilities.context_window;
  if (reported && reported > 0) return { tokens: reported, known: true };
  const match = name ? KNOWN_WINDOWS.find(([re]) => re.test(name)) : undefined;
  return { tokens: match?.[1] ?? DEFAULT_WINDOW, known: false };
}

/** Rough token estimate (~4 chars/token) for when no usage was reported yet. */
function estimateTokens(messages: MessageRecord[]) {
  let chars = 0;
  for (const m of messages) {
    chars += m.content.length;
    for (const c of m.tool_calls) chars += c.arguments.length;
  }
  return Math.round(chars / 4);
}

export function ContextMeter() {
  const { usage, messages, sessions, activeSessionId } = useChat(
    useShallow((s) => ({
      usage: s.usage,
      messages: s.messages,
      sessions: s.sessions,
      activeSessionId: s.activeSessionId,
    })),
  );
  const modelsOf = useProviders((s) => s.modelsOf);
  const [open, setOpen] = useState(false);

  const session = sessions.find((s) => s.id === activeSessionId);
  const model = session?.provider_id
    ? modelsOf(session.provider_id).find((m) => m.name === session.model_name)
    : undefined;
  const ctxWindow = contextWindowFor(model, session?.model_name);
  const total = (usage?.total_input ?? 0) + (usage?.total_output ?? 0);

  const reported = (usage?.requests ?? 0) > 0;
  const estimated = useMemo(() => (reported ? 0 : estimateTokens(messages)), [reported, messages]);
  const used = reported ? usage!.last_input + usage!.last_output : estimated;
  const pct = Math.min(100, (used / ctxWindow.tokens) * 100);
  const pctLabel = pct > 0 && pct < 1 ? "<1%" : `${Math.round(pct)}%`;

  const tone = pct >= 85 ? "text-red-500" : pct >= 60 ? "text-amber-500" : "text-indigo-400";
  const stroke = pct >= 85 ? "#ef4444" : pct >= 60 ? "#f59e0b" : "#818cf8";
  const r = 6;
  const circ = 2 * Math.PI * r;

  return (
    <div className="relative" onMouseEnter={() => setOpen(true)} onMouseLeave={() => setOpen(false)}>
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex items-center gap-1 rounded-md px-1.5 py-1 text-[11px] text-neutral-500 hover:bg-neutral-100 dark:hover:bg-neutral-800"
        aria-label={`Context ${pctLabel} used`}
      >
        <svg width="16" height="16" viewBox="0 0 16 16" className="-rotate-90">
          <circle cx="8" cy="8" r={r} fill="none" strokeWidth="2.2" className="stroke-neutral-300 dark:stroke-neutral-700" />
          <circle
            cx="8"
            cy="8"
            r={r}
            fill="none"
            stroke={stroke}
            strokeWidth="2.2"
            strokeLinecap="round"
            strokeDasharray={circ}
            strokeDashoffset={circ * (1 - pct / 100)}
          />
        </svg>
        <span className={`tabular-nums font-medium ${tone}`}>
          {reported ? "" : "≈"}
          {pctLabel}
        </span>
        {total > 0 && <span className="hidden tabular-nums sm:inline">· {formatTokens(total)} tok</span>}
      </button>
      {open && (
        <div className="absolute bottom-full right-0 z-40 mb-1 w-64 rounded-lg border border-neutral-200 bg-white p-3 text-[11px] shadow-xl dark:border-neutral-700 dark:bg-[#15171d]">
          <div className="mb-1.5 flex items-center justify-between font-semibold">
            <span>Context window</span>
            <span className={tone}>{pctLabel}</span>
          </div>
          <div className="mb-2 h-1.5 overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-800">
            <div className="h-full rounded-full transition-all" style={{ width: `${pct}%`, background: stroke }} />
          </div>
          <Row label="Used" value={`${reported ? "" : "≈"}${formatTokens(used)} / ${formatTokens(ctxWindow.tokens)}`} />
          {!ctxWindow.known && (
            <div className="mb-1.5 text-[10px] leading-snug text-neutral-500">
              Window size not reported by the provider — using a typical value for this model.
            </div>
          )}
          {!reported && (
            <div className="mb-1.5 text-[10px] leading-snug text-neutral-500">
              Estimated from message length until the provider reports usage.
            </div>
          )}
          <div className="my-2 border-t border-neutral-200 dark:border-neutral-800" />
          <div className="mb-1 font-semibold">This conversation</div>
          <Row label="Input tokens" value={formatTokens(usage?.total_input ?? 0)} />
          <Row label="Output tokens" value={formatTokens(usage?.total_output ?? 0)} />
          <Row label="Model requests" value={String(usage?.requests ?? 0)} />
          <Row label="Est. cost" value={formatCost(usage?.total_cost)} />
          {pct >= 85 && (
            <div className="mt-2 rounded bg-red-500/10 px-2 py-1 text-[10px] text-red-500">
              Context is almost full — older messages will be trimmed. Consider starting a new conversation.
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-center justify-between py-0.5">
      <span className="text-neutral-500">{label}</span>
      <span className="tabular-nums font-medium">{value}</span>
    </div>
  );
}
