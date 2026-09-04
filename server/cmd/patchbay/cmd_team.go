package main

import (
	"context"
	"fmt"
	"os"
	"strconv"
	"text/tabwriter"

	"github.com/spf13/cobra"

	"github.com/patchbay-ai/patchbay/server/internal/cli"
)

var teamCmd = &cobra.Command{
	Use:   "team",
	Short: "Work with teams",
}

// ── List ────────────────────────────────────────────────────────────────────

var teamListCmd = &cobra.Command{
	Use:   "list",
	Short: "List teams in the workspace",
	Args:  cobra.NoArgs,
	RunE:  runTeamList,
}

func runTeamList(cmd *cobra.Command, _ []string) error {
	client, err := newAPIClient(cmd)
	if err != nil {
		return err
	}
	ctx, cancel := cli.APIContext(context.Background())
	defer cancel()

	var teams []map[string]any
	if err := client.GetJSON(ctx, "/api/teams", &teams); err != nil {
		return fmt.Errorf("list teams: %w", err)
	}

	output, _ := cmd.Flags().GetString("output")
	if output == "json" {
		return cli.PrintJSON(os.Stdout, teams)
	}

	if len(teams) == 0 {
		fmt.Fprintln(os.Stderr, "No teams found.")
		return nil
	}

	w := tabwriter.NewWriter(os.Stdout, 0, 4, 2, ' ', 0)
	fmt.Fprintln(w, "ID\tNAME\tLEADER ID\tMEMBERS")
	for _, s := range teams {
		fmt.Fprintf(w, "%s\t%s\t%s\t%s\n",
			strVal(s, "id"), strVal(s, "name"), strVal(s, "leader_id"),
			memberCountDisplay(s))
	}
	return w.Flush()
}

func memberCountDisplay(m map[string]any) string {
	v, ok := m["member_count"]
	if !ok || v == nil {
		return "-"
	}
	n, ok := v.(float64)
	if !ok || n <= 0 {
		return "-"
	}
	return strconv.Itoa(int(n))
}

// ── Get ─────────────────────────────────────────────────────────────────────

var teamGetCmd = &cobra.Command{
	Use:   "get <team-id>",
	Short: "Get team details",
	Args:  exactArgs(1),
	RunE:  runTeamGet,
}

func runTeamGet(cmd *cobra.Command, args []string) error {
	client, err := newAPIClient(cmd)
	if err != nil {
		return err
	}
	ctx, cancel := cli.APIContext(context.Background())
	defer cancel()

	var team map[string]any
	if err := client.GetJSON(ctx, "/api/teams/"+args[0], &team); err != nil {
		return fmt.Errorf("get team: %w", err)
	}

	output, _ := cmd.Flags().GetString("output")
	if output == "json" {
		return cli.PrintJSON(os.Stdout, team)
	}

	fmt.Printf("ID:           %s\n", strVal(team, "id"))
	fmt.Printf("Name:         %s\n", strVal(team, "name"))
	fmt.Printf("Description:  %s\n", strVal(team, "description"))
	fmt.Printf("Leader ID:    %s\n", strVal(team, "leader_id"))
	fmt.Printf("Created:      %s\n", strVal(team, "created_at"))
	if inst := strVal(team, "instructions"); inst != "" {
		fmt.Printf("Instructions: %s\n", inst)
	}
	return nil
}

// ── Create ──────────────────────────────────────────────────────────────────

var teamCreateCmd = &cobra.Command{
	Use:   "create",
	Short: "Create a new team",
	Args:  cobra.NoArgs,
	RunE:  runTeamCreate,
}

func runTeamCreate(cmd *cobra.Command, _ []string) error {
	name, _ := cmd.Flags().GetString("name")
	if name == "" {
		return fmt.Errorf("--name is required")
	}
	leader, _ := cmd.Flags().GetString("leader")
	if leader == "" {
		return fmt.Errorf("--leader is required (agent name or ID)")
	}

	client, err := newAPIClient(cmd)
	if err != nil {
		return err
	}
	ctx, cancel := cli.APIContext(context.Background())
	defer cancel()

	leaderID, err := resolveAgent(ctx, client, leader)
	if err != nil {
		return fmt.Errorf("resolve leader: %w", err)
	}

	body := map[string]any{
		"name":      name,
		"leader_id": leaderID,
	}
	if v, _ := cmd.Flags().GetString("description"); v != "" {
		body["description"] = v
	}

	var result map[string]any
	if err := client.PostJSON(ctx, "/api/teams", body, &result); err != nil {
		return fmt.Errorf("create team: %w", err)
	}

	output, _ := cmd.Flags().GetString("output")
	if output == "json" {
		return cli.PrintJSON(os.Stdout, result)
	}
	fmt.Printf("Team created: %s (%s)\n", strVal(result, "name"), strVal(result, "id"))
	return nil
}

