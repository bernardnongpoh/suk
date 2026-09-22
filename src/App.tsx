import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import Sidebar from "./navigation/Sidebar";
import ChatView from "./chat/ChatView";
import SearchPalette from "./search/SearchPalette";
import CalendarView from "./views/CalendarView";
import TodayView from "./views/TodayView";
import NotesView from "./views/NotesView";
import PageView from "./views/PageView";
import SectionView from "./views/SectionView";
import SettingsView from "./views/SettingsView";
import TidyView from "./views/TidyView";
import UpdatesView from "./views/UpdatesView";
import Icon from "./ui/Icon";
import Setup from "./onboarding/Setup";
import { assistantStatus, getSidebar, listUpdates, newConversation, tidyItems, type Sidebar as SidebarData } from "./api";
import type { ChatMessage, Route } from "./types";
import "./App.css";

const PLAN_PROMPT = "What should I focus on today? Plan my day.";

/** Whether a chat is waiting on something: a reply, a schedule to confirm, or a form to fill in. */
const waiting = (messages: ChatMessage[]) =>
  messages.some((m) => m.proposals?.some((p) => p.status === "pending") || (m.details?.length ?? 0) > 0);

function App() {
  // Opening pages pushes onto the stack, so Back returns to where you were.
  const [stack, setStack] = useState<Route[]>([{ view: "chat" }]);
  // Lives here so the conversation survives switching views.
  const [chatMessages, setChatMessages] = useState<ChatMessage[]>([]);
  // A page's chat, kept only while it's waiting on something; otherwise the page opens clean.
  const [pageChats, setPageChats] = useState<Record<string, ChatMessage[]>>({});
  const [chatPrompt, setChatPrompt] = useState<string | null>(null);
  const [sidebar, setSidebar] = useState<SidebarData | null>(null);
  const [searching, setSearching] = useState(false);
  // Bumped when pages may have changed, so open views reload.
  const [version, setVersion] = useState(0);
  // Relevant updates from followed people not yet seen, and the latest arrival to announce.
  const [unread, setUnread] = useState(0);
  const [toast, setToast] = useState<{ title: string; body: string } | null>(null);
  // New relationships, page types and possible duplicates waiting for a decision.
  const [toTidy, setToTidy] = useState(0);

  // Whether Claude Code or Codex is set up: null while checking. The app can't run without one.
  const [ready, setReady] = useState<boolean | null>(null);
  const [changingAssistant, setChangingAssistant] = useState(false);
  useEffect(() => {
    assistantStatus().then((s) => setReady(s.ready), () => setReady(false));
  }, []);

  const route = stack[stack.length - 1];
  const select = (r: Route) => {
    // Clicking Chat again starts a new conversation; what was said stays saved.
    if (r.view === "chat" && route.view === "chat" && chatMessages.length > 0) {
      setChatMessages([]);
      newConversation().catch(() => {});
    }
    setStack([r]);
  };
  const open = (id: string) => {
    setPageChats((chats) => (waiting(chats[id] ?? []) ? chats : { ...chats, [id]: [] }));
    setStack((s) => {
      const top = s[s.length - 1];
      return top.view === "page" && top.id === id ? s : [...s, { view: "page", id }];
    });
  };
  const back = stack.length > 1 ? () => setStack((s) => s.slice(0, -1)) : null;

  const changed = useCallback(() => {
    getSidebar().then(setSidebar, () => {});
    setVersion((v) => v + 1);
  }, []);

  const countUnread = useCallback(() => {
    listUpdates(null, false).then((updates) => setUnread(updates.filter((u) => !u.activity.seen).length), () => {});
  }, []);

  useEffect(countUnread, [countUnread, version]);

  useEffect(() => {
    tidyItems().then((items) => setToTidy(items.count), () => {});
  }, [version]);

  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => setToast(null), 10000);
    return () => clearTimeout(timer);
  }, [toast]);

  useEffect(() => {
    // Checks of followed people run in the background.
    const stopChanged = listen("updates-changed", () => {
      countUnread();
      setVersion((v) => v + 1);
    }).catch(() => () => {});
    const stopArrived = listen<{ title: string; body: string }>("updates-arrived", (e) => setToast(e.payload)).catch(() => () => {});
    return () => {
      stopChanged.then((stop) => stop());
      stopArrived.then((stop) => stop());
    };
  }, [countUnread]);

  useEffect(() => {
    getSidebar().then(setSidebar, () => {});
    // Edits made to the Markdown files, in Obsidian or any editor.
    const unlisten = listen("pages-changed", changed).catch(() => () => {});
    // The macOS menu owns ⌘K, so search opens from there too.
    const stopSearch = listen("open-search", () => setSearching(true)).catch(() => () => {});
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setSearching((s) => !s);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      unlisten.then((stop) => stop());
      stopSearch.then((stop) => stop());
    };
  }, [changed]);

  if (ready === null) {
    return <div className="app splash" aria-busy="true" />;
  }
  if (!ready || changingAssistant) {
    return (
      <Setup
        onReady={() => {
          setReady(true);
          setChangingAssistant(false);
          changed();
        }}
        onCancel={ready ? () => setChangingAssistant(false) : undefined}
      />
    );
  }

  return (
    <div className="app">
      <Sidebar
        route={route}
        data={sidebar}
        unread={unread}
        toTidy={toTidy}
        onSelect={select}
        onSearch={() => setSearching(true)}
        onOpen={open}
        onChanged={changed}
      />
      <main className="content">
        {route.view === "today" && (
          <TodayView
            version={version}
            onOpen={open}
            onUpdates={() => select({ view: "updates" })}
            onChanged={changed}
            onSettings={() => select({ view: "settings" })}
            onPlan={() => {
              setChatPrompt(PLAN_PROMPT);
              select({ view: "chat" });
            }}
          />
        )}
        {route.view === "chat" && (
          <ChatView
            messages={chatMessages}
            setMessages={setChatMessages}
            loadHistory
            prompt={chatPrompt}
            onPromptSent={() => setChatPrompt(null)}
            onChanged={changed}
            onOpen={open}
          />
        )}
        {route.view === "calendar" && <CalendarView version={version} onOpen={open} />}
        {route.view === "updates" && <UpdatesView version={version} onOpen={open} />}
        {route.view === "notes" && <NotesView version={version} onOpen={open} onChanged={changed} />}
        {route.view === "section" && (
          <SectionView
            key={route.tag}
            tag={route.tag}
            title={sidebar?.sections.find((s) => s.tag === route.tag)?.title ?? route.title}
            icon={sidebar?.sections.find((s) => s.tag === route.tag)?.icon ?? ""}
            version={version}
            onOpen={open}
            onChanged={changed}
          />
        )}
        {route.view === "page" && (
          <PageView
            key={route.id}
            id={route.id}
            version={version}
            messages={pageChats[route.id] ?? []}
            setMessages={(update) =>
              setPageChats((chats) => ({
                ...chats,
                [route.id]: typeof update === "function" ? update(chats[route.id] ?? []) : update,
              }))
            }
            onOpen={open}
            onBack={back}
            onChanged={changed}
          />
        )}
        {route.view === "tidy" && <TidyView version={version} onOpen={open} onChanged={changed} />}
        {route.view === "settings" && <SettingsView onChangeAssistant={() => setChangingAssistant(true)} onChanged={changed} />}
      </main>
      {toast && (
        <div className="toast" role="status">
          <span className="toast-icon">
            <Icon name="bell" size={15} />
          </span>
          <div className="toast-text">
            <div className="toast-title">{toast.title}</div>
            <div className="toast-body">{toast.body}</div>
          </div>
          <button
            className="button small"
            onClick={() => {
              setToast(null);
              select({ view: "updates" });
            }}
          >
            View
          </button>
          <button className="icon-button" aria-label="Dismiss" onClick={() => setToast(null)}>
            <Icon name="close" size={14} />
          </button>
        </div>
      )}
      {searching && (
        <SearchPalette onOpen={open} onSelect={select} onClose={() => setSearching(false)} onChanged={changed} />
      )}
    </div>
  );
}

export default App;
