import { useEffect, useState } from "react";
import {
  applyTemplate,
  assistantStatus,
  calendarFeed,
  connectGoogle,
  disconnectGoogle,
  getVault,
  googleStatus,
  openPageFile,
  openUrl,
  saveCalendarFile,
  setCalendarFeed,
  setGoogleClient,
  subscribeCalendar,
  templates,
  type AssistantStatus,
  type CalendarFeed,
  type GoogleStatus,
  type Template,
  type VaultInfo,
} from "../api";
import Icon from "../ui/Icon";
import { ACCENTS, applyAppearance, loadAppearance, type Appearance, type Theme } from "../ui/appearance";

interface Props {
  /** Opens setup to switch between Claude Code and Codex. */
  onChangeAssistant: () => void;
  /** Sections may have changed. */
  onChanged: () => void;
}

function SettingsView({ onChangeAssistant, onChanged }: Props) {
  const [assistant, setAssistant] = useState<AssistantStatus | null>(null);
  const [vault, setVault] = useState<VaultInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [starters, setStarters] = useState<Template[]>([]);
  const [starter, setStarter] = useState<string | null>(null);
  const [added, setAdded] = useState<string | null>(null);
  const [google, setGoogle] = useState<GoogleStatus | null>(null);
  // While the browser is open waiting for the user to allow access.
  const [connecting, setConnecting] = useState(false);
  const [client, setClient] = useState({ id: "", secret: "" });
  const [feed, setFeed] = useState<CalendarFeed | null>(null);
  // What just happened with the calendar: the link copied, or the file saved.
  const [feedNote, setFeedNote] = useState<string | null>(null);
  const [appearance, setAppearance] = useState<Appearance>(loadAppearance);
  const change = (next: Partial<Appearance>) => {
    const value = { ...appearance, ...next };
    setAppearance(value);
    applyAppearance(value);
  };

  useEffect(() => {
    assistantStatus().then(setAssistant, (err) => setError(String(err)));
    templates().then(([all, chosen]) => {
      setStarters(all);
      setStarter(chosen);
    }, () => {});
    getVault().then(setVault, (err) => setError(String(err)));
    calendarFeed().then(setFeed, () => {});
    googleStatus().then(setGoogle, () => {});
  }, []);

  const current = assistant?.assistants.find((a) => a.kind === assistant.chosen) ?? null;
  const calendar = assistant?.calendar_status;
  const calendarConnected = calendar === "connected" || calendar === "pending";

  return (
    <div className="page">
      <div className="page-header">
        <div>
          <h1>Settings</h1>
        </div>
      </div>

      {error && <p className="form-error">{error}</p>}

      <h2 className="section-title">Appearance</h2>
      <div className="settings-card">
        <div className="settings-row">
          <span className="section-icon">
            <Icon name={appearance.theme === "dark" ? "moon" : "sun"} size={15} />
          </span>
          <div className="settings-text">
            <div className="model-name">Theme</div>
            <div className="muted">System follows your Mac's light or dark setting.</div>
          </div>
          <div className="segmented" role="radiogroup" aria-label="Theme">
            {(["system", "light", "dark"] as Theme[]).map((theme) => (
              <button
                key={theme}
                role="radio"
                aria-checked={appearance.theme === theme}
                className={appearance.theme === theme ? "on" : ""}
                onClick={() => change({ theme })}
              >
                {theme.charAt(0).toUpperCase() + theme.slice(1)}
              </button>
            ))}
          </div>
        </div>
        <div className="settings-row">
          <span className="section-icon">
            <Icon name="palette" size={15} />
          </span>
          <div className="settings-text">
            <div className="model-name">Accent color</div>
            <div className="muted">Used for buttons, highlights and the current page.</div>
          </div>
          <div className="swatches" role="radiogroup" aria-label="Accent color">
            {ACCENTS.map((accent) => (
              <button
                key={accent.id}
                role="radio"
                aria-checked={appearance.accent === accent.id}
                aria-label={accent.label}
                title={accent.label}
                className={`swatch${appearance.accent === accent.id ? " on" : ""}`}
                style={{ "--swatch": accent.color } as React.CSSProperties}
                onClick={() => change({ accent: accent.id })}
              />
            ))}
          </div>
        </div>
      </div>

      <h2 className="section-title">Assistant</h2>
      {assistant && (
        <div className="settings-card">
          <div className="settings-row">
            <span className="section-icon">
              <Icon name="sparkle" size={15} />
            </span>
            <div className="settings-text">
              <div className="model-name">{current?.label ?? "No assistant"}</div>
              <div className="muted">
                {current
                  ? current.signed_in
                    ? `Signed in${current.account ? ` as ${current.account}` : ""}. Your messages and notes are sent to ${current.kind === "claude" ? "Anthropic" : "OpenAI"}.`
                    : `${current.installed ? "Not signed in" : "Not installed"}, so Suk can't answer.`
                  : "Suk needs Claude Code or Codex to run."}
              </div>
            </div>
            <span className={`status-dot${assistant.ready ? " ok" : ""}`} />
            <button className="button small" onClick={onChangeAssistant}>
              {assistant.ready ? "Change" : "Set up"}
            </button>
          </div>
          <div className="settings-row">
            <span className="section-icon">
              <Icon name="calendar" size={15} />
            </span>
            <div className="settings-text">
              <div className="model-name">Reading your calendar</div>
              <div className="muted">
                {assistant.chosen === "codex"
                  ? "Available with Claude Code. With Codex, confirmed plans are saved in the app only."
                  : calendar == null
                    ? "Checked the first time you chat."
                    : calendarConnected
                      ? "Connected, so planning can see what you already have on."
                      : `Not connected. ${assistant.calendar_help}`}
              </div>
            </div>
            <span className={`status-dot${calendarConnected && assistant.chosen === "claude" ? " ok" : ""}`} />
          </div>
        </div>
      )}

      <h2 className="section-title">Calendar</h2>
      {google && (
        <div className="settings-card">
          <div className="settings-row">
            <span className="section-icon">
              <Icon name="calendar" size={15} />
            </span>
            <div className="settings-text">
              <div className="model-name">Google Calendar</div>
              <div className="muted">
                {google.connected
                  ? `Connected${google.account ? ` as ${google.account}` : ""}. When a task has a day and a time, Suk asks whether to put it on your calendar; nothing goes across until you say yes.`
                  : google.client_set
                    ? "Sign in and Suk can add the tasks you approve. It asks only to manage its own events, never to read the rest of your calendar."
                    : "This copy of Suk was built without Google sign-in. Later releases have it; until then you can point Suk at your own Google project."}
              </div>
              {!google.client_set && (
                <details className="client-form">
                  <summary className="muted small">Use my own Google project</summary>
                  <p className="muted small">
                    From{" "}
                    <button className="text-button" onClick={() => openUrl("https://console.cloud.google.com/apis/credentials")}>
                      Google Cloud Console
                    </button>
                    : enable the Calendar API, then create an OAuth client ID of type <b>Desktop app</b>.
                  </p>
                  <input
                    className="detail-input"
                    placeholder="Client ID (ends in .apps.googleusercontent.com)"
                    value={client.id}
                    onChange={(e) => setClient({ ...client, id: e.currentTarget.value })}
                  />
                  <input
                    className="detail-input"
                    placeholder="Client secret"
                    value={client.secret}
                    onChange={(e) => setClient({ ...client, secret: e.currentTarget.value })}
                  />
                  <button
                    className="button small"
                    disabled={!client.id.trim()}
                    onClick={() =>
                      setGoogleClient(client.id, client.secret).then(
                        () => googleStatus().then(setGoogle),
                        (err) => setError(String(err)),
                      )
                    }
                  >
                    Save
                  </button>
                </details>
              )}
            </div>
            <span className={`status-dot${google.connected ? " ok" : ""}`} />
            {google.client_set &&
              (google.connected ? (
                <button
                  className="button small ghost"
                  onClick={() => disconnectGoogle().then(() => googleStatus().then(setGoogle), (err) => setError(String(err)))}
                >
                  Disconnect
                </button>
              ) : (
                <button
                  className="button small"
                  disabled={connecting}
                  onClick={() => {
                    setConnecting(true);
                    setError(null);
                    connectGoogle()
                      .then(() => googleStatus().then(setGoogle), (err) => setError(String(err)))
                      .finally(() => setConnecting(false));
                  }}
                >
                  {connecting ? "Waiting for Google…" : "Sign in with Google"}
                </button>
              ))}
          </div>
        </div>
      )}

      {feed && (
        <div className="settings-card">
          <div className="settings-row">
            <span className="section-icon">
              <Icon name="calendar" size={15} />
            </span>
            <div className="settings-text">
              <div className="model-name">Other calendar apps</div>
              <div className="muted">
                Not using Google? Apple Calendar, Outlook and Thunderbird can subscribe to this link and keep
                themselves up to date with the time blocks you confirm.
              </div>
              <p className="system">
                <code>{feed.url}</code>
              </p>
              <p className="system">
                {feed.on
                  ? `${feed.blocks} ${feed.blocks === 1 ? "block is" : "blocks are"} on it. Your calendar app looks again every 15 minutes.`
                  : "Publishing is off, so the link is empty."}
              </p>
            </div>
            <label className="switch" title={feed.on ? "Stop publishing" : "Publish again"}>
              <input
                type="checkbox"
                checked={feed.on}
                onChange={(e) => {
                  const on = e.currentTarget.checked;
                  setFeed({ ...feed, on });
                  setCalendarFeed(on).catch((err) => setError(String(err)));
                }}
              />
              <span />
            </label>
            <button className="button small" disabled={!feed.on} onClick={() => subscribeCalendar().catch((err) => setError(String(err)))}>
              Subscribe
            </button>
            <button
              className="button small ghost"
              onClick={() =>
                navigator.clipboard.writeText(feed.url).then(
                  () => setFeedNote("Link copied. In your calendar app, add a calendar by URL and paste it."),
                  () => setError("Couldn't copy the link."),
                )
              }
            >
              Copy link
            </button>
          </div>
          <div className="settings-row">
            <span className="section-icon">
              <Icon name="download" size={15} />
            </span>
            <div className="settings-text">
              <div className="model-name">A file to import</div>
              <div className="muted">
                For a calendar that imports rather than subscribes. Google only reads links it can reach from the internet, and this one lives on your computer. Save the
                blocks as a file and import it in Google Calendar (Settings → Import &amp; export). Repeat after
                planning; the same block never comes in twice.
              </div>
            </div>
            <button
              className="button small"
              onClick={() => saveCalendarFile().then((path) => setFeedNote(`Saved to ${path}.`), (err) => setError(String(err)))}
            >
              Save file
            </button>
          </div>
          {feedNote && <p className="form-note">{feedNote}</p>}
        </div>
      )}

      <h2 className="section-title">What you do</h2>
      <div className="settings-card">
        {starters.map((t) => (
          <div key={t.id} className="settings-row">
            <span className="section-icon">
              <Icon name={t.id === "academic" ? "student" : "briefcase"} size={15} />
            </span>
            <div className="settings-text">
              <div className="model-name">{t.name}</div>
              <div className="muted">{t.adds.join(". ")}.</div>
            </div>
            {starter === t.id ? (
              <span className="setup-badge ready">In use</span>
            ) : (
              <button
                className="button small"
                onClick={() =>
                  applyTemplate(t.id).then((sections) => {
                    setStarter(t.id);
                    setAdded(sections.length ? `Added ${sections.join(", ")} to the sidebar.` : "Nothing new to add; the assistant now knows what you do.");
                    onChanged();
                  }, (err) => setError(String(err)))
                }
              >
                Use this
              </button>
            )}
          </div>
        ))}
        {added && <p className="form-note">{added}</p>}
      </div>

      <h2 className="section-title">Your files</h2>
      {vault && (
        <div className="settings-card vault">
          <div className="settings-row">
            <span className="section-icon">
              <Icon name="document" size={15} />
            </span>
            <div className="settings-text">
              <div className="muted">
                Every page is a plain Markdown file in this folder, and they're yours: open them in any editor,
                back them up, or keep the folder in iCloud or Dropbox. Changes you make there come back into Suk
                within a few seconds.
              </div>
              <p className="system">
                <code>{vault.path}</code>
              </p>
              <p className="system">
                {vault.registered
                  ? "Obsidian has this folder as a vault, so pages open there."
                  : "Use Obsidian? Choose \"Open folder as vault\" there and pick this folder; pages will then open in Obsidian."}
              </p>
            </div>
            <button className="button small" onClick={() => openPageFile(null).catch((err) => setError(String(err)))}>
              {vault.registered ? "Open in Obsidian" : "Show folder"}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

export default SettingsView;
