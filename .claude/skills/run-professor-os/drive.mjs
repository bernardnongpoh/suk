// Drives the Professor OS frontend over the Chrome DevTools Protocol, against a fake backend.
// Usage: node drive.mjs [outDir]   (expects headless Chrome on port 9223 and Vite on 1420)
import { writeFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";

const OUT = process.argv[2] ?? process.cwd();
mkdirSync(OUT, { recursive: true });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let targets;
for (let i = 0; i < 30; i++) {
  try {
    targets = await (await fetch("http://127.0.0.1:9223/json")).json();
    break;
  } catch {
    await sleep(500);
  }
}
if (!targets) throw new Error("Chrome DevTools not reachable on port 9223");
const page = targets.find((t) => t.type === "page");
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener("open", r, { once: true }));

let nextId = 1;
const pending = new Map();
const consoleErrors = [];
ws.addEventListener("message", (e) => {
  const msg = JSON.parse(e.data);
  if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)(msg);
    pending.delete(msg.id);
  }
  if (msg.method === "Runtime.exceptionThrown") consoleErrors.push(msg.params.exceptionDetails.exception?.description);
  if (msg.method === "Runtime.consoleAPICalled" && msg.params.type === "error")
    consoleErrors.push(msg.params.args.map((a) => a.value ?? a.description).join(" "));
});
const send = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, (m) => (m.error ? reject(new Error(JSON.stringify(m.error))) : resolve(m.result)));
    ws.send(JSON.stringify({ id, method, params }));
  });
const evaluate = async (expr) =>
  (await send("Runtime.evaluate", { expression: expr, returnByValue: true, awaitPromise: true })).result.value;
const shot = async (name) => {
  const { data } = await send("Page.captureScreenshot", { format: "png" });
  writeFileSync(join(OUT, `${name}.png`), Buffer.from(data, "base64"));
  console.log(`screenshot: ${join(OUT, `${name}.png`)}`);
};
const click = (selector, text) =>
  evaluate(
    `(() => { const el = [...document.querySelectorAll(${JSON.stringify(selector)})].find(e => ${text === undefined ? "true" : `e.textContent.includes(${JSON.stringify(text)})`}); if (!el) return 'missing ${selector} ${text ?? ""}'; el.click(); return 'ok'; })()`,
  );
const focus = (selector) => evaluate(`document.querySelector(${JSON.stringify(selector)})?.focus() ?? 'missing'`);
const type = async (text) => {
  // One character at a time, as typing does, so the mention lookup runs on each keystroke.
  for (const ch of text) {
    await send("Input.insertText", { text: ch });
    await sleep(15);
  }
};
const key = async (k, modifiers = 0, code = k) => {
  const vk = { Enter: 13, Escape: 27, ArrowDown: 40, Tab: 9, k: 75 }[k] ?? 0;
  await send("Input.dispatchKeyEvent", { type: "keyDown", key: k, code, windowsVirtualKeyCode: vk, modifiers, text: k === "Enter" ? "\r" : undefined });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: k, code, windowsVirtualKeyCode: vk, modifiers });
};
const texts = (selector) => evaluate(`[...document.querySelectorAll(${JSON.stringify(selector)})].map(e => e.textContent)`);

