import { useEffect, useRef, useState } from "react";
import { listTagged, listUpdates, markUpdatesSeen, type Entity, type Update } from "../api";
import UpdateItem from "../updates/UpdateItem";
import Avatar from "../ui/Avatar";
import Icon from "../ui/Icon";

interface Props {
  version: number;
  onOpen: (id: string) => void;
}

/**
 * What people the user follows have published or changed. Relevant shows what Claude judged
 * related to their research areas, projects and ideas; Everything shows all new items.
 */
function UpdatesView({ version, onOpen }: Props) {
  const [all, setAll] = useState(false);
  const [updates, setUpdates] = useState<Update[] | null>(null);
  const [following, setFollowing] = useState<Entity[]>([]);
  const [error, setError] = useState<string | null>(null);
  // Items unread when the view opened keep their "new" mark until it closes.
  const fresh = useRef(new Set<string>());

  useEffect(() => {
    let cancelled = false;
    listUpdates(null, all).then((result) => {
      if (cancelled) return;
      for (const u of result) if (!u.activity.seen) fresh.current.add(u.activity.id);
      setUpdates(result);
      const unseen = result.filter((u) => !u.activity.seen).map((u) => u.activity.id);
      if (unseen.length) markUpdatesSeen(unseen).catch(() => {});
    }, (err) => setError(String(err)));
    listTagged("following").then((people) => !cancelled && setFollowing(people), () => {});
    return () => {
      cancelled = true;
    };
  }, [all, version]);

  return (
    <div className="page">
      <div className="page-header">
        <div>
          <h1>Updates</h1>
          <p className="muted">
            {following.length === 0
              ? "From people you follow"
              : `From ${following.length} ${following.length === 1 ? "person" : "people"} you follow, checked every few hours`}
          </p>
        </div>
        <div className="segmented" role="tablist">
          <button role="tab" aria-selected={!all} className={!all ? "on" : ""} onClick={() => setAll(false)}>
            Relevant
          </button>
          <button role="tab" aria-selected={all} className={all ? "on" : ""} onClick={() => setAll(true)}>
            Everything
          </button>
        </div>
      </div>

      {error && <p className="form-error">{error}</p>}

      {following.length > 0 && (
        <div className="chips following-strip">
          {following.map((p) => (
            <button key={p.id} className="task-chip" onClick={() => onOpen(p.id)}>
              <Avatar entity={p} size={16} />
              {p.name}
            </button>
          ))}
        </div>
      )}

      {updates && updates.length === 0 && (
        <div className="empty">
          <Icon name="bell" size={20} />
          {following.length === 0 ? (
            <p>
              Follow researchers to hear when they publish or post something related to your work. Open a
              person's page and choose Follow, or tell the chat: "Keep track of Andreas Zeller,
              andreas-zeller.info".
            </p>
          ) : all ? (
            <p>Nothing new yet. What they had already published when you followed them is on their pages.</p>
          ) : (
            <p>
              Nothing related to your work yet. Updates are matched against your research areas, projects and
              ideas, so keep those up to date.
            </p>
          )}
        </div>
      )}

      {updates && updates.length > 0 && (
        <ul className="updates">
          {updates.map((u) => (
            <UpdateItem
              key={u.activity.id}
              activity={u.activity}
              person={u.person}
              fresh={fresh.current.has(u.activity.id)}
              onOpen={onOpen}
              onError={setError}
            />
          ))}
        </ul>
      )}
    </div>
  );
}

export default UpdatesView;
