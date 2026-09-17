import { openUrl, search, type Activity, type Entity } from "../api";
import { dateLabel } from "../format";
import Avatar from "../ui/Avatar";
import Icon from "../ui/Icon";

interface Props {
  activity: Activity;
  /** Shown when listing several people's updates. */
  person?: Entity;
  /** Marked as new: not seen before this list opened. */
  fresh?: boolean;
  onOpen: (id: string) => void;
  onError: (error: string) => void;
}

const KIND: Record<Activity["kind"], { label: string; icon: string }> = {
  paper: { label: "Paper", icon: "document" },
  post: { label: "Post", icon: "rss" },
  page_change: { label: "Page update", icon: "globe" },
};

const RELEVANCE: Record<string, string> = {
  high: "Highly relevant",
  medium: "Relevant",
  low: "Loosely related",
  none: "Not related",
};

/** One paper, post or page change, with why it matters to the user's work. */
function UpdateItem({ activity: a, person, fresh, onOpen, onError }: Props) {
  const kind = KIND[a.kind] ?? KIND.post;
  // Related pages are stored by name; open the page with exactly that name.
  const openRelated = (name: string) =>
    search(name).then((hits) => {
      const hit = hits.find((h) => h.entity.name.toLowerCase() === name.toLowerCase());
      if (hit) onOpen(hit.entity.id);
    }, (err) => onError(String(err)));

  return (
    <li className={`update${fresh ? " fresh" : ""}`}>
      <div className="update-head">
        {person && (
          <button className="update-person" onClick={() => onOpen(person.id)}>
            <Avatar entity={person} size={20} />
            {person.name}
          </button>
        )}
        <span className="update-kind">
          <Icon name={kind.icon} size={12} />
          {kind.label}
        </span>
        {a.published && <span className="update-date">{dateLabel(a.published)}</span>}
        {fresh && <span className="update-dot" aria-label="New" />}
      </div>
      <button className="update-title" onClick={() => openUrl(a.url).catch((err) => onError(String(err)))}>
        {a.title}
        <Icon name="external" size={12} />
      </button>
      {a.summary && <p className={`update-summary${a.kind === "page_change" ? " changes" : ""}`}>{a.summary}</p>}
      {(a.relevance || !a.baseline) && (
        <div className="update-why">
          {a.relevance ? (
            <span className={`relevance ${a.relevance}`}>{RELEVANCE[a.relevance]}</span>
          ) : (
            <span className="relevance pending">Not rated yet</span>
          )}
          {a.reason && <span className="update-reason">{a.reason}</span>}
          {a.related.map((name) => (
            <button key={name} className="task-chip plain" onClick={() => openRelated(name)}>
              {name}
            </button>
          ))}
        </div>
      )}
    </li>
  );
}

export default UpdateItem;
