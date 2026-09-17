import { useEffect, useState } from "react";
import { fillProfile, listEntities, skipDetails, submitDetails, type DetailsRequest, type Entity, type Field } from "../api";
import Avatar from "../ui/Avatar";
import Icon from "../ui/Icon";
import { iconForTag } from "../ui/entity";

interface Props {
  request: DetailsRequest;
  /** In chat the form can be skipped for good; on the page it just closes. */
  place: "chat" | "page";
  onChange: () => void;
  onOpen?: (id: string) => void;
  onClose?: () => void;
}

/**
 * Asks who a person is: how they're connected to the professor, and whatever details are still
 * missing. A profile link fills them in. Everything is optional.
 */
function ProfileForm({ request, place, onChange, onOpen, onClose }: Props) {
  const [entity, setEntity] = useState<Entity>(request.entity);
  // Existing organizations, by every name, offered for Affiliation.
  const [organizations, setOrganizations] = useState<string[]>([]);
  useEffect(() => {
    listEntities("Organization").then(
      (orgs) => setOrganizations(orgs.flatMap((o) => [o.name, ...o.aliases])),
      () => {},
    );
  }, []);
  const optionsFor = (field: Field) => (field.key === "affiliation" ? [...organizations, ...field.options] : field.options);
  const [roles, setRoles] = useState<string[]>([]);
  const [values, setValues] = useState<Record<string, string>>({});
  const [link, setLink] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [filled, setFilled] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<"saved" | "skipped" | null>(null);

  const first = entity.name.split(/\s+/)[0];
  const fields: Field[] = [...request.fields];
  for (const role of roles) {
    for (const field of request.role_fields[role] ?? []) {
      if (!fields.some((f) => f.key === field.key)) fields.push(field);
    }
  }
  const value = (key: string) => values[key] ?? entity.info[key] ?? "";
  const changed = roles.length > 0 || Object.entries(values).some(([k, v]) => v.trim() && v !== (entity.info[k] ?? ""));

  async function fill() {
    const url = link.trim();
    if (!url) return;
    setBusy("Opening the link…");
    setError(null);
    setFilled(null);
    try {
      const result = await fillProfile(entity.id, url, setBusy);
      setEntity(result.entity);
      setFilled(result.text);
      setLink("");
      onChange();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(null);
    }
  }

  async function save() {
    setBusy("Saving…");
    setError(null);
    try {
      const given = Object.fromEntries(Object.entries(values).filter(([k, v]) => v.trim() && v !== (entity.info[k] ?? "")));
      await submitDetails(entity.id, given, roles);
      setDone("saved");
      onChange();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(null);
    }
  }

  async function skip() {
    if (place === "page") {
      onClose?.();
      return;
    }
    try {
      await skipDetails(entity.id);
      setDone("skipped");
      onChange();
    } catch (err) {
      setError(String(err));
    }
  }

  if (done) {
    return (
      <div className="card card-done">
        <Icon name={done === "saved" ? "check" : "close"} size={14} />
        <span>{done === "saved" ? `Saved ${first}'s profile.` : `Skipped. You can add ${first}'s details on the page any time.`}</span>
        {onOpen && (
          <button className="text-button" onClick={() => onOpen(entity.id)}>
            Open page
          </button>
        )}
      </div>
    );
  }

  return (
    <form
      className={`card profile-form ${place}`}
      onSubmit={(e) => {
        e.preventDefault();
        save();
      }}
    >
      <div className="profile-form-head">
        <Avatar entity={entity} size={36} />
        <div>
          <div className="card-title">{place === "chat" ? `Who is ${entity.name}?` : "Complete profile"}</div>
          <div className="card-sub">All optional. Paste a profile link to fill it in.</div>
        </div>
      </div>

      {request.roles.length > 0 && (
        <div className="form-group">
          <div className="form-label">How is {first} connected to you?</div>
          <div className="choice-chips">
            {request.roles.map((role) => {
              const on = roles.includes(role.tag);
              return (
                <button
                  type="button"
                  key={role.tag}
                  className={`choice-chip${on ? " on" : ""}`}
                  aria-pressed={on}
                  disabled={!!busy}
                  onClick={() => setRoles((r) => (on ? r.filter((t) => t !== role.tag) : [...r, role.tag]))}
                >
                  <Icon name={iconForTag(role.tag)} size={14} />
                  {role.label}
                </button>
              );
            })}
          </div>
        </div>
      )}

      <div className="link-fill">
        <Icon name="link" size={15} />
        <input
          value={link}
          type="url"
          placeholder="University page, Google Scholar, personal site…"
          disabled={!!busy}
          onChange={(e) => setLink(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              fill();
            }
          }}
        />
        <button type="button" className="button small" disabled={!!busy || !link.trim()} onClick={fill}>
          <Icon name="sparkle" size={13} />
          Fill in
        </button>
      </div>
      {filled && <div className="form-note">{filled}</div>}

      {fields.length > 0 && (
        <div className="form-fields">
          {fields.map((field) => (
            <label key={field.key} className={field.key === "homepage" || field.key === "thesis" ? "wide" : ""}>
              <span>{field.label}</span>
              <input
                value={value(field.key)}
                type={field.key === "email" ? "email" : "text"}
                list={optionsFor(field).length ? `${entity.id}-${field.key}` : undefined}
                disabled={!!busy}
                placeholder={field.key === "affiliation" ? "IIT Guwahati, IISc…" : field.options.slice(0, 2).join(", ")}
                onChange={(e) => {
                  const v = e.currentTarget.value;
                  setValues((old) => ({ ...old, [field.key]: v }));
                }}
              />
              {optionsFor(field).length > 0 && (
                <datalist id={`${entity.id}-${field.key}`}>
                  {optionsFor(field).map((o) => (
                    <option key={o} value={o} />
                  ))}
                </datalist>
              )}
            </label>
          ))}
        </div>
      )}

      {error && <div className="form-error">{error}</div>}
      <div className="card-actions">
        {busy ? (
          <span className="busy">
            <span className="spinner" />
            {busy}
          </span>
        ) : (
          <>
            <button className="button primary" type="submit" disabled={!changed}>
              Save
            </button>
            <button className="button ghost" type="button" onClick={skip}>
              {place === "chat" ? "Skip" : "Not now"}
            </button>
          </>
        )}
      </div>
    </form>
  );
}

export default ProfileForm;
