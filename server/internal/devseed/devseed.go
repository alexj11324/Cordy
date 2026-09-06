// Package devseed installs deterministic, development-only product fixtures.
//
// The fixtures live in their own workspace, are safe to insert repeatedly, and
// deliberately preserve rows that a developer edited after the first seed.
// They are never part of migrations or server startup: callers must opt in and
// pass the local-target safety check before opening a database connection.
package devseed

import (
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"net/url"
	"strings"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/patchbay-ai/patchbay/server/internal/issuestatus"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

const (
	WorkspaceName         = "开发调试样例"
	WorkspaceSlug         = "dev-fixtures"
	WorkspaceDescription  = "Patchbay 内置开发 fixture。内容可从代码重建，不代表真实工作记录。"
	FixtureSet            = "ui-guidance-v1"
	DefaultDeveloperEmail = "dev@localhost"
)

var fixtureNamespace = uuid.MustParse("59180596-1e6c-4ad9-a886-57a03e23c920")

type Result struct {
	WorkspaceID string
	Workspace   string
	Issues      int
	GraphNodes  int
	GraphEdges  int
}

type issueFixture struct {
	key         string
	number      int32
	title       string
	description string
	status      string
	priority    string
	position    float64
}

type graphNodeFixture struct {
	tempID   string
	issueKey string
	outputs  []string
	wave     int32
}

type graphEdgeFixture struct {
	fromIssueKey string
	toIssueKey   string
	reason       string
	output       string
}

var graphNodeFixtures = []graphNodeFixture{
	{tempID: "task-9", issueKey: "layout-cleanup", outputs: []string{"统一的页面结构"}, wave: 0},
	{tempID: "task-4", issueKey: "issue-detail-hierarchy", outputs: []string{"任务详情信息结构"}, wave: 1},
	{tempID: "task-5", issueKey: "system-colors", outputs: []string{"黑白配色规范"}, wave: 1},
	{tempID: "task-2", issueKey: "team-create", outputs: []string{"团队创建流程"}, wave: 2},
	{tempID: "task-3", issueKey: "settings-inventory", outputs: []string{"设置功能去向清单"}, wave: 2},
}

var graphEdgeFixtures = []graphEdgeFixture{
	{
		fromIssueKey: "issue-detail-hierarchy",
		toIssueKey:   "settings-inventory",
		reason:       "确定哪些字段留在详情中，才能收敛全局设置入口。",
		output:       "任务详情信息结构",
	},
	{
		fromIssueKey: "issue-detail-hierarchy",
		toIssueKey:   "team-create",
		reason:       "团队创建需要与详情中的补充信息边界保持一致。",
		output:       "任务详情信息结构",
	},
	{
		fromIssueKey: "layout-cleanup",
		toIssueKey:   "issue-detail-hierarchy",
		reason:       "先确定单一内容区域，再整理详情信息层级。",
		output:       "统一的页面结构",
	},
	{
		fromIssueKey: "layout-cleanup",
		toIssueKey:   "system-colors",
		reason:       "配色需要应用到确定后的页面结构。",
		output:       "统一的页面结构",
	},
}

func issueFixtures() []issueFixture {
	return []issueFixture{
		fixtureIssue("onboarding-flow", 1, "梳理新用户第一次使用的完整流程", "走一遍从进入工作区到创建首个任务的过程，记录需要解释、重复输入或容易迷路的步骤。", "给出三个优先改进点，并说明改动后的用户路径。", "todo", "high", 1),
		fixtureIssue("team-create", 2, "把团队创建缩短到两个必要选择", "创建时只填写团队名称与负责 Agent；描述、成员和协作说明在团队详情中补充。", "单个 Agent 自动选择，无可用 Agent 时保留团队名称草稿。", "todo", "medium", 2),
		fixtureIssue("settings-inventory", 3, "为设置页面整理功能去向清单", "盘点标签、自定义字段、任务状态与快捷操作，明确哪些应放到任务详情中。", "保留数据与能力，避免在多个位置重复提供同一个管理入口。", "todo", "low", 3),
		fixtureIssue("issue-detail-hierarchy", 4, "重新整理任务详情的信息层级", "先展示目标、状态和负责人，再展示成果。执行日志默认收起，补充资料按需打开。", "首次查看详情时能清楚知道要完成什么，以及下一步需要做什么。", "in_progress", "high", 4),
		fixtureIssue("system-colors", 5, "统一主界面的黑白系统配色", "移除大面积强调色和多余装饰，使用黑白与系统灰阶区分层级。", "深浅模式下文字可读，主要操作清楚，侧栏与内容色调一致。", "in_progress", "medium", 5),
		fixtureIssue("tabs", 6, "检查工作标签的切换与关闭行为", "检查侧栏切换、打开多个工作标签以及关闭当前标签的行为。", "标题、侧栏选中项与内容一致，关闭最后一个工作标签不出现空白。", "in_progress", "urgent", 6),
		fixtureIssue("create-dialog", 7, "验收简化后的新建任务弹窗", "已整理一版更短的创建表单，重点检查必填项、键盘顺序与中文输入。", "只要求必要信息，Enter 不打断中文输入，关闭弹窗后焦点回到触发入口。", "in_review", "high", 7),
		fixtureIssue("work-product", 8, "确认成果预览与修改反馈的入口", "成果预览需要明确显示所属任务，并提供通过验收和提出修改两种操作。", "反馈能回到所属任务，用户不需要进入设置或寻找日志。", "in_review", "medium", 8),
		fixtureIssue("layout-cleanup", 9, "清理重复的页面标题和分隔线", "已合并重复标题并减少无必要的外框，保留能够帮助理解内容分组的结构。", "主内容区有一个清晰标题，不再出现重复页头。", "done", "low", 9),
		fixtureIssue("action-copy", 10, "整理统一的按钮与表单文案", "将模糊的确认、提交改为创建任务、保存修改等具体动作。", "同一操作在列表、详情和弹窗中使用相同说法。", "done", "medium", 10),
		fixtureIssue("integration-permissions", 11, "核对第三方连接的权限说明", "逐项核对第三方连接申请的权限、数据用途和断开连接后的处理方式。", "确认每个连接的实际用途后，再补充对应的授权说明。", "blocked", "medium", 11),
		fixtureIssue("weekly-review", 12, "探索更轻量的每周工作回顾", "考虑用完成事项、需要关注的事项和下一步三部分呈现工作进展。", "先提供一份低保真结构草图，再决定是否进入开发。", "backlog", "none", 12),
		{
			key:         "graph-parent",
			number:      13,
			title:       "完成一轮界面简化（示例目标）",
			description: "开发版依赖图示例。先完成页面结构清理，再并行调整任务详情与配色，最后基于详情方案简化团队创建和设置入口。这里只展示任务关系，不启动真实执行。",
			status:      "backlog",
			priority:    "medium",
			position:    0,
		},
	}
}

func fixtureIssue(key string, number int32, title, goal, acceptance, status, priority string, position float64) issueFixture {
	return issueFixture{
		key:         key,
		number:      number,
		title:       title,
		status:      status,
		priority:    priority,
		position:    position,
		description: fmt.Sprintf("这是用于开发版界面检查的示例任务，不代表真实执行记录。\n\n## 目标\n\n%s\n\n## 验收标准\n\n- %s", goal, acceptance),
	}
}

// ValidateTarget prevents this development-only command from being pointed at
// a hosted database by an inherited DATABASE_URL. The explicit opt-in is set by
// the repository's make target; loopback and database-name checks are a second
// independent boundary.
func ValidateTarget(rawDatabaseURL string, enabled bool) error {
	if !enabled {
		return errors.New("development seed is disabled; run it through `make seed-dev`")
	}
	u, err := url.Parse(rawDatabaseURL)
	if err != nil {
		return fmt.Errorf("parse DATABASE_URL: %w", err)
	}
	if u.Scheme != "postgres" && u.Scheme != "postgresql" {
		return fmt.Errorf("development seed requires a postgres DATABASE_URL, got scheme %q", u.Scheme)
	}
	host := strings.ToLower(u.Hostname())
	if host != "localhost" && host != "127.0.0.1" && host != "::1" {
		return fmt.Errorf("development seed refuses non-loopback database host %q", host)
	}
	database := strings.TrimPrefix(u.Path, "/")
	if database != "patchbay" && !strings.HasPrefix(database, "patchbay_") {
		return fmt.Errorf("development seed refuses database %q; expected patchbay or patchbay_*", database)
	}
	return nil
}

// Seed inserts any missing fixture rows and preserves rows already present.
func Seed(ctx context.Context, pool *pgxpool.Pool, developerEmail string) (Result, error) {
	if strings.TrimSpace(developerEmail) == "" {
		developerEmail = DefaultDeveloperEmail
	}

	tx, err := pool.BeginTx(ctx, pgx.TxOptions{})
	if err != nil {
		return Result{}, fmt.Errorf("begin development seed: %w", err)
	}
	defer tx.Rollback(ctx) //nolint:errcheck -- commit below owns the successful path

	var developerID pgtype.UUID
	if err := tx.QueryRow(ctx, `SELECT id FROM "user" WHERE email = $1`, developerEmail).Scan(&developerID); err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return Result{}, fmt.Errorf("developer user %q does not exist; run `make up` once before `make seed-dev`", developerEmail)
		}
		return Result{}, fmt.Errorf("find developer user: %w", err)
	}

	workspaceID := pgUUID(fixtureID("workspace"))
	if err := ensureWorkspace(ctx, tx, workspaceID, developerID); err != nil {
		return Result{}, err
	}
	if err := issuestatus.Ensure(ctx, db.New(tx), workspaceID); err != nil {
		return Result{}, fmt.Errorf("seed built-in issue statuses: %w", err)
	}

	issues := issueFixtures()
	for _, issue := range issues {
		metadata, err := json.Marshal(map[string]any{
			"demo_seed":  FixtureSet,
			"demo_index": issue.number,
		})
		if err != nil {
			return Result{}, fmt.Errorf("encode issue fixture %q: %w", issue.key, err)
		}
		if _, err := tx.Exec(ctx, `
			INSERT INTO issue (
				id, workspace_id, title, description, status, priority,
				creator_type, creator_id, owner_type, owner_id,
				position, number, metadata, last_activity_at
			) VALUES ($1, $2, $3, $4, $5, $6, 'member', $7, 'member', $7, $8, $9, $10, now())
			ON CONFLICT (id) DO NOTHING`,
			pgUUID(fixtureID("issue/"+issue.key)), workspaceID, issue.title,
			issue.description, issue.status, issue.priority, developerID,
			issue.position, issue.number, metadata,
		); err != nil {
			return Result{}, fmt.Errorf("seed issue %q: %w", issue.key, err)
		}
	}

	if _, err := tx.Exec(ctx, `UPDATE workspace SET issue_counter = GREATEST(issue_counter, $2) WHERE id = $1`, workspaceID, len(issues)); err != nil {
		return Result{}, fmt.Errorf("advance fixture issue counter: %w", err)
	}
	if err := ensureGraph(ctx, tx, workspaceID, developerID, issues); err != nil {
		return Result{}, err
	}
	if err := verifyFixtureRows(ctx, tx, workspaceID, issues); err != nil {
		return Result{}, err
	}

	if err := tx.Commit(ctx); err != nil {
		return Result{}, fmt.Errorf("commit development seed: %w", err)
	}
	return Result{
		WorkspaceID: fixtureID("workspace"),
		Workspace:   WorkspaceSlug,
		Issues:      len(issues),
		GraphNodes:  len(graphNodeFixtures),
		GraphEdges:  len(graphEdgeFixtures),
	}, nil
}

