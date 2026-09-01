export type TeamMemberType = "agent" | "member";

export type TeamActivityOutcome = "action" | "no_action" | "failed";

export interface TeamMemberPreview {
  member_type: TeamMemberType;
  member_id: string;
  role: string;
}

export interface Team {
  id: string;
  workspace_id: string;
  name: string;
  description: string;
  instructions: string;
  avatar_url: string | null;
  leader_id: string;
  creator_id: string;
  created_at: string;
  updated_at: string;
  archived_at: string | null;
  archived_by: string | null;
  member_count?: number;
  member_preview?: TeamMemberPreview[];
}

export interface TeamMember {
  id: string;
  team_id: string;
  member_type: TeamMemberType;
  member_id: string;
  role: string;
  created_at: string;
}

export interface TeamActivityLog {
  id: string;
  team_id: string;
  issue_id: string;
  trigger_comment_id: string | null;
  leader_id: string;
  outcome: TeamActivityOutcome;
  details: unknown;
  created_at: string;
}

export interface CreateTeamRequest {
  name: string;
  description?: string;
  leader_id: string;
  avatar_url?: string;
}

export interface UpdateTeamRequest {
  name?: string;
  description?: string;
  instructions?: string;
  leader_id?: string;
  avatar_url?: string;
}

export interface AddTeamMemberRequest {
  member_type: TeamMemberType;
  member_id: string;
  role?: string;
}

export interface RemoveTeamMemberRequest {
  member_type: TeamMemberType;
  member_id: string;
}

export interface UpdateTeamMemberRoleRequest {
  member_type: TeamMemberType;
  member_id: string;
  role: string;
}

export interface CreateTeamActivityLogRequest {
  team_id: string;
  issue_id: string;
  trigger_comment_id?: string;
  outcome: TeamActivityOutcome;
  details?: unknown;
}

// TeamMemberStatus mirrors the five-way bucket the back-end derives in
// handler/team.go::deriveTeamMemberStatus. Kept as a string union here
// (rather than re-derived from snapshot data) so the team page can render
// the freshest server-side judgement without re-fetching the agent
// snapshot / runtime list. `archived` wins over every runtime/task signal.
export type TeamMemberStatusValue =
  | "working"
  | "idle"
  | "offline"
  | "unstable"
  | "archived";

export interface TeamActiveIssueBrief {
  issue_id: string;
  identifier: string;
  title: string;
  issue_status: string;
}

export interface TeamMemberStatus {
  member_type: TeamMemberType;
  member_id: string;
  // Human members are returned with status === null so the UI can render
  // them in the same list without showing a status pill (v1 has no
  // presence signal for humans).
  status: TeamMemberStatusValue | null;
  active_issues: TeamActiveIssueBrief[];
  last_active_at: string | null;
}

export interface TeamMemberStatusListResponse {
  members: TeamMemberStatus[];
}