// ── Update ──────────────────────────────────────────────────────────────────

var teamUpdateCmd = &cobra.Command{
	Use:   "update <team-id>",
	Short: "Update a team",
	Args:  exactArgs(1),
	RunE:  runTeamUpdate,
}

func runTeamUpdate(cmd *cobra.Command, args []string) error {
	client, err := newAPIClient(cmd)
	if err != nil {
		return err
	}
	ctx, cancel := cli.APIContext(context.Background())
	defer cancel()

	body := map[string]any{}
	if cmd.Flags().Changed("name") {
		v, _ := cmd.Flags().GetString("name")
		body["name"] = v
	}
	if cmd.Flags().Changed("description") {
		v, _ := cmd.Flags().GetString("description")
		body["description"] = v
	}
	if cmd.Flags().Changed("instructions") {
		v, _ := cmd.Flags().GetString("instructions")
		body["instructions"] = v
	}
	if cmd.Flags().Changed("leader") {
		v, _ := cmd.Flags().GetString("leader")
		leaderID, err := resolveAgent(ctx, client, v)
		if err != nil {
			return fmt.Errorf("resolve leader: %w", err)
		}
		body["leader_id"] = leaderID
	}
	if cmd.Flags().Changed("avatar-url") {
		v, _ := cmd.Flags().GetString("avatar-url")
		body["avatar_url"] = v
	}

	if len(body) == 0 {
		return fmt.Errorf("no fields to update; use flags like --name, --description, --instructions, --leader")
	}

	var result map[string]any
	if err := client.PutJSON(ctx, "/api/teams/"+args[0], body, &result); err != nil {
		return fmt.Errorf("update team: %w", err)
	}

	output, _ := cmd.Flags().GetString("output")
	if output == "json" {
		return cli.PrintJSON(os.Stdout, result)
	}
	fmt.Printf("Team updated: %s (%s)\n", strVal(result, "name"), strVal(result, "id"))
	return nil
}

// ── Delete ──────────────────────────────────────────────────────────────────

var teamDeleteCmd = &cobra.Command{
	Use:   "delete <team-id>",
	Short: "Delete (archive) a team",
	Args:  exactArgs(1),
	RunE:  runTeamDelete,
}

func runTeamDelete(cmd *cobra.Command, args []string) error {
	client, err := newAPIClient(cmd)
	if err != nil {
		return err
	}
	ctx, cancel := cli.APIContext(context.Background())
	defer cancel()

	if err := client.DeleteJSON(ctx, "/api/teams/"+args[0]); err != nil {
		return fmt.Errorf("delete team: %w", err)
	}

	output, _ := cmd.Flags().GetString("output")
	if output == "json" {
		return cli.PrintJSON(os.Stdout, map[string]any{"id": args[0], "deleted": true})
	}
	fmt.Fprintf(os.Stderr, "Team %s deleted.\n", args[0])
	return nil
}

// ── Members ─────────────────────────────────────────────────────────────────

var teamMemberCmd = &cobra.Command{
	Use:   "member",
	Short: "Work with team members",
}

var teamMemberListCmd = &cobra.Command{
	Use:   "list <team-id>",
	Short: "List members of a team",
	Args:  exactArgs(1),
	RunE:  runTeamMemberList,
}

func runTeamMemberList(cmd *cobra.Command, args []string) error {
	client, err := newAPIClient(cmd)
	if err != nil {
		return err
	}
	ctx, cancel := cli.APIContext(context.Background())
	defer cancel()

	var members []map[string]any
	if err := client.GetJSON(ctx, "/api/teams/"+args[0]+"/members", &members); err != nil {
		return fmt.Errorf("list members: %w", err)
	}

	output, _ := cmd.Flags().GetString("output")
	if output == "json" {
		return cli.PrintJSON(os.Stdout, members)
	}

	if len(members) == 0 {
		fmt.Fprintln(os.Stderr, "No members found.")
		return nil
	}

	w := tabwriter.NewWriter(os.Stdout, 0, 4, 2, ' ', 0)
	fmt.Fprintln(w, "MEMBER ID\tTYPE\tROLE")
	for _, m := range members {
		fmt.Fprintf(w, "%s\t%s\t%s\n",
			strVal(m, "member_id"), strVal(m, "member_type"), strVal(m, "role"))
	}
	return w.Flush()
}

// ── Member Add ──────────────────────────────────────────────────────────────

var teamMemberAddCmd = &cobra.Command{
	Use:   "add <team-id>",
	Short: "Add a member to a team",
	Args:  exactArgs(1),
	RunE:  runTeamMemberAdd,
}

