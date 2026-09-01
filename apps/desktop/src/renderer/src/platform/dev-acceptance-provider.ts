export type DevAcceptanceInstallation = {
  agent_id: string | null;
  status: string;
  round_trip_status?: string;
};

/** Select the workspace Hub installation used by the Settings flow. */
export function findActiveHubInstallation<T extends DevAcceptanceInstallation>(
  installations: readonly T[],
): T | undefined {
  return installations.find(
    (candidate) => candidate.agent_id === null && candidate.status === "active",
  );
}
