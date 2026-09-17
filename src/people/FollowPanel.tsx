import { useEffect, useState } from "react";
import {
  addProfileLinks,
  checkPersonNow,
  followPerson,
  listUpdates,
  openalexCandidates,
  openUrl,
  PROFILE_LINKS,
  removeProfileLink,
  watchInfo,
  type AuthorCandidate,
  type Entity,
  type Update,
  type WatchInfo,
} from "../api";
import { agoLabel } from "../format";
import UpdateItem from "../updates/UpdateItem";
import Icon from "../ui/Icon";

interface Props {
  person: Entity;
  version: number;
  onOpen: (id: string) => void;
  onChanged: () => void;
}

/** Links that don't show as a readable address. */
function linkText(key: string, value: string) {
  if (key === "openalex") return value;
  return value.replace(/^https?:\/\/(www\.)?/, "").replace(/\/$/, "");
}

const linkUrl = (key: string, value: string) => (key === "openalex" ? `https://openalex.org/${value}` : value);

const SHOWN = 6;

/**
 * A person's profile links, and following them: what is checked, choosing their OpenAlex author
 * so papers can be checked, and what they've done recently.
 */
function FollowPanel({ person, version, onOpen, onChanged }: Props) {
  const [watch, setWatch] = useState<WatchInfo | null>(null);
  const [activity, setActivity] = useState<Update[]>([]);
  const [draft, setDraft] = useState<string | null>(null);
  const [candidates, setCandidates] = useState<AuthorCandidate[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [showAll, setShowAll] = useState(false);

  const load = () => {
    watchInfo(person.id).then(setWatch, (err) => setError(String(err)));
    listUpdates(person.id, true).then(setActivity, () => {});
  };
  useEffect(load, [person.id, person.updated_at, version]);

  if (!watch) return null;
  const first = person.name.split(/\s+/)[0];
  const links = PROFILE_LINKS.filter((l) => person.info[l.key]);

  async function run(label: string, action: () => Promise<unknown>) {
    setBusy(label);
    setError(null);
    try {
      await action();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(null);
    }
  }

  const addLink = () => {
    const url = draft?.trim();
    setDraft(null);
    if (url) run("Saving…", () => addProfileLinks(person.id, [url]).then(onChanged));
  };

  const follow = (on: boolean) =>
    run(on ? "Following…" : "Unfollowing…", async () => {
      setWatch(await followPerson(person.id, on));
      setNote(on ? `Following ${first}. What's already there is recorded now; you'll see what's new.` : null);
      onChanged();
      // The first check runs in the background; show what it found.
      if (on) setTimeout(load, 6000);
    });

  const findPapers = () =>
    run("Looking on OpenAlex…", async () => {
      const found = await openalexCandidates(person.id);
      setCandidates(found);
    });

  const choose = (candidate: AuthorCandidate) =>
    run("Saving…", async () => {
      await addProfileLinks(person.id, [`https://openalex.org/${candidate.id}`]);
      setCandidates(null);
      onChanged();
      if (watch.following) {
        const result = await checkPersonNow(person.id);
        setNote(result.errors.length ? result.errors.join("; ") : `Papers by ${first} will be checked from now on.`);
        load();
      }
    });

  const checkNow = () =>
    run("Checking…", async () => {
      const result = await checkPersonNow(person.id);
      const found = result.new ? `${result.new} new ${result.new === 1 ? "item" : "items"}` : "Nothing new";
      setNote(result.errors.length ? `${found}. Couldn't check: ${result.errors.join("; ")}` : `${found}.`);
      load();
    });

  const lastChecked = Math.max(0, ...watch.sources.map((s) => s.checked_at ?? 0));
  const recent = showAll ? activity : activity.slice(0, SHOWN);

  return (
    <section className="page-section follow-panel">
      <div className="section-title-row">
        <h2 className="section-title">Profiles and activity</h2>
        {watch.following ? (
          <button className="button small following" disabled={!!busy} onClick={() => follow(false)} title="Stop following">
            <Icon name="check" size={13} />
            <span className="when-idle">Following</span>
            <span className="when-hover">Unfollow</span>
          </button>
        ) : (
          <button className="button small primary" disabled={!!busy} onClick={() => follow(true)}>
            <Icon name="eye" size={13} />
            Follow
          </button>
        )}
      </div>

      <div className="chips profile-links">
        {links.map((l) => (
          <span key={l.key} className="chip profile-link">
            <button className="profile-link-open" title={l.label} onClick={() => openUrl(linkUrl(l.key, person.info[l.key])).catch((err) => setError(String(err)))}>
              <Icon name={l.icon} size={13} />
              {l.key === "homepage" || l.key === "feed" ? linkText(l.key, person.info[l.key]) : l.label}
            </button>
            <button aria-label={`Remove ${l.label} link`} onClick={() => run("Saving…", () => removeProfileLink(person.id, l.key).then(onChanged))}>
              <Icon name="close" size={11} />
            </button>
          </span>
        ))}
        {draft !== null ? (
          <input
            className="chip-input wide"
            value={draft}
            autoFocus
            placeholder="Paste a homepage, X, LinkedIn, Scholar, GitHub link…"
            onChange={(e) => setDraft(e.currentTarget.value)}
            onBlur={addLink}
            onKeyDown={(e) => {
              if (e.key === "Enter") addLink();
              if (e.key === "Escape") setDraft(null);
            }}
          />
        ) : (
          <button className="chip add" onClick={() => setDraft("")}>
            <Icon name="plus" size={12} /> Link
          </button>
        )}
      </div>

      {watch.following && (
        <div className="watch-status">
          <p>
            {watch.sources.length > 0 ? (
              <>
                Checking {watch.sources.map((s) => (s.label === "GitHub" ? s.label : s.label.toLowerCase())).join(", ")}
                {lastChecked > 0 ? ` · last checked ${agoLabel(lastChecked)}` : " · first check running"}
              </>
            ) : (
              `Nothing to check yet. Add ${first}'s homepage or find their papers.`
            )}
          </p>
          {watch.unsupported.length > 0 && (
            <p className="muted small">
              {watch.unsupported.length === 1
                ? `${watch.unsupported[0]} doesn't let apps read its pages, so that link is only kept here.`
                : `${watch.unsupported.join(" and ")} don't let apps read their pages, so those links are only kept here.`}
            </p>
          )}
          <div className="watch-actions">
            {!watch.has_papers && !candidates && (
              <button className="button small" disabled={!!busy} onClick={findPapers}>
                <Icon name="document" size={13} />
                Find {first}'s papers
              </button>
            )}
            {watch.sources.length > 0 && (
              <button className="button small ghost" disabled={!!busy} onClick={checkNow}>
                <Icon name="refresh" size={13} />
                Check now
              </button>
            )}
            {busy && (
              <span className="busy">
                <span className="spinner" />
                {busy}
              </span>
            )}
          </div>
        </div>
      )}

      {candidates && (
        <div className="card candidates">
          <div className="card-title">Which of these is {person.name}?</div>
          <div className="card-sub">Researchers on OpenAlex with this name. Papers are checked for the one you pick.</div>
          {candidates.length === 0 && <p className="muted">No one found under this name.</p>}
          <ul className="candidate-list">
            {candidates.map((c) => (
              <li key={c.id}>
                <div className="candidate-text">
                  <span className="candidate-name">{c.name}</span>
                  <span className="muted small">
                    {[c.institutions.slice(0, 2).join(", ") || "No institution listed", `${c.works_count} works`].join(" · ")}
                  </span>
                </div>
                <button className="button small ghost" onClick={() => openUrl(`https://openalex.org/${c.id}`)} aria-label={`See ${c.name} on OpenAlex`}>
                  <Icon name="external" size={13} />
                </button>
                <button className="button small" disabled={!!busy} onClick={() => choose(c)}>
                  This is them
                </button>
              </li>
            ))}
          </ul>
          <div className="card-actions">
            <button className="button ghost small" onClick={() => setCandidates(null)}>
              None of these
            </button>
          </div>
        </div>
      )}

      {note && <p className="form-note">{note}</p>}
      {error && <p className="form-error">{error}</p>}

      {activity.length > 0 && (
        <>
          <h3 className="subsection-title">Recent activity</h3>
          <ul className="updates compact">
            {recent.map((u) => (
              <UpdateItem key={u.activity.id} activity={u.activity} onOpen={onOpen} onError={setError} />
            ))}
          </ul>
          {activity.length > SHOWN && (
            <button className="text-button" onClick={() => setShowAll((s) => !s)}>
              {showAll ? "Show less" : `Show all ${activity.length}`}
            </button>
          )}
        </>
      )}
    </section>
  );
}

export default FollowPanel;
