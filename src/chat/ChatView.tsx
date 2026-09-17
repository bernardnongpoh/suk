import { useEffect, useRef, useState } from "react";
import {
  chatHistory,
  confirmProposal,
  dismissProposal,
  sendMessage,
  type ChatReply,
  type Entity,
  type Proposal,
} from "../api";
import ProfileForm from "../people/ProfileForm";
import Icon from "../ui/Icon";
import type { ChatMessage } from "../types";
import Composer from "./Composer";
import ProposalCard from "./ProposalCard";
import SectionCard from "./SectionCard";

interface Props {
  messages: ChatMessage[];
  setMessages: React.Dispatch<React.SetStateAction<ChatMessage[]>>;
  /** The page this chat is about; Claude answers about it only. */
  focus?: Entity;
  /** A message to send as soon as the view opens, e.g. from Today's "Plan my day". */
  prompt?: string | null;
  onPromptSent?: () => void;
  /** Called after anything that may have changed pages or sections. */
  onChanged: () => void;
  onOpen: (id: string) => void;
}

let nextId = 0;

function ChatView({ messages, setMessages, focus, prompt, onPromptSent, onChanged, onOpen }: Props) {
  const [pending, setPending] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(messages.length > 0);
  const [preset, setPreset] = useState<{ text: string } | null>(null);
  const endRef = useRef<HTMLDivElement>(null);

  // Earlier conversations are kept, so they come back after a restart.
  useEffect(() => {
    if (loaded) return;
    chatHistory(focus?.id ?? null).then(
      (records) => {
        setMessages((current) =>
          current.length ? current : records.map((r) => ({ id: r.id, role: r.role, text: r.text })),
        );
        setLoaded(true);
      },
      () => setLoaded(true),
    );
  }, [focus?.id]);

  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: loaded ? "smooth" : "auto", block: "end" });
  }, [messages, pending, status]);

  const add = (message: Omit<ChatMessage, "id">) =>
    setMessages((prev) => [...prev, { ...message, id: `new-${nextId++}` }]);

  /** Replaces a proposal wherever it is shown with its latest state. */
  const updateProposal = (updated: Proposal) =>
    setMessages((prev) =>
      prev.map((m) =>
        m.proposals?.some((p) => p.id === updated.id)
          ? { ...m, proposals: m.proposals.map((p) => (p.id === updated.id ? updated : p)) }
          : m,
      ),
    );

  async function run(work: () => Promise<ChatReply>) {
    setPending(true);
    setStatus(null);
    try {
      const reply = await work();
      reply.proposals.filter((p) => p.status !== "pending").forEach(updateProposal);
      add({
        role: "agent",
        text: reply.text,
        proposals: reply.proposals.filter((p) => p.status === "pending"),
        sections: reply.sections,
        details: reply.details,
        notice: reply.notice,
      });
    } catch (err) {
      add({ role: "agent", text: `Something went wrong: ${err}` });
    } finally {
      setPending(false);
      setStatus(null);
      onChanged();
    }
  }

  async function send(text: string, mentions: Entity[] = []) {
    text = text.trim();
    if (!text || pending) return;
    add({ role: "user", text });
    await run(() => sendMessage(text, focus?.id ?? null, mentions.map((m) => m.id), setStatus));
  }

  const sentPrompt = useRef<string | null>(null);
  useEffect(() => {
    // StrictMode runs effects twice; send each prompt once.
    if (prompt && sentPrompt.current !== prompt) {
      sentPrompt.current = prompt;
      onPromptSent?.();
      send(prompt);
    }
  }, [prompt]);

  async function dismiss(proposal: Proposal) {
    try {
      updateProposal(await dismissProposal(proposal.id));
    } catch (err) {
      add({ role: "agent", text: `Something went wrong: ${err}` });
    }
  }

  // People are called by first name; other pages by their whole name.
  const first = focus?.kind === "Person" ? focus.name.split(/\s+/)[0] : focus?.name;
  const hints = focus
    ? [
        { label: `What do I know about ${first}?`, send: true },
        { label: "Add a note: ", send: false },
      ]
    : [
        { label: "What should I focus on today?", send: true },
        { label: "I have a new PhD student, ", send: false },
        { label: "I met ", send: false },
      ];

  return (
    <div className={`chat${focus ? " focused" : ""}`}>
      {focus && (
        <div className="chat-focus">
          <Icon name="sparkle" size={14} />
          <span>
            Claude is focused on <strong>{focus.name}</strong>
          </span>
        </div>
      )}
      <div className="chat-log">
        <div className="chat-column">
          {messages.length === 0 && !pending ? (
            loaded && (
              <div className="chat-empty">
                <div className="chat-empty-mark">
                  <Icon name="sparkle" size={22} />
                </div>
                <div className="chat-empty-title">
                  {focus ? `Ask or tell Claude anything about ${first}` : "What are you working on?"}
                </div>
                {!focus && (
                  <div className="chat-empty-sub">
                    Mention people and projects by name. When a suggestion appears, press Tab or Enter to pick it.
                  </div>
                )}
                <div className="chat-hints">
                  {hints.map((hint) => (
                    <button
                      key={hint.label}
                      className="hint"
                      onClick={() => (hint.send ? send(hint.label) : setPreset({ text: hint.label }))}
                    >
                      {hint.label.trim()}
                      {!hint.send && "…"}
                    </button>
                  ))}
                </div>
              </div>
            )
          ) : (
            messages.map((m) => (
              <div key={m.id} className={`turn ${m.role}`}>
                {m.role === "agent" && (
                  <div className="turn-mark" aria-hidden="true">
                    <Icon name="sparkle" size={13} />
                  </div>
                )}
                <div className="turn-body">
                  <div className={`message ${m.role}`}>{m.text}</div>
                  {m.details?.map((d) => (
                    <ProfileForm
                      key={d.entity.id}
                      request={d}
                      place="chat"
                      onChange={onChanged}
                      onOpen={focus ? undefined : onOpen}
                    />
                  ))}
                  {m.sections && m.sections.length > 0 && <SectionCard ideas={m.sections} onChange={onChanged} />}
                  {m.proposals?.map((p) => (
                    <ProposalCard
                      key={p.id}
                      proposal={p}
                      busy={pending}
                      onConfirm={(items) => run(() => confirmProposal(p.id, items, setStatus))}
                      onDismiss={() => dismiss(p)}
                    />
                  ))}
                  {m.notice && <div className="message-notice">{m.notice}</div>}
                </div>
              </div>
            ))
          )}
          {pending && (
            <div className="turn agent">
              <div className="turn-mark thinking" aria-hidden="true">
                <Icon name="sparkle" size={13} />
              </div>
              <div className="turn-body">
                <div className="message agent pending">
                  <span className="dots">
                    <i />
                    <i />
                    <i />
                  </span>
                  {status}
                </div>
              </div>
            </div>
          )}
          <div ref={endRef} />
        </div>
      </div>

      <div className="chat-dock">
        <Composer
          placeholder={focus ? `Ask about ${first}…` : "Message Claude — type a name, then Tab or Enter to pick it"}
          disabled={pending}
          autoFocus={!focus}
          preset={preset}
          onSend={send}
        />
      </div>
    </div>
  );
}

export default ChatView;