await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false });
await send("Page.enable");
await send("Runtime.enable");
await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "light" }] });
// Outside Tauri there is no IPC bridge: a small in-memory backend stands in for commands.rs.
await send("Page.addScriptToEvaluateOnNewDocument", {
  source: `
    window.__invokes = [];
    const pad = (n) => String(n).padStart(2, "0");
    const d = new Date();
    const today = d.getFullYear() + "-" + pad(d.getMonth() + 1) + "-" + pad(d.getDate());
    const db = { entities: {}, links: [], sections: [{ tag: "student", title: "Students", pinned: true, icon: "🎓" }, { tag: "project", title: "Projects", pinned: true }], messages: [] };
    const put = (kind, name, extra = {}) => {
      const id = kind.toLowerCase() + ":" + name.toLowerCase();
      db.entities[id] ??= { id, kind, name, info: {}, tags: [], notes: "", updated_at: Date.now(), aliases: [] };
      Object.assign(db.entities[id], extra);
      return db.entities[id];
    };
    put("Person", "Satya", { tags: ["student"], info: { full_name: "Satya Prakash Das", email: "satya.das@example.edu", program: "PhD", affiliation: "IIT Guwahati", start: "2024" } });
    put("Person", "Meera Iyer", { tags: ["student"], info: { program: "MTech", email: "meera@example.edu" } });
    put("Project", "Fuzzing", { info: { status: "in-progress", funding: "SERB CRG" }, notes: "Grammar-based fuzzing of compilers" });
    put("Project", "LLM Static Analysis", { info: { status: "planned" }, notes: "Agent-driven static analysis platform" });
    put("Project", "Grammar Inference", { info: { status: "completed", icon: "🔤" }, notes: "Published at FSE 2025" });
    put("Project", "Reading group");
    const addDays = (n) => { const x = new Date(); x.setDate(x.getDate() + n); return x.getFullYear() + "-" + pad(x.getMonth() + 1) + "-" + pad(x.getDate()); };
    put("Person", "Kavya Rao", { tags: ["collaborator"], info: { affiliation: "IISc" } });
    put("Course", "Software Analysis");
    put("Task", "Review Satya's survey", { info: { due: today, priority: "high", area: "students", status: "open" } });
    put("Task", "Prepare lecture 7 slides", { info: { due: addDays(1) + "T11:00", priority: "high", area: "teaching", status: "open" } });
    put("Task", "Submit grant proposal", { info: { due: addDays(8), status: "waiting", area: "research" } });
    put("Task", "Sign travel form", { info: { due: addDays(-2), area: "admin" } });
    put("Task", "Read PLDI papers", { info: { area: "research" } });
    put("Task", "Old task", { info: { status: "done" } });
    put("Task", "Present state of the art on Solidity Compiler Fuzzing", { info: { due: addDays(7), area: "research", status: "open" } });
    db.links.push({ from: "person:satya", kind: "WORKS_ON", to: "project:fuzzing" });
    db.links.push({ from: "task:review satya's survey", kind: "FOR", to: "person:satya" });
    db.links.push({ from: "project:fuzzing", kind: "HAS_TASK", to: "task:review satya's survey" });
    db.links.push({ from: "course:software analysis", kind: "HAS_TASK", to: "task:prepare lecture 7 slides" });
    db.links.push({ from: "task:submit grant proposal", kind: "WAITING_ON", to: "person:kavya rao" });
    db.links.push({ from: "task:present state of the art on solidity compiler fuzzing", kind: "ASSIGNED_TO", to: "person:meera iyer" });
    put("Organization", "IIT Guwahati", { aliases: ["IITG"], info: { type: "university", city: "Guwahati", icon: "🏛️" } });
    put("Organization", "CSE, IIT Guwahati", { info: { type: "department" } });
    put("Person", "Arun", { info: { affiliation: "Google" } });
    put("Organization", "Google", { info: { type: "company" } });
    db.links.push({ from: "organization:cse, iit guwahati", kind: "PART_OF", to: "organization:iit guwahati" });
    db.links.push({ from: "person:satya", kind: "AFFILIATED_WITH", to: "organization:cse, iit guwahati", detail: "PhD student", since: "2024" });
    db.entities["person:satya"].info.affiliation = "CSE, IIT Guwahati";
    db.links.push({ from: "person:kavya rao", kind: "STUDIED_AT", to: "organization:iit guwahati", detail: "MTech", until: "2015" });
    db.links.push({ from: "person:arun", kind: "AFFILIATED_WITH", to: "organization:iit guwahati", detail: "Postdoc", since: "2020", until: "2022-06" });
    db.links.push({ from: "person:arun", kind: "AFFILIATED_WITH", to: "organization:google", detail: "Engineer", since: "2022-07" });
    db.entities["person:meera iyer"].info.affiliation = "IITG";
    // Following: a followed collaborator with activity, and someone not followed yet.
    put("Person", "Andreas Zeller", { tags: ["collaborator", "following"], info: { affiliation: "CISPA", homepage: "https://andreas-zeller.info/", twitter: "https://x.com/AndreasZeller", linkedin: "https://www.linkedin.com/in/andreaszeller/", openalex: "A5051672229", github: "https://github.com/andreas-zeller" } });
    put("Person", "Lin Tan", { tags: ["faculty"], info: { homepage: "https://www.cs.purdue.edu/homes/lintan/" } });
    put("ResearchArea", "Compiler Testing");
    const act = (id, extra) => ({ id, person: "person:andreas zeller", source: "openalex", kind: "paper", title: "", url: "https://doi.org/10.0/" + id, summary: "", published: addDays(-1), found_at: Date.now(), baseline: false, relevance: "", reason: "", related: [], seen: false, notified: true, ...extra });
    db.activities = [
      act("a1", { title: "Finding Miscompilations in Solidity Compilers with Grammar-Based Fuzzing", summary: "ICSE 2027 — We generate Solidity programs from a grammar and compare solc optimization levels, finding 14 miscompilations in the Solidity compiler.", relevance: "high", reason: "Grammar-based fuzzing of solc, the same approach as your project", related: ["Fuzzing", "Compiler Testing"] }),
      act("a2", { source: "homepage", kind: "page_change", title: "Andreas Zeller updated andreas-zeller.info", url: "https://andreas-zeller.info/", summary: "New: PhD and postdoc positions in compiler testing\\nFANDANGO 1.0 released", published: today, relevance: "medium", reason: "Positions in compiler testing, your research area", related: ["Compiler Testing"] }),
      act("a3", { source: "feed", kind: "post", title: "andreas-zeller pushed to fuzzingbook", url: "https://github.com/andreas-zeller/fuzzingbook", published: addDays(-2), relevance: "" }),
      act("a4", { title: "Protein Structure Prediction at Scale", summary: "Nature — Folding proteins with deep learning.", published: addDays(-3), relevance: "none", reason: "Unrelated to your work", seen: true }),
      act("a0", { title: "Search-Based Generation of Complex Inputs with FANDANGO", summary: "ICSE 2026", published: "2026-07-09", baseline: true, seen: true }),
    ];
    const listeners = {};
    window.__emit = (event, payload) => (listeners[event] ?? []).forEach((h) => channels[h]?.({ event, id: 0, payload }));
    const watchOf = (e) => {
      const i = e.info;
      const sources = [
        i.openalex && { label: "Papers", url: "https://openalex.org/" + i.openalex, checked_at: Date.now() - 42 * 60000 },
        i.homepage && { label: "Homepage", url: i.homepage, checked_at: Date.now() - 42 * 60000 },
        i.github && { label: "GitHub", url: i.github, checked_at: Date.now() - 42 * 60000 },
      ].filter(Boolean);
      return { following: e.tags.includes("following"), sources, unsupported: [i.twitter && "X", i.linkedin && "LinkedIn", i.scholar && "Google Scholar"].filter(Boolean), has_papers: !!i.openalex };
    };
    const linkKey = (url) => /x\\.com|twitter\\.com/.test(url) ? "twitter" : /linkedin/.test(url) ? "linkedin" : /scholar\\.google/.test(url) ? "scholar" : /github\\.com/.test(url) ? "github" : /openalex\\.org/.test(url) ? "openalex" : "homepage";
    const byName = (name) => Object.values(db.entities).find((e) => [e.name, ...(e.aliases ?? [])].some((n) => n.toLowerCase() === name.toLowerCase()));
    const linkOf = (l, outgoing) => ({ kind: l.kind, outgoing, other: db.entities[outgoing ? l.to : l.from], detail: l.detail ?? null, since: l.since ?? null, until: l.until ?? null });
    const partsOf = (id) => [id, ...db.links.filter((l) => l.kind === "PART_OF" && l.to === id).flatMap((l) => partsOf(l.from))];
    const kindTag = (k) => k.toLowerCase();
    const tagged = (tag) => Object.values(db.entities).filter((e) => kindTag(e.kind) === tag || e.tags.includes(tag));
    const roleOptions = [["student", "My student"], ["collaborator", "Collaborator"], ["colleague", "Colleague"], ["faculty", "Faculty elsewhere"], ["staff", "Staff"], ["alumni", "Former student"]].map(([tag, label]) => ({ tag, label }));
    const identity = [["full_name", "Full name"], ["email", "Email"], ["position", "Position"], ["affiliation", "Affiliation"], ["homepage", "Profile link"]];
    const details = (e) => {
      if (e.kind !== "Person") return null;
      const hasRole = roleOptions.some((r) => e.tags.includes(r.tag));
      const miss = (fields) => fields.filter(([k]) => !e.info[k] && !(k === "full_name" && e.name.includes(" "))).map(([key, label]) => ({ key, label, options: [] }));
      const fields = miss(identity);
      const role_fields = { student: miss([["program", "Programme"], ["start", "Started"], ["thesis", "Thesis topic"]]) };
      return !hasRole || fields.length ? { entity: e, roles: hasRole ? [] : roleOptions, fields, role_fields } : null;
    };
    const channels = {};
    // Assistant setup: pass ?setup=fresh to start with nothing installed.
    const fresh = location.search.includes("setup=fresh");
    const setup = {
      chosen: fresh ? null : "claude",
      codes: [],
      claude: { installed: !fresh, signed_in: !fresh, account: "prof@example.edu", version: "2.1.273 (Claude Code)" },
      codex: { installed: fresh, signed_in: false, account: null, version: "codex-cli 0.154.0" },
    };
    window.__setup = setup;
    const assistantInfo = (kind) => ({
      kind,
      label: kind === "claude" ? "Claude Code" : "Codex",
      installed: setup[kind].installed,
      version: setup[kind].installed ? setup[kind].version : null,
      signed_in: setup[kind].signed_in,
      account: setup[kind].signed_in ? setup[kind].account : null,
      install_command: kind === "claude" ? "curl -fsSL https://claude.ai/install.sh | bash" : "curl -fsSL https://chatgpt.com/codex/install.sh | sh",
      requirement: kind === "claude" ? "a Claude Pro or Max plan, or an Anthropic Console account" : "a ChatGPT Plus, Pro, Business or Enterprise plan",
    });
    const assistantState = () => ({
      chosen: setup.chosen,
      ready: !!setup.chosen && setup[setup.chosen].installed && setup[setup.chosen].signed_in,
      assistants: [assistantInfo("claude"), assistantInfo("codex")],
      calendar_status: "needs-auth",
      calendar_help: "To connect Google Calendar, run claude in Terminal, type /mcp, and choose \\"claude.ai Google Calendar\\".",
    });
    const status = (channel, text) => channels[channel?.id]?.({ index: 0, message: { status: text } });
    let callbackId = 0;
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    window.__TAURI_INTERNALS__ = {
      transformCallback: (cb) => { const id = ++callbackId; channels[id] = cb; return id; },
      unregisterCallback: () => {},
      invoke: async (cmd, args) => {
        window.__invokes.push({ cmd, args: JSON.parse(JSON.stringify(args ?? {}, (k, v) => (k === "onStatus" ? undefined : v))) });
        const wait = (ms) => new Promise((r) => setTimeout(r, ms));
        switch (cmd) {
          case "plugin:event|listen": (listeners[args.event] ??= []).push(args.handler); return args.handler;
          case "follow_person": {
            const e = db.entities[args.id];
            e.tags = e.tags.filter((t) => t !== "following").concat(args.follow ? ["following"] : []);
            return watchOf(e);
          }
          case "add_profile_links": for (const url of args.urls) { const k = linkKey(url); db.entities[args.id].info[k] = k === "openalex" ? url.split("/").pop() : url; } return db.entities[args.id];
          case "remove_profile_link": delete db.entities[args.id].info[args.key]; return db.entities[args.id];
          case "watch_info": return watchOf(db.entities[args.id]);
          case "openalex_candidates": await wait(300); return [
            { id: "A5023888391", name: "Lin Tan", works_count: 212, institutions: ["Purdue University West Lafayette"], orcid: null },
            { id: "A5100362120", name: "Lin Tan", works_count: 64, institutions: ["Zhejiang University"], orcid: null },
          ];
          case "check_person_now": await wait(300); return { new: 0, judged: 0, errors: [] };
          case "list_updates": return db.activities
            .filter((a) => (args.person ? a.person === args.person : !a.baseline) && (args.all || ["high", "medium"].includes(a.relevance)))
            .map((a) => ({ activity: { ...a }, person: db.entities[a.person] }));
          case "mark_updates_seen": for (const a of db.activities) if (!args.ids || args.ids.includes(a.id)) a.seen = true; setTimeout(() => window.__emit("updates-changed", null), 0); return null;
          case "plugin:event|unlisten": return null;
          case "chat_history":
            return db.messages.filter((m) => (args.focus ? m.mentions.includes(args.focus) : !m.focus));
          case "send_message": {
            status(args.onStatus, "Saving…");
            await wait(300);
            const store = (text, mentions) => {
              db.messages.push({ id: "u" + db.messages.length, at: Date.now(), role: "user", text: args.message, focus: args.focus, mentions });
              db.messages.push({ id: "a" + db.messages.length, at: Date.now(), role: "agent", text, focus: args.focus, mentions });
            };
            if (/kavya/i.test(args.message)) {
              const kavya = put("Person", "Kavya Rao", { info: { affiliation: "IISc" } });
              const text = "Saved Kavya Rao (IISc) and noted you met her at the PLDI workshop.";
              store(text, [kavya.id]);
              return { text, proposals: [], notice: null, sections: [{ tag: "person", title: "People", members: ["Kavya Rao"], count: 1 }], details: [details(kavya)] };
            }
            const text = args.focus ? "His email is " + db.entities[args.focus].info.email + "." : "Noted: " + args.message;
            store(text, [...(args.mentions ?? []), ...(args.focus ? [args.focus] : [])]);
            return { text, proposals: [], notice: null, sections: [], details: [] };
          }
          case "fill_profile": {
            status(args.onStatus, "Reading the link…");
            await wait(400);
            Object.assign(db.entities[args.id].info, { position: "Associate Professor", email: "kavya@iisc.ac.in", department: "CSA", homepage: args.url });
            db.entities[args.id].updated_at = Date.now();
            return { entity: db.entities[args.id], text: "Filled in position, department, email and homepage from her IISc page." };
          }
          case "get_sidebar": {
            const pinned = db.sections.filter((s) => s.pinned).map((s) => ({ tag: s.tag, title: s.title, count: tagged(s.tag).length }));
            const known = pinned.map((s) => s.tag);
            const tags = {};
            for (const e of Object.values(db.entities)) for (const t of [kindTag(e.kind), ...e.tags]) {
              if (["task", "event", "note"].includes(t) || known.includes(t)) continue;
              (tags[t] ??= []).push(e.name);
            }
            const titles = { project: "Projects", person: "People", collaborator: "Collaborators" };
            return { favorites: Object.values(db.entities).filter((e) => e.tags.includes("favorite")), sections: pinned.map((p) => ({ ...p, icon: db.sections.find((x) => x.tag === p.tag).icon ?? "" })), suggested: Object.entries(tags).map(([tag, names]) => ({ tag, title: titles[tag] ?? tag, members: names, count: names.length })) };
          }
          case "pin_section": {
            const s = db.sections.find((x) => x.tag === args.tag);
            if (s) Object.assign(s, { title: args.title, pinned: true }); else db.sections.push({ tag: args.tag, title: args.title, pinned: true });
            return null;
          }
          case "dismiss_section": {
            const s = db.sections.find((x) => x.tag === args.tag);
            if (s) s.pinned = false; else db.sections.push({ tag: args.tag, title: args.tag, pinned: false });
            return null;
          }
          case "list_tagged": return tagged(args.tag);
          case "set_icon": if (args.icon) db.entities[args.id].info.icon = args.icon; else delete db.entities[args.id].info.icon; return db.entities[args.id];
          case "set_section_icon": db.sections.find((x) => x.tag === args.tag).icon = args.icon ?? ""; return null;
          case "set_favorite": { const e = db.entities[args.id]; e.tags = e.tags.filter((t) => t !== "favorite").concat(args.favorite ? ["favorite"] : []); return e; }
          case "submit_details": {
            const e = db.entities[args.id];
            Object.assign(e.info, args.values);
            e.tags = [...new Set([...e.tags, ...args.roles])];
            return e;
          }
          case "skip_details": db.entities[args.id].info.details_skipped = "yes"; return db.entities[args.id];
          case "search": {
            const q = args.query.toLowerCase();
            return Object.values(db.entities)
              .filter((e) => e.name.toLowerCase().includes(q) || (e.aliases ?? []).some((a) => a.toLowerCase().includes(q)) || Object.values(e.info).some((v) => v.toLowerCase().includes(q)))
              // As the real search ranks: names, then other names, then details.
              .sort((a, b) => {
                const rank = (e) => (e.name.toLowerCase().startsWith(q) ? 0 : (e.aliases ?? []).some((x) => x.toLowerCase().startsWith(q)) ? 1 : 2);
                return rank(a) - rank(b);
              })
              .map((e) => {
                const alias = (e.aliases ?? []).find((a) => a.toLowerCase().includes(q));
                return e.name.toLowerCase().includes(q) ? { entity: e, field: "name", snippet: null } : alias ? { entity: e, field: "alias", snippet: alias } : { entity: e, field: "full_name", snippet: null };
              });
          }
          case "get_entity": {
            const e = db.entities[args.id];
            const links = [
              ...db.links.filter((l) => l.from === e.id).map((l) => linkOf(l, true)),
              ...db.links.filter((l) => l.to === e.id).map((l) => linkOf(l, false)),
            ];
            const orgs = e.kind === "Organization" ? partsOf(e.id) : [];
            const affiliations = db.links
              .filter((l) => ["AFFILIATED_WITH", "STUDIED_AT"].includes(l.kind) && orgs.includes(l.to))
              .map((l) => ({ person: db.entities[l.from], kind: l.kind, organization: db.entities[l.to], detail: l.detail ?? null, since: l.since ?? null, until: l.until ?? null }));
            const hasLink = db.links.some((l) => l.from === e.id && l.kind === "AFFILIATED_WITH");
            const unlinked = e.kind === "Person" && !hasLink && e.info.affiliation ? [e.info.affiliation, byName(e.info.affiliation) ?? null] : null;
            return { entity: e, links, details: details(e), affiliations, unlinked_affiliation: unlinked };
          }
          case "save_notes": db.entities[args.id].notes = args.notes; return db.entities[args.id];
          case "set_tags": db.entities[args.id].tags = args.tags.map((t) => t.toLowerCase()); return db.entities[args.id];
          case "open_page": return byName(args.name) ?? put(args.kind ?? "Note", args.name);
          case "rename_page": db.entities[args.id].name = args.name; return db.entities[args.id];
          case "delete_page": delete db.entities[args.id]; return null;
          case "get_vault": return { path: "/Users/prof/Documents/Professor OS", registered: true };
          case "open_in_obsidian": case "open_url": return null;
          case "unlinked_affiliations":
            return Object.values(db.entities)
              .filter((e) => e.kind === "Person" && e.info.affiliation && !db.links.some((l) => l.from === e.id && l.kind === "AFFILIATED_WITH"))
              .map((e) => ({ person: e, text: e.info.affiliation, organization: byName(e.info.affiliation) ?? null }));
          case "link_affiliation": {
            const org = byName(args.organization) ?? put("Organization", args.organization);
            db.links.push({ from: args.person, kind: "AFFILIATED_WITH", to: org.id });
            db.entities[args.person].info.affiliation = org.name;
            return org;
          }
          case "set_aliases": db.entities[args.id].aliases = args.aliases; return db.entities[args.id];
          case "list_tasks": {
            const linked = (t) => db.links.some((l) => ["FOR", "WAITING_ON", "HAS_TASK", "ASSIGNED_TO"].includes(l.kind) && ((l.from === t.id && l.to === args.about) || (l.to === t.id && l.from === args.about)));
            const rank = { high: 0, medium: 1, low: 2 };
            return Object.values(db.entities)
              .filter((t) => t.kind === "Task")
              .filter((t) => args.status === "all" || (args.status === "done") === (t.info.status === "done"))
              .filter((t) => !args.about || linked(t))
              .filter((t) => {
                const has = db.links.some((l) => l.from === t.id && l.kind === "ASSIGNED_TO");
                return !args.assigned || (args.assigned === "others") === has;
              })
              .sort((a, b) => (a.info.due ? 0 : 1) - (b.info.due ? 0 : 1) || (a.info.due ?? "").localeCompare(b.info.due ?? "") || (rank[a.info.priority] ?? 1) - (rank[b.info.priority] ?? 1))
              .map((t) => ({
                task: t,
                assigned_to: db.links.filter((l) => l.from === t.id && l.kind === "ASSIGNED_TO").map((l) => db.entities[l.to]),
                for: db.links.filter((l) => l.from === t.id && l.kind === "FOR").map((l) => db.entities[l.to]),
                waiting_on: db.links.filter((l) => l.from === t.id && l.kind === "WAITING_ON").map((l) => db.entities[l.to]),
                part_of: db.links.filter((l) => l.to === t.id && l.kind === "HAS_TASK").map((l) => db.entities[l.from]),
              }));
          }
          case "assign_task":
            if (args.assigned) db.links.push({ from: args.task, kind: "ASSIGNED_TO", to: args.person });
            else db.links = db.links.filter((l) => !(l.from === args.task && l.kind === "ASSIGNED_TO" && l.to === args.person));
            return null;
          case "set_task_done": db.entities[args.id].info.status = args.done ? "done" : "open"; return db.entities[args.id];
          case "list_entities": return Object.values(db.entities).filter((e) => e.kind === args.kind);
          case "assistant_status": return assistantState();
          case "choose_assistant": setup.chosen = args.kind; return null;
          case "install_assistant": {
            let i = 0;
            for (const line of ["Setting up Claude Code...", "Downloading claude 2.1.273 for darwin-arm64", "✔ Claude Code successfully installed!", "Location: ~/.local/bin/claude"]) {
              await wait(250);
              channels[args.onOutput?.id]?.({ index: i++, message: { line } });
            }
            setup[args.kind].installed = true;
            return assistantInfo(args.kind);
          }
          case "sign_in_assistant": {
            await wait(300);
            const prompt = args.kind === "claude"
              ? { url: "https://claude.com/cai/oauth/authorize?code=true", code: null, needs_code: true }
              : { url: "https://auth.openai.com/codex/device", code: "IB3U-Y26HE", needs_code: false };
            channels[args.onPrompt?.id]?.({ index: 0, message: prompt });
            await new Promise((resolve, reject) => { setup.finishSignIn = resolve; setup.cancelSignIn = reject; });
            setup[args.kind].signed_in = true;
            return assistantInfo(args.kind);
          }
          case "submit_sign_in_code": setup.codes.push(args.code); setTimeout(() => setup.finishSignIn?.(), 300); return null;
          case "cancel_sign_in": setup.cancelSignIn?.(new Error("Sign-in was cancelled.")); return null;
        }
        throw new Error("unexpected command " + cmd);
      },
    };
  `,
});
// First run: nothing set up. Install Claude Code, sign in with a pasted code, start.
await send("Page.navigate", { url: "http://localhost:1420/?setup=fresh" });
await sleep(2500);
const setupResults = {};
setupResults.options = await texts(".setup-option");
setupResults.steps = await texts(".setup-steps > li .setup-step-title");
setupResults.need = await evaluate("document.querySelector('.setup-need')?.textContent");
setupResults.startDisabled = await evaluate("document.querySelector('.setup-footer .button.primary')?.disabled");
await shot("0a-setup-fresh");
await click(".setup-steps .button.primary", "Install");
await sleep(500);
setupResults.installing = await evaluate("({ button: document.querySelector('.setup-steps .button.primary')?.textContent, log: document.querySelector('.setup-log')?.textContent })");
await shot("0b-setup-installing");
await sleep(900);
setupResults.afterInstall = await texts(".setup-steps > li .setup-step-title");
await click(".setup-steps .button.primary", "Sign in");
await sleep(600);
setupResults.signIn = await evaluate("document.querySelector('.setup-signin')?.textContent");
await shot("0c-setup-sign-in");
await focus(".setup-code-form input");
await type("abc123#state");
await key("Enter");
await sleep(900);
setupResults.codes = await evaluate("window.__setup.codes");
setupResults.afterSignIn = await evaluate("({ steps: [...document.querySelectorAll('.setup-steps > li .setup-step-title')].map(t => t.textContent), start: document.querySelector('.setup-footer .button.primary')?.textContent, disabled: document.querySelector('.setup-footer .button.primary')?.disabled })");
await shot("0d-setup-ready");
// Codex shows the device code to enter in the browser.
await click(".setup-option", "Codex");
await sleep(200);
await click(".setup-steps .button.primary", "Sign in");
await sleep(600);
setupResults.codexCode = await evaluate("document.querySelector('.setup-code')?.textContent");
await shot("0e-setup-codex-code");
await click(".setup-actions .text-button", "Cancel");
await sleep(300);
await click(".setup-option", "Claude Code");
await sleep(200);
await click(".setup-footer .button.primary", "Start");
await sleep(1200);
setupResults.inApp = await evaluate("({ sidebar: !!document.querySelector('.sidebar'), chosen: window.__setup.chosen })");
await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "dark" }] });
await send("Page.navigate", { url: "http://localhost:1420/?setup=fresh" });
await sleep(2000);
await shot("0f-setup-dark");
await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "light" }] });

