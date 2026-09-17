import type { Entity } from "../api";
import Icon from "./Icon";
import { iconFor } from "./entity";

interface Props {
  entity: Pick<Entity, "kind" | "name"> & { info?: Record<string, string> };
  size?: number;
}

/** A stable hue for a name, so a person always gets the same color. */
function hue(name: string) {
  let h = 0;
  for (const c of name) h = (h * 31 + c.charCodeAt(0)) % 360;
  return h;
}

function initials(name: string) {
  const words = name
    .replace(/^(dr|prof|mr|ms|mrs)\.?\s+/i, "")
    .split(/\s+/)
    .filter(Boolean);
  const letters = words.length > 1 ? words[0][0] + words[words.length - 1][0] : (words[0] ?? "?").slice(0, 2);
  return letters.toUpperCase();
}

/**
 * A page's chosen emoji; otherwise people get initials in a colored circle and other pages their
 * kind's icon in a rounded square.
 */
function Avatar({ entity, size = 28 }: Props) {
  const emoji = entity.info?.icon;
  if (emoji) {
    return (
      <span className="avatar emoji" style={{ width: size, height: size, fontSize: Math.round(size * 0.62) }} aria-hidden="true">
        {emoji}
      </span>
    );
  }
  const person = entity.kind === "Person";
  const style = {
    width: size,
    height: size,
    fontSize: size * 0.38,
    "--avatar-hue": hue(entity.name),
  } as React.CSSProperties;
  return (
    <span className={`avatar${person ? " person" : ""}`} style={style} aria-hidden="true">
      {person ? initials(entity.name) : <Icon name={iconFor(entity as Entity)} size={Math.round(size * 0.55)} />}
    </span>
  );
}

export default Avatar;