func ensureWorkspace(ctx context.Context, tx pgx.Tx, workspaceID, developerID pgtype.UUID) error {
	var existingID pgtype.UUID
	err := tx.QueryRow(ctx, `SELECT id FROM workspace WHERE slug = $1`, WorkspaceSlug).Scan(&existingID)
	switch {
	case errors.Is(err, pgx.ErrNoRows):
		if _, err := tx.Exec(ctx, `
			INSERT INTO workspace (id, name, slug, description, settings, issue_prefix)
			VALUES ($1, $2, $3, $4, jsonb_build_object('development_fixture', $5::text), 'DEV')`,
			workspaceID, WorkspaceName, WorkspaceSlug, WorkspaceDescription, FixtureSet,
		); err != nil {
			return fmt.Errorf("create fixture workspace: %w", err)
		}
	case err != nil:
		return fmt.Errorf("find fixture workspace: %w", err)
	case existingID != workspaceID:
		return fmt.Errorf("workspace slug %q already belongs to another workspace; refusing to overwrite it", WorkspaceSlug)
	}

	if _, err := tx.Exec(ctx, `
		INSERT INTO member (id, workspace_id, user_id, role)
		VALUES ($1, $2, $3, 'owner')
		ON CONFLICT (workspace_id, user_id) DO UPDATE SET role = 'owner'`,
		pgUUID(fixtureID("member")), workspaceID, developerID,
	); err != nil {
		return fmt.Errorf("add developer to fixture workspace: %w", err)
	}
	return nil
}

