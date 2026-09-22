import { useEffect, useRef, useState } from "react";
import {
  applyTemplate,
  assistantStatus,
  cancelSignIn,
  chooseAssistant,
  installAssistant,
  openUrl,
  signInAssistant,
  submitSignInCode,
  templates,
  type AssistantInfo,
  type AssistantKind,
  type AssistantStatus,
  type SignInPrompt,
  type Template,
} from "../api";
import Icon from "../ui/Icon";

interface Props {
  /** Called once an assistant is installed, signed in and chosen. */
  onReady: () => void;
  /** Shown when changing the assistant from Settings. */
  onCancel?: () => void;
}

const ABOUT: Record<AssistantKind, { by: string; perks: string; signUp: string; signUpLabel: string }> = {
  claude: {
    by: "by Anthropic",
    perks: "Recommended · also connects Google Calendar",
    signUp: "https://claude.com/pricing",
    signUpLabel: "Get a Claude plan",
  },
  codex: {
    by: "by OpenAI",
    perks: "Use your ChatGPT plan",
    signUp: "https://chatgpt.com/pricing",
    signUpLabel: "Get a ChatGPT plan",
  },
  gemini: {
    by: "by Google",
    perks: "Has a free tier",
    signUp: "https://codeassist.google/",
    signUpLabel: "About Gemini CLI",
  },
};

type Busy = { what: "install"; log: string[] } | { what: "sign-in"; prompt: SignInPrompt | null } | null;

/**
 * First run: choose Claude Code or Codex, install it and sign in without leaving the app.
 * Suk can't run without one of them.
 */