await send("Page.navigate", { url: "http://localhost:1420/" });
await sleep(2500);

const results = {};
await shot("1-chat-empty");

// 1. A new person: reply, profile form with roles, section offer.
await focus(".composer textarea");
await type("I met Dr. Kavya Rao from IISc at PLDI today");
await sleep(300);
results.popoverForNewName = await texts(".mention-option");
await key("Escape");
await key("Enter");
await sleep(800);
results.reply = await texts(".message.agent");
results.formTitle = await texts(".profile-form .card-title");
results.roleChoices = await texts(".choice-chip");
results.formFields = await texts(".form-fields label span");
await shot("2-new-person");

// 2. Choose a role, fill from a link, save.
await click(".choice-chip", "Collaborator");
await evaluate("document.querySelector('.link-fill input').focus()");
await type("https://iisc.ac.in/~kavya");
await click(".link-fill button");
await sleep(150);
results.fillingStatus = await texts(".profile-form .busy");
await sleep(700);
results.filledNote = await texts(".form-note");
results.filledValues = await evaluate("[...document.querySelectorAll('.form-fields input')].map(i => i.value)");
await shot("3-filled");
await click(".profile-form .button.primary", "Save");
await sleep(400);
results.afterSave = await texts(".card-done");
results.submitted = await evaluate("window.__invokes.filter(i => i.cmd === 'submit_details').at(-1)?.args");

