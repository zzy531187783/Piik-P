import { z } from "zod";

export const serverDomainSchema = z.object({ id: z.uuid(), name: z.string() });
export const serverDomainsSchema = z.array(serverDomainSchema);
export type ServerDomain = z.infer<typeof serverDomainSchema>;

/** A single-domain Server has no registry and keeps its root room page. */
export async function fetchServerDomains(signal?: AbortSignal): Promise<ServerDomain[] | null> {
  const response = await fetch("/api/server/domains", { cache: "no-store", signal });
  if (response.status === 404) return null;
  if (!response.ok) throw new Error("domains_unavailable");
  return serverDomainsSchema.parse(await response.json());
}