function Setup({ onReady, onCancel }: Props) {
  const [status, setStatus] = useState<AssistantStatus | null>(null);
  const [kind, setKind] = useState<AssistantKind | null>(null);
  const [busy, setBusy] = useState<Busy>(null);
  const [code, setCode] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  // The starter template: which sections the sidebar begins with.
  const [starters, setStarters] = useState<Template[]>([]);
  const [starter, setStarter] = useState<string | null>(null);
  const logRef = useRef<HTMLPreElement>(null);

  const refresh = async () => {
    setChecking(true);
    try {
      const next = await assistantStatus();
      setStatus(next);
      setKind((current) => current ?? next.chosen ?? next.assistants.find((a) => a.signed_in)?.kind ?? "claude");
    } catch (err) {
      setError(String(err));
    } finally {
      setChecking(false);
    }
  };

  useEffect(() => {
    templates().then(([all, chosen]) => {
      setStarters(all);
      setStarter(chosen ?? all[0]?.id ?? null);
    }, () => {});
  }, []);

  useEffect(() => {
    refresh();
    return () => {
      cancelSignIn().catch(() => {});
    };
  }, []);

  useEffect(() => {
    logRef.current?.scrollTo({ top: logRef.current.scrollHeight });
  }, [busy]);

  const info = status?.assistants.find((a) => a.kind === kind) ?? null;
  const update = (next: AssistantInfo) =>
    setStatus((s) => (s ? { ...s, assistants: s.assistants.map((a) => (a.kind === next.kind ? next : a)) } : s));

  async function install() {
    if (!info) return;
    setError(null);
    setBusy({ what: "install", log: [] });
    try {
      update(await installAssistant(info.kind, (line) => setBusy((b) => (b?.what === "install" ? { ...b, log: [...b.log, line].slice(-200) } : b))));
      setBusy(null);
    } catch (err) {
      setError(String(err));
      setBusy((b) => (b?.what === "install" ? b : null));
    }
  }

  async function signIn() {
    if (!info) return;
    setError(null);
    setCode("");
    setBusy({ what: "sign-in", prompt: null });
    try {
      update(await signInAssistant(info.kind, (prompt) => setBusy({ what: "sign-in", prompt })));
      setBusy(null);
    } catch (err) {
      setBusy(null);
      if (!String(err).includes("cancelled")) setError(String(err));
    }
  }

  async function sendCode() {
    if (!code.trim()) return;
    setError(null);
    try {
      await submitSignInCode(code.trim());
      setCode("");
    } catch (err) {
      setError(String(err));
    }
  }

  async function start() {
    if (!info) return;
    try {
      await chooseAssistant(info.kind);
      if (starter) await applyTemplate(starter);
      onReady();
    } catch (err) {
      setError(String(err));
    }
  }

  const stepState = (done: boolean, active: boolean) => (done ? "done" : active ? "active" : "todo");
  const signInPrompt = busy?.what === "sign-in" ? busy.prompt : null;

  return (
    <div className="setup">
      <div className="setup-card">
        <div className="setup-head">
          <span className="setup-mark">
            <Icon name="sparkle" size={22} />
          </span>
          <h1>{onCancel ? "Change your assistant" : "Welcome to Suk"}</h1>
          <p className="muted">
            Suk works through an AI assistant you already use. It reads and saves your notes through the
            assistant, so it needs one to run.
          </p>
        </div>

        {!status ? (
          <div className="setup-loading">
            <span className="spinner" /> Checking what's on this computer…
          </div>
        ) : (
          <>
            <div className="setup-options" role="radiogroup" aria-label="Assistant">
              {status.assistants.map((a) => {
                const about = ABOUT[a.kind];
                const selected = a.kind === kind;
                return (
                  <button
                    key={a.kind}
                    role="radio"
                    aria-checked={selected}
                    className={`setup-option${selected ? " on" : ""}`}
                    disabled={!!busy}
                    onClick={() => {
                      setKind(a.kind);
                      setError(null);
                    }}
                  >
                    <span className="setup-option-top">
                      <span className="setup-option-name">{a.label}</span>
                      <span className={`setup-badge ${a.signed_in ? "ready" : a.installed ? "half" : ""}`}>
                        {a.signed_in ? "Ready" : a.installed ? "Installed" : "Not installed"}
                      </span>
                    </span>
                    <span className="setup-option-by">{about.by}</span>
                    <span className="setup-option-perks">{about.perks}</span>
                  </button>
                );
              })}
            </div>

            {info && (
              <ol className="setup-steps">
                <li className={stepState(info.installed, !info.installed)}>
                  <span className="setup-step-mark">{info.installed ? <Icon name="check" size={13} /> : 1}</span>
                  <div className="setup-step-body">
                    <div className="setup-step-title">
                      {info.installed ? `${info.label} is installed` : `Install ${info.label}`}
                      {info.version && <span className="muted small"> {info.version.replace(/\(.*\)/, "").trim()}</span>}
                    </div>
                    {!info.installed && (
                      <>
                        <p className="muted small">Takes about a minute. Nothing else on your computer changes.</p>
                        <div className="setup-actions">
                          <button className="button primary" disabled={!!busy} onClick={install}>
                            {busy?.what === "install" ? <span className="spinner light" /> : <Icon name="plus" size={14} />}
                            {busy?.what === "install" ? "Installing…" : `Install ${info.label}`}
                          </button>
                        </div>
                        <details className="setup-manual">
                          <summary>Or install it yourself in a terminal</summary>
                          <code>{info.install_command}</code>
                        </details>
                        {busy?.what === "install" && busy.log.length > 0 && (
                          <pre className="setup-log" ref={logRef}>
                            {busy.log.join("\n")}
                          </pre>
                        )}
                      </>
                    )}
                  </div>
                </li>

                <li className={stepState(info.signed_in, info.installed && !info.signed_in)}>
                  <span className="setup-step-mark">{info.signed_in ? <Icon name="check" size={13} /> : 2}</span>
                  <div className="setup-step-body">
                    <div className="setup-step-title">
                      {info.signed_in ? "Signed in" : `Sign in to ${info.label}`}
                      {info.account && <span className="muted small"> {info.account}</span>}
                    </div>
                    {info.installed && !info.signed_in && (
                      <>
                        {busy?.what !== "sign-in" ? (
                          <>
                            <p className="muted small">You'll need {info.requirement}.</p>
                            <div className="setup-actions">
                              <button className="button primary" disabled={!!busy} onClick={signIn}>
                                Sign in
                              </button>
                            </div>
                          </>
                        ) : !signInPrompt ? (
                          <p className="busy">
                            <span className="spinner" /> Opening the sign-in page…
                          </p>
                        ) : signInPrompt.in_terminal ? (
                          <div className="setup-signin">
                            <p>
                              A terminal window opened with {info.label}. Choose how to sign in there and follow the steps;
                              this page continues by itself once you're done.
                            </p>
                            <p className="busy">
                              <span className="spinner" /> Waiting for the terminal…
                            </p>
                          </div>
                        ) : info.kind === "codex" ? (
                          <div className="setup-signin">
                            <p>A sign-in page opened in your browser. Enter this code there:</p>
                            <div className="setup-code">{signInPrompt.code ?? "…"}</div>
                            <p className="busy">
                              <span className="spinner" /> Waiting for you to finish in the browser…
                            </p>
                          </div>
                        ) : (
                          <div className="setup-signin">
                            <p>A sign-in page opened in your browser. After you sign in, it shows a code. Paste it here:</p>
                            <form
                              className="setup-code-form"
                              onSubmit={(e) => {
                                e.preventDefault();
                                sendCode();
                              }}
                            >
                              <input value={code} autoFocus placeholder="Paste the code" onChange={(e) => setCode(e.currentTarget.value)} />
                              <button className="button primary" disabled={!code.trim()}>
                                Continue
                              </button>
                            </form>
                          </div>
                        )}
                        {(signInPrompt?.url || signInPrompt?.in_terminal) && (
                          <div className="setup-actions">
                            {signInPrompt.url && (
                              <button className="text-button" onClick={() => openUrl(signInPrompt.url!).catch(() => {})}>
                                Open the sign-in page again
                              </button>
                            )}
                            <button className="text-button muted-link" onClick={() => cancelSignIn()}>
                              Cancel
                            </button>
                          </div>
                        )}
                      </>
                    )}
                  </div>
                </li>
              </ol>
            )}

            {info?.signed_in && starters.length > 0 && (
              <div className="setup-starter">
                <div className="setup-step-title">What do you do?</div>
                <p className="muted small">Sets up the sidebar and the words the assistant uses. You can change it later.</p>
                <div className="choice-chips">
                  {starters.map((t) => (
                    <button
                      key={t.id}
                      className={`choice-chip${starter === t.id ? " on" : ""}`}
                      aria-pressed={starter === t.id}
                      title={t.adds.join(". ")}
                      onClick={() => setStarter(t.id)}
                    >
                      {t.name}
                      <span className="muted small"> · {t.about}</span>
                    </button>
                  ))}
                </div>
              </div>
            )}

            {error && <p className="form-error setup-error">{error}</p>}

            <div className="setup-footer">
              {onCancel && (
                <button className="button ghost" onClick={onCancel}>
                  Cancel
                </button>
              )}
              <button className="button ghost" disabled={checking || !!busy} onClick={refresh}>
                <Icon name="refresh" size={14} /> Check again
              </button>
              <button className="button primary large" disabled={!info?.signed_in || !!busy} onClick={start}>
                {info?.signed_in ? `Start with ${info.label}` : "Start"}
                <Icon name="chevron" size={14} />
              </button>
            </div>

            {!status.assistants.some((a) => a.signed_in) && (
              <div className="setup-need">
                <strong>Don't have any of them?</strong> Suk can't run without one. Claude Code and Codex come with
                a paid Claude or ChatGPT plan; Gemini CLI has a free tier.{" "}
                {kind && (
                  <button className="text-button" onClick={() => openUrl(ABOUT[kind].signUp).catch(() => {})}>
                    {ABOUT[kind].signUpLabel}
                  </button>
                )}
              </div>
            )}
          </>
        )}
      </div>
    </div>
  );
}

export default Setup;
