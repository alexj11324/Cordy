import { seatInvitationCapacityFailure } from "../../settings/components/seat-invite-purchase";

export type DirectoryInviteErrorKind =
  | "purchase"
  | "overcommitted"
  | "unavailable"
  | "rate_limited"
  | "unknown";

export function classifyDirectoryInviteError(
  errorCode: string | undefined,
): DirectoryInviteErrorKind {
  if (errorCode === "seat_capacity_overcommitted") return "overcommitted";
  switch (seatInvitationCapacityFailure(errorCode)) {
    case "full":
      return "purchase";
    case "unavailable":
      return "unavailable";
    case "rate_limited":
      return "rate_limited";
    default:
      return "unknown";
  }
}