// 3. Mention existing pages while typing, with Tab.
await focus(".composer textarea");
await type("Sat");
await sleep(400);
results.satPopover = await texts(".mention-option");
await shot("4-mention-popover");
await key("Tab");
await sleep(150);
await type("is working on Fuz");
await sleep(400);
results.fuzPopover = await texts(".mention-option");
// Enter while a suggestion shows picks it rather than sending.
const sendsBefore = await evaluate("window.__invokes.filter(i => i.cmd === 'send_message').length");
await key("Enter");
await sleep(150);
results.enterPickedInsteadOfSending = await evaluate(`window.__invokes.filter(i => i.cmd === 'send_message').length === ${sendsBefore} && document.querySelector('.composer textarea').value.endsWith('Fuzzing ') && !document.querySelector('.mention-popover')`);
await type("with a new LLVM target");
results.composerText = await evaluate("document.querySelector('.composer textarea').value");
results.highlighted = await texts(".composer-backdrop mark");
results.mentionChips = await texts(".mention-chip");
await shot("5-mentions-picked");
// Typing on past a suggestion and pressing Enter at once never picks the stale suggestion.
await type(" Sat");
await sleep(400);
await send("Input.insertText", { text: "urday" });
await key("Enter");
await sleep(150);
results.staleEnterSent = await evaluate("window.__invokes.filter(i => i.cmd === 'send_message').at(-1).args");
await sleep(600);
await focus(".composer textarea");
// Esc closes a suggestion, and Enter then sends.
await type(" Meer");
await sleep(400);
results.meerPopover = await texts(".mention-option");
await key("Escape");
await sleep(100);
results.popoverAfterEscape = await evaluate("!!document.querySelector('.mention-popover')");
await key("Enter");
await sleep(700);
results.sentText = await evaluate("window.__invokes.filter(i => i.cmd === 'send_message').at(-1).args.message");
results.sentMentions = await evaluate("window.__invokes.filter(i => i.cmd === 'send_message').at(-1).args.mentions");

