import { useState } from "react";
import type { Proposal } from "../api";
import { spanLabel } from "../format";

interface Props {
  proposal: Proposal;
  busy: boolean;
  onConfirm: (items: number[]) => void;
  onDismiss: () => void;
}

const STATE_LABEL = { added: "In calendar", confirmed: "Saved to Today", skipped: "Left out", proposed: "" };

/** Time blocks Claude proposed, to confirm before anything reaches the calendar. */
function ProposalCard({ proposal, busy, onConfirm, onDismiss }: Props) {
  const [chosen, setChosen] = useState(() => proposal.items.map(() => true));
  const pending = proposal.status === "pending";
  const selected = chosen.flatMap((on, i) => (on ? [i] : []));

  return (
    <div className={`card proposal ${proposal.status}`}>
      <div className="card-title">{pending ? "Proposed schedule" : proposal.status === "dismissed" ? "Schedule" : "Scheduled"}</div>
      <ul className="card-list">
        {proposal.items.map((item, i) => (
          <li key={i}>
            <label>
              {pending && (
                <input
                  type="checkbox"
                  checked={chosen[i]}
                  disabled={busy}
                  onChange={() => setChosen((c) => c.map((on, j) => (j === i ? !on : on)))}
                />
              )}
              <span className="proposal-item">
                <span className="proposal-title">{item.title}</span>
                <span className="proposal-time">{spanLabel(item.start, item.end)}</span>
              </span>
              {!pending && (
                <span className={`proposal-state ${proposal.states[i]}`}>
                  {STATE_LABEL[proposal.states[i]]}
                </span>
              )}
            </label>
          </li>
        ))}
      </ul>
      {pending ? (
        <div className="card-actions">
          <button
            className="button primary"
            disabled={busy || selected.length === 0}
            onClick={() => onConfirm(selected)}
          >
            {selected.length === proposal.items.length
              ? "Add to calendar"
              : `Add ${selected.length} to calendar`}
          </button>
          <button className="button ghost" disabled={busy} onClick={onDismiss}>
            Not now
          </button>
        </div>
      ) : (
        proposal.status === "dismissed" && <div className="proposal-note">Not scheduled</div>
      )}
    </div>
  );
}

export default ProposalCard;