func runTeamMemberAdd(cmd *cobra.Command, args []string) error {
	memberID, _ := cmd.Flags().GetString("member-id")
	memberType, _ := cmd.Flags().GetString("type")
	role, _ := cmd.Flags().GetString("role")

	if memberID == "" {
		return fmt.Errorf("--member-id is required")
	}
	if memberType != "agent" && memberType != "member" {
		return fmt.Errorf("--type must be 'agent' or 'member'")
	}

	client, err := newAPIClient(cmd)
	if err != nil {
		return err
	}
	ctx, cancel := cli.APIContext(context.Background())
	defer cancel()

	body := map[string]any{
		"member_type": memberType,
		"member_id":   memberID,
		"role":        role,
	}

	var result map[string]any
	if err := client.PostJSON(ctx, "/api/teams/"+args[0]+"/members", body, &result); err != nil {
		return fmt.Errorf("add member: %w", err)
	}

	output, _ := cmd.Flags().GetString("output")
	if output == "json" {
		return cli.PrintJSON(os.Stdout, result)
	}
	fmt.Printf("Member %s added to team.\n", memberID)
	return nil
}

// ── Member Set Role ─────────────────────────────────────────────────────────

var teamMemberSetRoleCmd = &cobra.Command{
	Use:   "set-role <team-id>",
	Short: "Change a team member's role",
	Args:  exactArgs(1),
	RunE:  runTeamMemberSetRole,
}

func runTeamMemberSetRole(cmd *cobra.Command, args []string) error {
	memberID, _ := cmd.Flags().GetString("member-id")
	memberType, _ := cmd.Flags().GetString("member-type")
	role, _ := cmd.Flags().GetString("role")

	if memberID == "" {
		return fmt.Errorf("--member-id is required")
	}
	if memberType != "agent" && memberType != "member" {
		return fmt.Errorf("--member-type must be 'agent' or 'member'")
	}
	if role == "" {
		return fmt.Errorf("--role is required")
	}

	client, err := newAPIClient(cmd)
	if err != nil {
		return err
	}
	ctx, cancel := cli.APIContext(context.Background())
	defer cancel()

	body := map[string]any{
		"member_type": memberType,
		"member_id":   memberID,
		"role":        role,
	}

	var result map[string]any
	if err := client.PatchJSON(ctx, "/api/teams/"+args[0]+"/members/role", body, &result); err != nil {
		return fmt.Errorf("set member role: %w", err)
	}

	output, _ := cmd.Flags().GetString("output")
	if output == "json" {
		return cli.PrintJSON(os.Stdout, result)
	}
	fmt.Fprintf(os.Stderr, "Member %s role updated to %s.\n", memberID, role)
	return nil
}

// ── Member Remove ───────────────────────────────────────────────────────────

var teamMemberRemoveCmd = &cobra.Command{
	Use:   "remove <team-id>",
	Short: "Remove a member from a team",
	Args:  exactArgs(1),
	RunE:  runTeamMemberRemove,
}

func runTeamMemberRemove(cmd *cobra.Command, args []string) error {
	memberID, _ := cmd.Flags().GetString("member-id")
	memberType, _ := cmd.Flags().GetString("type")

	if memberID == "" {
		return fmt.Errorf("--member-id is required")
	}
	if memberType != "agent" && memberType != "member" {
		return fmt.Errorf("--type must be 'agent' or 'member'")
	}

	client, err := newAPIClient(cmd)
	if err != nil {
		return err
	}
	ctx, cancel := cli.APIContext(context.Background())
	defer cancel()

	body := map[string]any{
		"member_type": memberType,
		"member_id":   memberID,
	}

	if err := client.DeleteJSONWithBody(ctx, "/api/teams/"+args[0]+"/members", body); err != nil {
		return fmt.Errorf("remove member: %w", err)
	}

	output, _ := cmd.Flags().GetString("output")
	if output == "json" {
		return cli.PrintJSON(os.Stdout, map[string]any{"team_id": args[0], "member_id": memberID, "removed": true})
	}
	fmt.Fprintf(os.Stderr, "Member %s removed from team.\n", memberID)
	return nil
}

// ── Activity ────────────────────────────────────────────────────────────────

var teamActivityCmd = &cobra.Command{
	Use:   "activity <issue-id> <outcome>",
	Short: "Record a team leader evaluation on an issue",
	Long: `Record the team leader's evaluation decision for an issue.

Outcome must be one of:
  action     — leader delegated or took action
  no_action  — leader evaluated and decided no action needed
  failed     — leader encountered an error

This command is intended to be called by team leader agents after each
trigger to record their decision in the issue timeline.

Pass the issue the current turn is running on. That issue does not need to
be assigned to your team — authorization comes from the leader task itself,
so @team mentions on an individually owned issue and leader tasks bound to a
child issue both record fine. A leader woken by a stage barrier runs on the
parent issue, so record against the parent.`,
	Args: exactArgs(2),
	RunE: runTeamActivity,
}