// 4. Search and open Kavya's page.
await key("k", 4, "KeyK");
await sleep(200);
await type("kav");
await sleep(400);
results.search = await texts(".palette-results li");
await shot("6-search");
await key("Enter");
await sleep(700);
results.page = await evaluate(`({
  title: document.querySelector('.page-title')?.textContent,
  sub: document.querySelector('.page-sub')?.textContent,
  chips: [...document.querySelectorAll('.page-hero .chip')].map(c => c.textContent),
  props: [...document.querySelectorAll('.prop')].map(p => p.textContent),
  profileForm: !!document.querySelector('.profile-form.page'),
  focus: document.querySelector('.chat-focus')?.textContent,
  history: [...document.querySelectorAll('.page-chat .message')].map(m => m.textContent),
})`);
await shot("7-person-page");

// 5. Satya's page, the Students section, and dark mode.
await click(".nav-item", "Students");
await sleep(400);
results.students = await texts(".list .row");
await shot("8-students");
await click(".list .row", "Satya");
await sleep(600);
results.satyaPage = await evaluate("({ sub: document.querySelector('.page-sub')?.textContent, links: [...document.querySelectorAll('.link-card')].map(c => c.textContent), form: !!document.querySelector('.profile-form'), addDetails: document.querySelector('.add-details')?.textContent })");
await shot("9-satya");
await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "dark" }] });
await sleep(300);
await shot("10-satya-dark");
await click(".nav-item", "Chat");
await sleep(400);
await shot("11-chat-dark");
await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "light" }] });
await click(".nav-item", "Settings");
await sleep(500);
results.settings = await texts(".settings-row");
await shot("11b-settings");
await click(".nav-item", "Today");
await sleep(500);
results.today = await evaluate(`({
  summary: document.querySelector('.page-header .muted')?.textContent,
  groups: [...document.querySelectorAll('.task-groups section')].map(s => ({
    title: s.querySelector('h2').textContent,
    tasks: [...s.querySelectorAll('.task')].map(t => ({ title: t.querySelector('.task-title').textContent, meta: t.querySelector('.task-meta').textContent, due: t.querySelector('.task-due')?.textContent })),
  })),
})`);
await shot("12-today");

