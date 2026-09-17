import type { DetailsRequest, Proposal, SectionIdea } from "./api";

/** Where the main area is. Sections and pages come from the graph, so the sidebar isn't fixed. */
export type Route =
  | { view: "today" }
  | { view: "chat" }
  | { view: "notes" }
  | { view: "updates" }
  | { view: "settings" }
  | { view: "section"; tag: string; title: string }
  | { view: "page"; id: string };

export interface ChatMessage {
  id: string | number;
  role: "user" | "agent";
  text: string;
  proposals?: Proposal[];
  /** Sidebar sections offered for what the message saved. */
  sections?: SectionIdea[];
  /** Forms for details of newly saved students and people. */
  details?: DetailsRequest[];
  /** Shown under a reply, e.g. when Claude couldn't answer. */
  notice?: string | null;
}