func runTeamActivity(cmd *cobra.Command, args []string) error {
	issueID := args[0]
	outcome := args[1]

	if outcome != "action" && outcome != "no_action" && outcome != "failed" {
		return fmt.Errorf("invalid outcome %q; valid values: action, no_action, failed", outcome)
	}

	reason, _ := cmd.Flags().GetString("reason")

	client, err := newAPIClient(cmd)
	if err != nil {
		return err
	}

	ctx, cancel := cli.APIContext(context.Background())
	defer cancel()

	issueRef, err := resolveIssueRef(ctx, client, issueID)
	if err != nil {
		return fmt.Errorf("resolve issue: %w", err)
	}

	body := map[string]any{
		"outcome": outcome,
		"reason":  reason,
	}
	var result map[string]any
	if err := client.PostJSON(ctx, "/api/issues/"+issueRef.ID+"/team-evaluated", body, &result); err != nil {
		return fmt.Errorf("record evaluation: %w", err)
	}

	fmt.Fprintf(os.Stderr, "Team evaluation recorded: %s (issue %s)\n", outcome, issueRef.Display)

	output, _ := cmd.Flags().GetString("output")
	if output == "json" {
		return cli.PrintJSON(os.Stdout, result)
	}
	return nil
}

// ── Init ────────────────────────────────────────────────────────────────────

func init() {
	// list
	teamListCmd.Flags().String("output", "table", "Output format: table or json")

	// get
	teamGetCmd.Flags().String("output", "table", "Output format: table or json")

	// create
	teamCreateCmd.Flags().String("name", "", "Team name (required)")
	teamCreateCmd.Flags().String("description", "", "Team description")
	teamCreateCmd.Flags().String("leader", "", "Leader agent (name or ID) — required")
	teamCreateCmd.Flags().String("output", "json", "Output format: table or json")

	// update
	teamUpdateCmd.Flags().String("name", "", "New name")
	teamUpdateCmd.Flags().String("description", "", "New description")
	teamUpdateCmd.Flags().String("instructions", "", "New instructions")
	teamUpdateCmd.Flags().String("leader", "", "New leader agent (name or ID)")
	teamUpdateCmd.Flags().String("avatar-url", "", "New avatar URL")
	teamUpdateCmd.Flags().String("output", "json", "Output format: table or json")

	// delete
	teamDeleteCmd.Flags().String("output", "table", "Output format: table or json")

	// member list
	teamMemberListCmd.Flags().String("output", "table", "Output format: table or json")

	// member add
	teamMemberAddCmd.Flags().String("member-id", "", "Member or agent ID (required)")
	teamMemberAddCmd.Flags().String("type", "agent", "Member type: agent or member")
	teamMemberAddCmd.Flags().String("role", "member", "Role in the team")
	teamMemberAddCmd.Flags().String("output", "json", "Output format: table or json")

	// member remove
	teamMemberRemoveCmd.Flags().String("member-id", "", "Member or agent ID (required)")
	teamMemberRemoveCmd.Flags().String("type", "agent", "Member type: agent or member")
	teamMemberRemoveCmd.Flags().String("output", "table", "Output format: table or json")

	// member set-role
	teamMemberSetRoleCmd.Flags().String("member-id", "", "Member or agent ID (required)")
	teamMemberSetRoleCmd.Flags().String("member-type", "agent", "Member type: agent or member")
	teamMemberSetRoleCmd.Flags().String("role", "", "New role in the team (required)")
	teamMemberSetRoleCmd.Flags().String("output", "json", "Output format: table or json")

	// activity
	teamActivityCmd.Flags().String("reason", "", "Short explanation of the decision")
	teamActivityCmd.Flags().String("output", "table", "Output format: table or json")

	teamMemberCmd.AddCommand(teamMemberListCmd)
	teamMemberCmd.AddCommand(teamMemberAddCmd)
	teamMemberCmd.AddCommand(teamMemberRemoveCmd)
	teamMemberCmd.AddCommand(teamMemberSetRoleCmd)

	teamCmd.AddCommand(teamListCmd)
	teamCmd.AddCommand(teamGetCmd)
	teamCmd.AddCommand(teamCreateCmd)
	teamCmd.AddCommand(teamUpdateCmd)
	teamCmd.AddCommand(teamDeleteCmd)
	teamCmd.AddCommand(teamMemberCmd)
	teamCmd.AddCommand(teamActivityCmd)
}
