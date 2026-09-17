import { useState } from "react";
import { dismissSection, pinSection, type SectionIdea } from "../api";
import Icon from "../ui/Icon";
import { iconForTag } from "../ui/entity";

interface Props {
  ideas: SectionIdea[];
  onChange: () => void;
}

function members(idea: SectionIdea) {
  const shown = idea.members.slice(0, 3).join(", ");
  const more = idea.count - Math.min(idea.count, 3);
  return more > 0 ? `${shown} and ${more} more` : shown;
}

/** Offers sidebar sections for new kinds of pages, e.g. "Students" after the first student. */
function SectionCard({ ideas, onChange }: Props) {
  const [chosen, setChosen] = useState(() => ideas.map(() => true));
  const [titles, setTitles] = useState(() => ideas.map((i) => i.title));
  const [state, setState] = useState<"open" | "added" | "declined" | "busy">("open");
  const [error, setError] = useState<string | null>(null);

  async function answer(add: boolean) {
    setState("busy");
    setError(null);
    try {
      for (const [i, idea] of ideas.entries()) {
        if (add && chosen[i]) await pinSection(idea.tag, titles[i].trim() || idea.title);
        else await dismissSection(idea.tag);
      }
      setState(add && chosen.some(Boolean) ? "added" : "declined");
      onChange();
    } catch (err) {
      setError(String(err));
      setState("open");
    }
  }

  if (state === "added" || state === "declined") {
    const added = ideas.flatMap((_, i) => (state === "added" && chosen[i] ? [titles[i]] : []));
    return (
      <div className="card card-done">
        <Icon name={added.length ? "check" : "close"} size={14} />
        <span>{added.length ? `Added to your sidebar: ${added.join(", ")}` : "Sidebar left as it is"}</span>
      </div>
    );
  }

  return (
    <div className="card">
      <div className="card-title">
        {ideas.length === 1 ? "Add a section to your sidebar?" : "Add sections to your sidebar?"}
      </div>
      <ul className="card-list">
        {ideas.map((idea, i) => (
          <li key={idea.tag}>
            <label>
              <input
                type="checkbox"
                checked={chosen[i]}
                disabled={state === "busy"}
                onChange={() => setChosen((c) => c.map((on, j) => (j === i ? !on : on)))}
              />
              <span className="section-icon">
                <Icon name={iconForTag(idea.tag)} size={15} />
              </span>
              <span className="card-item">
                <input
                  className="inline-title"
                  value={titles[i]}
                  aria-label="Section name"
                  disabled={state === "busy"}
                  onChange={(e) => {
                    const value = e.currentTarget.value;
                    setTitles((t) => t.map((old, j) => (j === i ? value : old)));
                  }}
                />
                <span className="card-sub">{members(idea)}</span>
              </span>
            </label>
          </li>
        ))}
      </ul>
      {error && <div className="form-error">{error}</div>}
      <div className="card-actions">
        <button className="button primary" disabled={state === "busy" || !chosen.some(Boolean)} onClick={() => answer(true)}>
          Add
        </button>
        <button className="button ghost" disabled={state === "busy"} onClick={() => answer(false)}>
          Not now
        </button>
      </div>
    </div>
  );
}

export default SectionCard;
