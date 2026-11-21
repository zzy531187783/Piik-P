import { useEffect, useState } from "react";
import { z } from "zod";
import { ChannelPreviewPage } from "./ChannelPreviewPage";
import { useCopy } from "../ui/copy";
import { Glyph } from "../ui/icons";
import "./server-domains-page.css";

const domainsSchema = z.array(z.object({ id: z.uuid(), name: z.string() }));

type State =
  | { kind: "loading" }
  | { kind: "legacy" }
  | { kind: "failed" }
  | { kind: "ready"; domains: z.infer<typeof domainsSchema> };

/** A multi-domain Server lists its domains at the root; a single-domain Server keeps the room page. */
export function ServerDomainsPage() {
  const { t } = useCopy();
  const [state, setState] = useState<State>({ kind: "loading" });
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let current = true;
    setState({ kind: "loading" });
    fetch("/api/server/domains", { cache: "no-store" }).then(async response => {
      if (!current) return;
      if (response.status === 404) {
        setState({ kind: "legacy" });
        return;
      }
      if (!response.ok) {
        setState({ kind: "failed" });
        return;
      }
      const parsed = domainsSchema.safeParse(await response.json());
      setState(parsed.success ? { kind: "ready", domains: parsed.data } : { kind: "failed" });
    }).catch(() => {
      if (current) setState({ kind: "failed" });
    });
    return () => { current = false; };
  }, [attempt]);

  if (state.kind === "legacy") return <ChannelPreviewPage />;
  return <main className="lr-server-domains">
    <header className="lr-server-domains-head">
      <span className="lr-server-domains-mark" aria-hidden="true"><Glyph name="couch" size={30} /></span>
      <h1>Piik</h1>
      <p>{t("server.domains.hint")}</p>
    </header>
    {state.kind === "loading" && <p className="lr-server-domains-status" role="status">{t("common.loading")}</p>}
    {state.kind === "failed" && <div className="lr-server-domains-failure">
      <p role="alert">{t("server.domains.failed")}</p>
      <button type="button" className="lr-btn" onClick={() => setAttempt(value => value + 1)}>
        {t("common.retry")}
      </button>
    </div>}
    {state.kind === "ready" && (state.domains.length === 0
      ? <p className="lr-server-domains-status">{t("server.domains.empty")}</p>
      : <ul className="lr-server-domains-list" aria-label={t("server.domains.title")}>
          {state.domains.map(domain => <li key={domain.id}>
            <a className="lr-server-domain" href={`/d/${domain.id}/`}>
              <span className="lr-server-domain-avatar" aria-hidden="true">{domain.name.slice(0, 1).toUpperCase()}</span>
              <span className="lr-server-domain-copy">
                <strong>{domain.name}</strong>
                <small>{domain.id.slice(0, 8)}</small>
              </span>
              <span className="lr-server-domain-open">{t("server.domains.open")}
                <Glyph name="arrowRight" size={16} />
              </span>
            </a>
          </li>)}
        </ul>)}
  </main>;
}
