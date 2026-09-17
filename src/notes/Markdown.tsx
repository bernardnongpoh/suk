import type { ReactNode } from "react";

// A small Markdown renderer for notes: headings, lists, checkboxes, quotes, code, bold, italic,
// inline code, #tags and [[wikilinks]]. It builds React elements, never raw HTML.

interface Props {
  text: string;
  onLink: (name: string) => void;
}

const INLINE = /(\[\[[^\]\n]+\]\]|\*\*[^*\n]+\*\*|\*[^*\s][^*\n]*\*|`[^`\n]+`|#[A-Za-z][\w/-]*)/g;

function inline(text: string, onLink: (name: string) => void, key: string): ReactNode[] {
  const out: ReactNode[] = [];
  let last = 0;
  for (const match of text.matchAll(INLINE)) {
    const token = match[0];
    const at = match.index ?? 0;
    // A # only starts a tag at the start of a word, as in Obsidian.
    if (token.startsWith("#") && at > 0 && !/\s/.test(text[at - 1])) continue;
    out.push(text.slice(last, at));
    const k = `${key}-${at}`;
    if (token.startsWith("[[")) {
      const [name, shown] = token.slice(2, -2).split("|");
      out.push(
        <button key={k} className="wikilink" onClick={(e) => { e.stopPropagation(); onLink(name.trim()); }}>
          {(shown ?? name).trim()}
        </button>,
      );
    } else if (token.startsWith("**")) {
      out.push(<strong key={k}>{token.slice(2, -2)}</strong>);
    } else if (token.startsWith("*")) {
      out.push(<em key={k}>{token.slice(1, -1)}</em>);
    } else if (token.startsWith("`")) {
      out.push(<code key={k}>{token.slice(1, -1)}</code>);
    } else {
      out.push(<span key={k} className="md-tag">{token}</span>);
    }
    last = at + token.length;
  }
  out.push(text.slice(last));
  return out;
}

function Markdown({ text, onLink }: Props) {
  const blocks: ReactNode[] = [];
  const lines = text.split("\n");
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    const key = `b${i}`;
    if (line.startsWith("```")) {
      const code: string[] = [];
      i++;
      while (i < lines.length && !lines[i].startsWith("```")) code.push(lines[i++]);
      i++;
      blocks.push(<pre key={key}><code>{code.join("\n")}</code></pre>);
      continue;
    }
    const heading = /^(#{1,4})\s+(.*)$/.exec(line);
    if (heading) {
      const level = Math.min(heading[1].length + 1, 5);
      const Tag = `h${level}` as "h2";
      blocks.push(<Tag key={key}>{inline(heading[2], onLink, key)}</Tag>);
      i++;
      continue;
    }
    if (/^\s*([-*+]|\d+\.)\s+/.test(line)) {
      const items: ReactNode[] = [];
      const ordered = /^\s*\d+\./.test(line);
      while (i < lines.length && /^\s*([-*+]|\d+\.)\s+/.test(lines[i])) {
        const item = /^(\s*)(?:[-*+]|\d+\.)\s+(?:\[( |x|X)\]\s+)?(.*)$/.exec(lines[i])!;
        const depth = Math.floor(item[1].replace(/\t/g, "  ").length / 2);
        items.push(
          <li key={`${key}-${i}`} style={depth ? { marginLeft: depth * 18 } : undefined}>
            {item[2] !== undefined && <input type="checkbox" checked={item[2] !== " "} readOnly />}
            {inline(item[3], onLink, `${key}-${i}`)}
          </li>,
        );
        i++;
      }
      blocks.push(ordered ? <ol key={key}>{items}</ol> : <ul key={key}>{items}</ul>);
      continue;
    }
    if (line.startsWith(">")) {
      const quote: string[] = [];
      while (i < lines.length && lines[i].startsWith(">")) quote.push(lines[i++].replace(/^>\s?/, ""));
      blocks.push(<blockquote key={key}>{inline(quote.join(" "), onLink, key)}</blockquote>);
      continue;
    }
    if (line.trim() === "") {
      i++;
      continue;
    }
    // A paragraph keeps its single line breaks, as Obsidian shows them.
    const para: ReactNode[] = [];
    while (i < lines.length && lines[i].trim() !== "" && !/^(#{1,4}\s|```|>|\s*([-*+]|\d+\.)\s)/.test(lines[i])) {
      if (para.length) para.push(<br key={`br${i}`} />);
      para.push(...inline(lines[i], onLink, `p${i}`));
      i++;
    }
    blocks.push(<p key={key}>{para}</p>);
  }
  return <div className="markdown">{blocks}</div>;
}

export default Markdown;