// Tick a task off: crossed out, saved, still visible until the list reloads.
await click(".task-check", "");
await evaluate("[...document.querySelectorAll('.task')].find(t => t.textContent.includes('Review Satya')).querySelector('.task-check').click()");
await sleep(300);
results.ticked = await evaluate(`({
  row: [...document.querySelectorAll('.task')].find(t => t.textContent.includes('Review Satya'))?.className,
  saved: window.__invokes.filter(i => i.cmd === 'set_task_done').map(i => i.args),
})`);
await shot("13-ticked");
await evaluate("[...document.querySelectorAll('.task')].find(t => t.textContent.includes('Review Satya')).querySelector('.task-check').click()");
await sleep(300);

// Tasks on pages: Satya's open tasks (via the chip), the course's, and a task page's checkbox.
await evaluate("[...document.querySelectorAll('.task-chip')].find(c => c.textContent.includes('Satya')).click()");
await sleep(600);
results.satyaTasks = await evaluate(`({
  title: document.querySelector('.page-title')?.textContent,
  sections: [...document.querySelectorAll('.page-main h2')].map(h => h.textContent),
  tasks: [...document.querySelectorAll('.page-main .task')].map(t => t.textContent),
})`);
await shot("14-satya-tasks");
await evaluate("document.querySelector('.page-main .task-title').click()");
await sleep(600);
results.taskPage = await evaluate(`({
  title: document.querySelector('.page-title')?.textContent,
  check: document.querySelector('.page-title .task-check')?.getAttribute('aria-checked'),
  sections: [...document.querySelectorAll('.page-main h2')].map(h => h.textContent),
})`);
await evaluate("document.querySelector('.page-title .task-check').click()");
await sleep(500);
results.taskPageAfterTick = await evaluate("({ check: document.querySelector('.page-title .task-check')?.getAttribute('aria-checked'), done: document.querySelector('.page-title')?.className })");
await shot("15-task-page");

