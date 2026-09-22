import { useEffect, useState } from "react";
import { addTaskToCalendar, googleStatus, skipTaskCalendar, type CalendarOffer, type GoogleStatus } from "../api";
import Icon from "../ui/Icon";

interface Props {
  version: number;
  /** A task went on the calendar, or was answered for: pages have changed. */
  onChanged: () => void;
  /** Google isn't connected yet and the user wants to see to it. */
  onSettings: () => void;
  /** Only these tasks, rather than everything waiting to be asked about. */
  only?: string;
}

/**
 * The question Suk asks when a task has a day and a time: should this go on your calendar?
 * Nothing is sent to Google until the answer is yes, and a no is remembered so the same task
 * isn't asked about twice.
 */
function CalendarAsk({ version, onChanged, onSettings, only }: Props) {
  const [status, setStatus] = useState<GoogleStatus | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [added, setAdded] = useState<{ name: string; link: string } | null>(null);

  useEffect(() => {
    googleStatus().then(setStatus, () => {});
  }, [version]);

  const offers = (status?.offers ?? []).filter((o) => !only || o.task.id === only);
  if (!status || offers.length === 0) return null;

  async function answer(offer: CalendarOffer, yes: boolean) {
    setBusy(offer.task.id);
    setError(null);
    try {
      if (yes) {
        const link = await addTaskToCalendar(offer.task.id);
        setAdded({ name: offer.task.name, link });
      } else {
        await skipTaskCalendar(offer.task.id);
      }
      setStatus(await googleStatus());
      onChanged();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(null);
    }
  }

  // Nothing can be added until Google is connected, so that is the only thing to offer.
  if (!status.connected) {
    return (
      <div className="ask-card">
        <span className="section-icon">
          <Icon name="calendar" size={15} />
        </span>
        <div className="ask-text">
          <div className="ask-title">
            {offers.length === 1 ? offers[0].task.name : `${offers.length} tasks have a day and a time`}
          </div>
          <div className="muted small">Sign in with Google to put {offers.length === 1 ? "it" : "them"} on your calendar.</div>
        </div>
        <button className="button small" onClick={onSettings}>
          Sign in
        </button>
      </div>
    );
  }

  return (
    <div className="ask-card">
      <span className="section-icon">
        <Icon name="calendar" size={15} />
      </span>
      <div className="ask-text">
        <div className="ask-title">Put {offers.length === 1 ? "this" : "these"} on your Google Calendar?</div>
        <ul className="ask-list">
          {offers.map((offer) => (
            <li key={offer.task.id}>
              <span className="ask-name">{offer.task.name}</span>
              <span className="muted small">{offer.when}</span>
              <button className="button small" disabled={busy === offer.task.id} onClick={() => answer(offer, true)}>
                Add
              </button>
              <button className="button small ghost" disabled={busy === offer.task.id} onClick={() => answer(offer, false)}>
                Not now
              </button>
            </li>
          ))}
        </ul>
        {added && (
          <p className="form-note">
            {added.name} is on your calendar.
            {added.link && (
              <>
                {" "}
                <a href={added.link} target="_blank" rel="noreferrer">
                  Open it
                </a>
              </>
            )}
          </p>
        )}
        {error && <p className="form-error">{error}</p>}
      </div>
    </div>
  );
}

export default CalendarAsk;
