/** Approval card: allow / deny sensitive agent actions. */
import { useState } from "react";
import { Eye, ShieldAlert, FileDiff, Zap } from "lucide-react";
import type { ApprovalRecord } from "@/lib/ipc";
import { Button } from "@/components/ui";
import { useChat } from "@/stores/chat";

export function ApprovalCard({
  approval,
  onOpenDiff,
}: {
  approval: ApprovalRecord;
  onOpenDiff: (key: string, title: string, diff: string) => void;
}) {
  const decide = useChat((s) => s.decide);
  const setPolicy = useChat((s) => s.setPolicy);
  const [busy, setBusy] = useState(false);
  const decided = approval.status !== "pending";

  const answer = async (approved: boolean, grant?: string) => {
    setBusy(true);
    try {
      await decide(approval.id, approved, grant);
    } finally {
      setBusy(false);
    }
  };

  /** Approve this request AND switch the session to full access so the
   *  agent stops asking for the rest of the session. */
  const fullAccess = async () => {
    setBusy(true);
    try {
      await answer(true);
      await setPolicy("yolo");
    } finally {
      setBusy(false);
    }
  };

  const isEdit = ["write_file", "edit_file", "delete_file"].includes(approval.kind);
  const grantKind = isEdit ? "write_files" : "run_commands";

  return (
    <div
      className={`my-2 rounded-lg border p-3 ${
        decided
          ? "border-neutral-200 opacity-60 dark:border-neutral-800"
          : "border-amber-500/40 bg-amber-500/5"
      }`}
    >
      <div className="flex items-start gap-2">
        <ShieldAlert size={15} className="mt-0.5 shrink-0 text-amber-500" />
        <div className="min-w-0 flex-1">
          <div className="text-xs font-semibold">Approval required</div>
          <div className="mt-0.5 break-words text-xs text-neutral-600 dark:text-neutral-300">
            {approval.description}
          </div>
          {approval.preview && (
            <button
              className="mt-1.5 inline-flex items-center gap-1 text-[11px] text-indigo-400 hover:underline"
              onClick={() => onOpenDiff(`approval:${approval.id}`, approval.description, approval.preview!)}
            >
              {approval.preview.startsWith("diff --git") || approval.preview.includes("\n@@") ? (
                <>
                  <FileDiff size={11} /> View diff
                </>
              ) : (
                <>
                  <Eye size={11} /> Preview
                </>
              )}
            </button>
          )}
          {!decided ? (
            <div className="mt-2.5 flex flex-wrap gap-1.5">
              <Button variant="success" size="sm" disabled={busy} onClick={() => void answer(true)}>
                Allow once
              </Button>
              <Button size="sm" disabled={busy} onClick={() => void answer(true, grantKind)}>
                Allow for this session
              </Button>
              <Button
                variant="primary"
                size="sm"
                disabled={busy}
                title="Approve this and stop asking for the rest of the session (git push/pull and sensitive files still ask; destructive commands stay blocked)."
                onClick={() => void fullAccess()}
              >
                <Zap size={11} /> Full access
              </Button>
              <Button variant="danger" size="sm" disabled={busy} onClick={() => void answer(false)}>
                Deny
              </Button>
            </div>
          ) : (
            <div className="mt-2 text-[11px] text-neutral-500">
              {approval.status === "approved" ? "✓ Approved" : "✗ Denied"}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