// Organizations: alias search, the organization page, a person's affiliation, and linking text.
await key("k", 4, "KeyK");
await sleep(200);
await type("iitg");
await sleep(400);
results.orgSearch = await texts(".palette-results li");
await key("Enter");
await sleep(700);
results.orgPage = await evaluate(`({
  title: document.querySelector('.page-title')?.textContent,
  sub: document.querySelector('.page-sub')?.textContent,
  aliases: [...document.querySelectorAll('.chips.aliases .chip:not(.add)')].map(c => c.textContent),
  sections: [...document.querySelectorAll('.page-main h2')].map(h => ({ title: h.textContent, cards: [...h.parentElement.querySelectorAll('.link-card')].map(c => c.textContent) })),
})`);
await shot("16-organization");
await evaluate("[...document.querySelectorAll('.link-card')].find(c => c.textContent.includes('Arun')).click()");
await sleep(600);
results.arunPage = await evaluate(`({
  affiliation: [...document.querySelectorAll('.prop')].find(p => p.textContent.startsWith('Affiliation'))?.textContent,
  sections: [...document.querySelectorAll('.page-main h2')].map(h => ({ title: h.textContent, cards: [...h.parentElement.querySelectorAll('.link-card')].map(c => c.textContent) })),
})`);
await shot("17-person-affiliation");
await click(".nav-item", "Students");
await sleep(400);
await click(".list .row", "Meera");
await sleep(600);
results.meeraBefore = await evaluate("[...document.querySelectorAll('.prop')].find(p => p.textContent.startsWith('Affiliation'))?.textContent");
await shot("18-unlinked");
await click(".prop .button", "Link to");
await sleep(600);
results.meeraAfter = await evaluate("[...document.querySelectorAll('.prop')].find(p => p.textContent.startsWith('Affiliation'))?.textContent");
results.linkCalls = await evaluate("window.__invokes.filter(i => i.cmd === 'link_affiliation').map(i => i.args)");

// Assigned tasks: Today keeps them apart; a task page assigns more people; the person's page shows it.
await click(".nav-item", "Today");
await sleep(600);
results.todayAssigned = await evaluate(`({
  headings: [...document.querySelectorAll('.page h2')].map(h => h.textContent),
  lastList: [...document.querySelectorAll('.task-list')].at(-1)?.textContent,
})`);
await shot("19-today-assigned");
await evaluate("[...document.querySelectorAll('.task-title')].find(t => t.textContent.includes('Solidity')).click()");
await sleep(600);
results.assignBefore = await texts(".page-hero .chip.assignee");
await click(".page-hero .chip.add", "Assign");
await sleep(300);
await type("Sat");
await sleep(200);
results.assignOptions = await texts(".assign-menu button");
await shot("20-assign-menu");
await click(".assign-menu button", "Satya");
await sleep(600);
results.assignAfter = await texts(".page-hero .chip.assignee");
results.assignCalls = await evaluate("window.__invokes.filter(i => i.cmd === 'assign_task').map(i => i.args)");
await shot("21-assigned");
await evaluate("[...document.querySelectorAll('.page-hero .chip.assignee .assignee-name')].find(b => b.textContent.includes('Satya')).click()");
await sleep(700);
results.satyaAssigned = await evaluate("[...document.querySelectorAll('.page-main .task')].find(t => t.textContent.includes('Solidity'))?.textContent");

// Following people: Today's section, the Updates badge and view, a followed person's page,
// following someone new and choosing their OpenAlex author, and an update arriving.
await click(".nav-item", "Today");
await sleep(600);
results.todayUpdates = await texts(".page .updates .update-title");
results.badgeBefore = await texts(".nav-item .nav-badge");
await shot("22-today-updates");
await click(".nav-item", "Updates");
await sleep(700);
results.updates = await evaluate(`[...document.querySelectorAll('.update')].map(u => ({ head: u.querySelector('.update-head').textContent, title: u.querySelector('.update-title').textContent, why: u.querySelector('.update-why')?.textContent, fresh: u.classList.contains('fresh') }))`);
results.seenCalls = await evaluate("window.__invokes.filter(i => i.cmd === 'mark_updates_seen').map(i => i.args)");
await shot("23-updates");
await sleep(200);
results.badgeAfter = await texts(".nav-item .nav-badge");
await click(".segmented button", "Everything");
await sleep(500);
results.everything = await texts(".update .relevance");
await shot("24-updates-everything");
await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "dark" }] });
await sleep(300);
await shot("25-updates-dark");
await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "light" }] });
await click(".update-person", "Andreas");
await sleep(800);
results.zellerPage = await evaluate(`({
  props: [...document.querySelectorAll('.prop')].map(p => p.textContent),
  links: [...document.querySelectorAll('.profile-link')].map(c => c.textContent),
  follow: document.querySelector('.follow-panel .section-title-row .button')?.textContent,
  status: document.querySelector('.watch-status')?.textContent,
  activity: [...document.querySelectorAll('.follow-panel .update-title')].map(t => t.textContent),
})`);
await shot("26-followed-person");
await evaluate("document.querySelector('.follow-panel').scrollIntoView()");
await sleep(200);
await shot("27-followed-person-activity");

await key("k", 4, "KeyK");
await sleep(200);
await type("lin tan");
await sleep(400);
await key("Enter");
await sleep(700);
results.linBefore = await evaluate("({ follow: document.querySelector('.follow-panel .section-title-row .button')?.textContent, links: [...document.querySelectorAll('.profile-link')].map(c => c.textContent) })");
await click(".follow-panel .button.primary", "Follow");
await sleep(500);
results.linFollowing = await evaluate("({ follow: document.querySelector('.follow-panel .section-title-row .button')?.textContent, status: document.querySelector('.watch-status')?.textContent, note: document.querySelector('.follow-panel .form-note')?.textContent })");
await click(".watch-actions .button", "papers");
await sleep(600);
results.candidates = await texts(".candidate-list li");
await shot("28-candidates");
await click(".candidate-list .button", "This is them");
await sleep(900);
results.linAfter = await evaluate("({ status: document.querySelector('.watch-status')?.textContent, links: [...document.querySelectorAll('.profile-link')].map(c => c.textContent), note: document.querySelector('.follow-panel .form-note')?.textContent })");
results.followCalls = await evaluate("window.__invokes.filter(i => ['follow_person', 'add_profile_links', 'check_person_now'].includes(i.cmd)).map(i => [i.cmd, i.args])");
await click(".follow-panel .chip.add", "Link");
await sleep(150);
await type("https://x.com/lintan");
await key("Enter");
await sleep(500);
results.linLinks = await evaluate("({ links: [...document.querySelectorAll('.profile-link')].map(c => c.textContent), status: document.querySelector('.watch-status')?.textContent })");
await shot("29-followed-new");

await evaluate(`window.__emit("updates-arrived", ${JSON.stringify({ title: "New paper by Andreas Zeller", body: "Finding Miscompilations in Solidity Compilers with Grammar-Based Fuzzing\nGrammar-based fuzzing of solc, the same approach as your project", count: 1 })})`);
await sleep(400);
results.toast = await texts(".toast");
await shot("30-toast");
await click(".toast .button", "View");
await sleep(500);
results.afterToast = await evaluate("({ heading: document.querySelector('.page h1')?.textContent, toast: !!document.querySelector('.toast') })");