func ensureGraph(ctx context.Context, tx pgx.Tx, workspaceID, developerID pgtype.UUID, issues []issueFixture) error {
	issuesByKey := make(map[string]issueFixture, len(issues))
	for _, issue := range issues {
		issuesByKey[issue.key] = issue
	}

	planID := pgUUID(fixtureID("graph/plan"))
	parentIssueID := pgUUID(fixtureID("issue/graph-parent"))
	requestHash := fmt.Sprintf("%x", sha256.Sum256([]byte(FixtureSet+"/dependency-graph")))
	if _, err := tx.Exec(ctx, `
		INSERT INTO dependency_graph_plan (
			id, workspace_id, parent_issue_id, idempotency_key, request_hash,
			goal, status, created_by_type, created_by_id
		) VALUES ($1, $2, $3, $4, $5, $6, 'active', 'member', $7)
		ON CONFLICT DO NOTHING`,
		planID, workspaceID, parentIssueID, FixtureSet+"/dependency-graph", requestHash,
		"示例：页面结构清理 → 详情与配色并行 → 团队和设置流程简化", developerID,
	); err != nil {
		return fmt.Errorf("seed dependency graph plan: %w", err)
	}

	for _, node := range graphNodeFixtures {
		issue, ok := issuesByKey[node.issueKey]
		if !ok {
			return fmt.Errorf("graph node %q references unknown issue fixture %q", node.tempID, node.issueKey)
		}
		outputs, err := json.Marshal(node.outputs)
		if err != nil {
			return fmt.Errorf("encode outputs for graph node %q: %w", node.tempID, err)
		}
		if _, err := tx.Exec(ctx, `
			INSERT INTO dependency_graph_node (
				id, plan_id, workspace_id, temp_id, issue_id, title, description,
				acceptance_criteria, context, outputs, candidate_executors,
				owner_type, owner_id, wave
			) VALUES ($1, $2, $3, $4, $5, $6, $7, '[]', '{}', $8, '[]', 'member', $9, $10)
			ON CONFLICT DO NOTHING`,
			pgUUID(fixtureID("graph/node/"+node.tempID)), planID, workspaceID,
			node.tempID, pgUUID(fixtureID("issue/"+node.issueKey)), issue.title,
			issue.description, outputs, developerID, node.wave,
		); err != nil {
			return fmt.Errorf("seed dependency graph node %q: %w", node.tempID, err)
		}
	}

	for _, edge := range graphEdgeFixtures {
		edgeKey := edge.fromIssueKey + "/" + edge.toIssueKey
		if _, err := tx.Exec(ctx, `
			INSERT INTO dependency_graph_edge (
				id, plan_id, workspace_id, from_issue_id, to_issue_id,
				type, reason, consumed_output
			) VALUES ($1, $2, $3, $4, $5, 'hard', $6, $7)
			ON CONFLICT DO NOTHING`,
			pgUUID(fixtureID("graph/edge/"+edgeKey)), planID, workspaceID,
			pgUUID(fixtureID("issue/"+edge.fromIssueKey)),
			pgUUID(fixtureID("issue/"+edge.toIssueKey)), edge.reason, edge.output,
		); err != nil {
			return fmt.Errorf("seed dependency graph edge %q: %w", edgeKey, err)
		}
	}
	return nil
}

