/**
 * Route gate for the floating chat overlay, shared by the overlay itself and by
 * the `toggleChat` keyboard shortcut so the two can never disagree.
 *
 * On the Chat tab the full-page surface already owns the same `activeSessionId`,
 * so a floating copy would be pure duplication — `FloatingChat` renders nothing
 * there. The shortcut has to honour the same rule: flipping `isOpen` on a route
 * where the overlay cannot mount reads as a dead keypress, and then surprises
 * the user with a window that pops open (or vanishes) on the next navigation.
 * An open task Agent panel also owns the corner where the launcher would sit.
 * Agent detail owns a direct message action in its identity card, so it does
 * not show a second global launcher.
 */
export function isFloatingChatRouteSuppressed(
  pathname: string,
  chatPath: string,
  agentThreadPath?: string,
  agentsPath?: string,
  agentDetailDmAvailable = false,
): boolean {
  const agentDetailSegment = agentsPath
    ? pathname.slice(`${agentsPath}/`.length)
    : "";
  const isAgentDetail =
    !!agentsPath &&
    pathname.startsWith(`${agentsPath}/`) &&
    agentDetailSegment !== "new" &&
    !agentDetailSegment.startsWith("new/") &&
    !agentDetailSegment.includes("/") &&
    agentDetailDmAvailable;

  return (
    pathname === agentThreadPath ||
    pathname === chatPath ||
    pathname.startsWith(`${chatPath}/`) ||
    isAgentDetail
  );
}