// Projects: grouped by status in the section; a project's status changed from its header.
await click(".nav-item", "Projects");
await sleep(600);
results.projectGroups = await evaluate(`[...document.querySelectorAll('.page section')].map(s => ({ title: s.querySelector('h2').textContent, rows: [...s.querySelectorAll('.row')].map(r => r.textContent) }))`);
await shot("31-projects");
await click(".row", "LLM Static Analysis");
await sleep(600);
results.projectPage = await evaluate("({ pill: document.querySelector('.status-pill')?.textContent, sub: document.querySelector('.page-sub')?.textContent, props: [...document.querySelectorAll('.prop')].map(p => p.textContent) })");
await click(".status-pill");
await sleep(200);
results.statusMenu = await evaluate("[...document.querySelectorAll('.menu [role=menuitemradio]')].map(b => b.textContent + (b.getAttribute('aria-checked') === 'true' ? ' ✓' : ''))");
await shot("32-status-menu");
await click(".menu [role=menuitemradio]", "In progress");
await sleep(600);
results.projectAfter = await evaluate("({ pill: document.querySelector('.status-pill')?.textContent, saved: window.__invokes.filter(i => i.cmd === 'submit_details').at(-1)?.args })");
await click(".nav-item", "Projects");
await sleep(600);
results.projectGroupsAfter = await evaluate(`[...document.querySelectorAll('.page section')].map(s => s.querySelector('h2').textContent)`);
await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "dark" }] });
await sleep(300);
await shot("33-projects-dark");
await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "light" }] });

// Design: page icons (suggested, searched), favorites, section icons, ⌘K commands, New menu,
// quick add on Today, and appearance.
await click(".nav-item", "Projects");
await sleep(500);
await click(".row", "Fuzzing");
await sleep(600);
await click(".hero-icon");
await sleep(700);
results.suggestedIcons = await texts(".emoji-grid.suggested .emoji-cell");
results.pickerTabs = await evaluate("document.querySelectorAll('.icon-picker-tabs button').length");
await shot("34-icon-picker");
await type("rocket");
await sleep(300);
results.rocketSearch = await evaluate("[...document.querySelectorAll('.emoji-grid .emoji-cell')].slice(0, 5).map(b => b.textContent)");
await key("Enter");
await sleep(500);
results.iconAfterSearch = await evaluate("({ hero: document.querySelector('.hero-icon .avatar')?.textContent, saved: window.__invokes.filter(i => i.cmd === 'set_icon').at(-1)?.args })");
await click(".hero-icon");
await sleep(400);
await click(".emoji-grid.suggested .emoji-cell");
await sleep(500);
results.iconAfterSuggested = await evaluate("document.querySelector('.hero-icon .avatar')?.textContent");
await click(".icon-button.favorite");
await sleep(500);
results.favorites = await evaluate("[...document.querySelectorAll('.nav-group')].find(g => g.textContent.startsWith('Favorites'))?.textContent");
results.starOn = await evaluate("document.querySelector('.icon-button.favorite')?.getAttribute('aria-pressed')");
await shot("35-project-icon-favorite");

await click(".nav-item", "Projects");
await sleep(500);
await click("button.page-icon");
await sleep(600);
results.sectionSuggested = await texts(".emoji-grid.suggested .emoji-cell");
await click(".emoji-grid.suggested .emoji-cell");
await sleep(500);
results.sectionIcon = await evaluate("({ header: document.querySelector('button.page-icon')?.textContent, nav: [...document.querySelectorAll('.nav-item')].find(n => n.textContent.includes('Projects'))?.querySelector('.nav-emoji')?.textContent })");
await shot("36-projects-icons");

await key("k", 4, "KeyK");
await sleep(300);
results.paletteEmpty = await texts(".palette-group-label");
results.paletteCommands = await texts(".palette-results .palette-name");
await shot("37-palette-commands");
await type("grant review");
await sleep(500);
results.paletteTyped = await evaluate("[...document.querySelectorAll('.palette-group')].map(g => g.querySelector('.palette-group-label').textContent + ': ' + [...g.querySelectorAll('.palette-name')].map(n => n.textContent).join(' | '))");
await shot("38-palette-create");
await key("Escape");
await sleep(200);

await click(".brand .new-button");
await sleep(200);
await click(".menu button", "Idea");
await sleep(200);
await type("Differential testing of ZK circuits");
await key("Enter");
await sleep(700);
results.newIdea = await evaluate("({ title: document.querySelector('.page-title')?.textContent, calls: window.__invokes.filter(i => i.cmd === 'open_page').at(-1)?.args })");

await click(".nav-item", "Today");
await sleep(600);
results.greeting = await evaluate("({ eyebrow: document.querySelector('.eyebrow')?.textContent, h1: document.querySelector('.page h1')?.textContent })");
await focus(".quick-add input");
await type("Email Kavya about the grant");
await sleep(200);
results.whenChips = await texts(".quick-add-when button");
await click(".quick-add-when button", "Tomorrow");
await shot("39-quick-add");
await focus(".quick-add input");
await key("Enter");
await sleep(800);
results.quickAdded = await evaluate("({ input: document.querySelector('.quick-add input').value, tomorrow: [...document.querySelectorAll('.task-groups section')].find(s => s.textContent.startsWith('Tomorrow'))?.textContent, saved: window.__invokes.filter(i => i.cmd === 'submit_details').at(-1)?.args })");
await shot("40-today-after-add");

await click(".nav-item", "Settings");
await sleep(500);
await click(".swatch[aria-label=Rose]");
await click(".segmented button", "Dark");
await sleep(300);
results.appearance = await evaluate("({ theme: document.documentElement.dataset.theme, accent: document.documentElement.dataset.accent, accentColor: getComputedStyle(document.documentElement).getPropertyValue('--accent').trim() })");
await shot("41-settings-dark-rose");
await click(".nav-item", "Today");
await sleep(600);
await shot("42-today-dark-rose");
await click(".nav-item", "Settings");
await sleep(300);
await click(".segmented button", "System");
await click(".swatch[aria-label=Indigo]");

results.setup = setupResults;
results.consoleErrors = consoleErrors;
console.log(JSON.stringify(results, null, 2));
ws.close();