func verifyFixtureRows(ctx context.Context, tx pgx.Tx, workspaceID pgtype.UUID, issues []issueFixture) error {
	for _, issue := range issues {
		var actualWorkspaceID pgtype.UUID
		if err := tx.QueryRow(ctx, `SELECT workspace_id FROM issue WHERE id = $1`, pgUUID(fixtureID("issue/"+issue.key))).Scan(&actualWorkspaceID); err != nil {
			return fmt.Errorf("verify issue fixture %q: %w", issue.key, err)
		}
		if actualWorkspaceID != workspaceID {
			return fmt.Errorf("issue fixture %q belongs to another workspace; refusing to continue", issue.key)
		}
	}

	planID := pgUUID(fixtureID("graph/plan"))
	var planWorkspaceID pgtype.UUID
	if err := tx.QueryRow(ctx, `SELECT workspace_id FROM dependency_graph_plan WHERE id = $1`, planID).Scan(&planWorkspaceID); err != nil {
		return fmt.Errorf("verify dependency graph plan: %w", err)
	}
	if planWorkspaceID != workspaceID {
		return errors.New("dependency graph fixture plan belongs to another workspace; refusing to continue")
	}
	for _, node := range graphNodeFixtures {
		var nodeWorkspaceID pgtype.UUID
		if err := tx.QueryRow(ctx, `SELECT workspace_id FROM dependency_graph_node WHERE id = $1`, pgUUID(fixtureID("graph/node/"+node.tempID))).Scan(&nodeWorkspaceID); err != nil {
			return fmt.Errorf("verify dependency graph node %q: %w", node.tempID, err)
		}
		if nodeWorkspaceID != workspaceID {
			return fmt.Errorf("dependency graph node %q belongs to another workspace; refusing to continue", node.tempID)
		}
	}
	for _, edge := range graphEdgeFixtures {
		edgeKey := edge.fromIssueKey + "/" + edge.toIssueKey
		var edgeWorkspaceID pgtype.UUID
		if err := tx.QueryRow(ctx, `SELECT workspace_id FROM dependency_graph_edge WHERE id = $1`, pgUUID(fixtureID("graph/edge/"+edgeKey))).Scan(&edgeWorkspaceID); err != nil {
			return fmt.Errorf("verify dependency graph edge %q: %w", edgeKey, err)
		}
		if edgeWorkspaceID != workspaceID {
			return fmt.Errorf("dependency graph edge %q belongs to another workspace; refusing to continue", edgeKey)
		}
	}
	return nil
}

func fixtureID(name string) string {
	return uuid.NewSHA1(fixtureNamespace, []byte("patchbay/dev-fixtures/v1/"+name)).String()
}

func pgUUID(value string) pgtype.UUID {
	id := uuid.MustParse(value)
	return pgtype.UUID{Bytes: [16]byte(id), Valid: true}
}
