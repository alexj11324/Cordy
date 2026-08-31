export type CredentialDaemonResult = {
  success: boolean;
  error?: string;
  blocked?: boolean;
};

export type CredentialDaemonInspection = {
  running: boolean;
  externallyManaged: boolean;
};

export type DesktopCredentialFlowResult = {
  credentialsChanged: boolean;
  daemonRestarted: boolean;
};

/**
 * Commit one Desktop credential transition as a fail-closed sequence.
 *
 * The caller supplies the side effects so the ordering can be tested without
 * importing Electron or depending on an ambient CLI. When a running daemon
 * would receive changed credentials, it is stopped before token minting; a
 * mint or persistence failure therefore cannot leave the previous account's
 * process active behind a successful-looking login. A restart result is
 * checked and propagated instead of being treated as fire-and-forget work.
 */
export async function commitDesktopCredentials({
  previousToken,
  previousUserId,
  previousServerUrl,
  userId,
  serverUrl,
  incomingToken,
  cachedTokenReusable,
  resolveToken,
  inspectDaemon,
  stopDaemon,
  writeCredentials,
  restartDaemon,
}: {
  previousToken: string | null;
  previousUserId: string | null;
  previousServerUrl: string | null;
  userId: string;
  serverUrl: string;
  incomingToken: string;
  cachedTokenReusable: boolean;
  resolveToken: () => Promise<string>;
  inspectDaemon: () => Promise<CredentialDaemonInspection>;
  stopDaemon: () => Promise<CredentialDaemonResult>;
  writeCredentials: (token: string) => Promise<void>;
  restartDaemon: () => Promise<CredentialDaemonResult>;
}): Promise<DesktopCredentialFlowResult> {
  const incomingTokenMatchesCached = incomingToken.startsWith("pby_")
    ? incomingToken === previousToken
    : cachedTokenReusable;
  const credentialsMayChange =
    previousUserId !== userId ||
    previousServerUrl !== serverUrl ||
    !incomingTokenMatchesCached;
  let daemonWasRunning = false;

  // Stop before minting. This is deliberately before resolveToken so a
  // network/auth failure cannot leave the previous account's daemon alive.
  if (credentialsMayChange) {
    const inspection = await inspectDaemon();
    if (inspection.running) {
      if (inspection.externallyManaged) {
        throw new Error(
          "daemon is externally managed; stop it from its owning environment before changing credentials",
        );
      }
      daemonWasRunning = true;
      const stopped = await stopDaemon();
      if (!stopped.success || stopped.blocked) {
        throw new Error(
          stopped.error ?? "failed to stop daemon before changing credentials",
        );
      }
    }
  }

  const finalToken = incomingToken.startsWith("pby_")
    ? incomingToken
    : cachedTokenReusable && previousToken
      ? previousToken
      : await resolveToken();
  const credentialsChanged =
    previousToken !== finalToken ||
    previousUserId !== userId ||
    previousServerUrl !== serverUrl;

  await writeCredentials(finalToken);

  let daemonRestarted = false;
  if (credentialsChanged) {
    let shouldRestart = daemonWasRunning;
    if (!shouldRestart) {
      const inspection = await inspectDaemon();
      shouldRestart = inspection.running;
      if (inspection.externallyManaged && shouldRestart) {
        throw new Error(
          "daemon became externally managed before credential restart; stop it from its owning environment",
        );
      }
    }
    if (shouldRestart) {
      const restarted = await restartDaemon();
      if (!restarted.success || restarted.blocked) {
        throw new Error(
          restarted.error ??
            "daemon restart did not complete after credential rotation",
        );
      }
      daemonRestarted = true;
    }
  }

  return { credentialsChanged, daemonRestarted };
}
