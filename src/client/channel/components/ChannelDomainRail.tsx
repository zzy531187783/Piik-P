import { useEffect, useState } from "react";
import { fetchServerDomains, type ServerDomain } from "../server-domains";
import { useCopy } from "../../ui/copy";
import { Glyph } from "../../ui/icons";
import "./channel-domain-rail.css";

/** A server's domains stay one click apart; a single-domain Server has no rail. */
export function ChannelDomainRail({ current }: { current: string }) {
  const { t } = useCopy();
  const [domains, setDomains] = useState<ServerDomain[] | null>(null);
  useEffect(() => {
    const controller = new AbortController();
    fetchServerDomains(controller.signal).then(value => {
      if (!controller.signal.aborted) setDomains(value);
    }).catch(() => undefined);
    return () => controller.abort();
  }, []);
  if (!domains || domains.length < 2) return null;
  return <nav className="lr-domain-rail" aria-label={t("server.domains.title")}>
    <a className="lr-domain-rail-home" href="/" aria-label="Piik" title="Piik">
      <Glyph name="couch" size={22} />
    </a>
    <ul className="lr-domain-rail-list">
      {domains.map(domain => {
        const active = domain.id === current;
        return <li key={domain.id}>
          <a className={`lr-domain-rail-item${active ? " is-current" : ""}`}
            href={`/d/${domain.id}/`} title={domain.name}
            aria-label={domain.name} aria-current={active ? "page" : undefined}>
            <span aria-hidden="true">{domain.name.slice(0, 1).toUpperCase()}</span>
          </a>
        </li>;
      })}
    </ul>
  </nav>;
}
