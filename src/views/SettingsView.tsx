import { useEffect, useState } from "react";
import { assistantStatus, getVault, openPageFile, type AssistantStatus, type VaultInfo } from "../api";
import Icon from "../ui/Icon";
import { ACCENTS, applyAppearance, loadAppearance, type Appearance, type Theme } from "../ui/appearance";

interface Props {
  /** Opens setup to switch between Claude Code and Codex. */
  onChangeAssistant: () => void;
}

function SettingsView({ onChangeAssistant }: Props) {
  const [assistant, setAssistant] = useState<AssistantStatus | null>(null);
  const [vault, setVault] = useState<VaultInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [appearance, setAppearance] = useState<Appearance>(loadAppearance);
  const change = (next: Partial<Appearance>) => {
    const value = { ...appearance, ...next };
    setAppearance(value);
    applyAppearance(value);
  };

  useEffect(() => {
    assistantStatus().then(setAssistant, (err) => setError(String(err)));
    getVault().then(setVault, (err) => setError(String(err)));
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
              <div className="model-name">Google Calendar</div>
              <div className="muted">
                {assistant.chosen === "codex"
                  ? "Available with Claude Code. With Codex, confirmed plans are saved in the app only."
                  : calendar == null
                    ? "Checked the first time you chat."
                    : calendarConnected
                      ? "Connected. Confirmed plans are added to your calendar."
                      : `Not connected. ${assistant.calendar_help}`}
              </div>
            </div>
            <span className={`status-dot${calendarConnected && assistant.chosen === "claude" ? " ok" : ""}`} />
          </div>
        </div>
      )}

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
