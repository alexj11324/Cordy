package handler

import (
	"context"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/go-chi/chi/v5"
	"github.com/jackc/pgx/v5/pgtype"
	"github.com/patchbay-ai/patchbay/server/internal/issueposition"
	"github.com/patchbay-ai/patchbay/server/internal/issuestatus"
	"github.com/patchbay-ai/patchbay/server/internal/service"
	"github.com/patchbay-ai/patchbay/server/internal/util"
	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
	"github.com/patchbay-ai/patchbay/server/pkg/dbid"
	"github.com/patchbay-ai/patchbay/server/pkg/protocol"
)

const (
	dependencyGraphHardType          = "hard"
	dependencyGraphMaxTasks          = 128
	dependencyGraphMaxEdges          = 512
	dependencyGraphMaxGoal           = 8000
	dependencyGraphMaxTempID         = 64
	dependencyGraphMaxTitle          = 500
	dependencyGraphMaxDescription    = 30000
	dependencyGraphMaxTextItem       = 2000
	dependencyGraphMaxEdgeReason     = 4000
	dependencyGraphMaxPageSize       = 64
	dependencyGraphDefaultPageSize   = 20
	dependencyGraphMaxIdempotencyKey = 200
)

// dependencyGraphRoleInput is a typed issue role. The role field determines
// which actor kinds are valid: owner is a workspace member, executor is an
// agent or team, and reviewer may be a member, agent, or team.
type dependencyGraphRoleInput struct {
	Type string `json:"type"`
	ID   string `json:"id"`
}

type dependencyGraphTaskInput struct {
	TempID             string                     `json:"temp_id"`
	Title              string                     `json:"title"`
	Description        string                     `json:"description"`
	AcceptanceCriteria []string                   `json:"acceptance_criteria"`
	Context            json.RawMessage            `json:"context"`
	Outputs            []string                   `json:"outputs"`
	Owner              *dependencyGraphRoleInput  `json:"owner"`
	Executor           *dependencyGraphRoleInput  `json:"executor"`
	CandidateExecutors []dependencyGraphRoleInput `json:"candidate_executors"`
	Reviewer           *dependencyGraphRoleInput  `json:"reviewer"`
	RuntimeID          *string                    `json:"runtime_id"`
	ModelID            *string                    `json:"model_id"`
}

type dependencyGraphEdgeInput struct {
	From           string `json:"from"`
	To             string `json:"to"`
	Type           string `json:"type"`
	Reason         string `json:"reason"`
	ConsumedOutput string `json:"consumed_output"`
}

type dependencyGraphApplyInput struct {
	Goal          string                     `json:"goal"`
	ParentIssueID string                     `json:"parent_issue_id"`
	Tasks         []dependencyGraphTaskInput `json:"tasks"`
	Edges         []dependencyGraphEdgeInput `json:"edges"`
}

type dependencyGraphError struct {
	status int
	code   string
	msg    string
	cause  error
}

func (e *dependencyGraphError) Error() string {
	if e.cause == nil {
		return e.msg
	}
	return fmt.Sprintf("%s: %v", e.msg, e.cause)
}

func invalidDependencyGraph(message string) error {
	return &dependencyGraphError{status: http.StatusUnprocessableEntity, code: "invalid_plan", msg: message}
}

func dependencyGraphNotFound(message string) error {
	return &dependencyGraphError{status: http.StatusNotFound, code: "not_found", msg: message}
}

func dependencyGraphConflict(code, message string) error {
	return &dependencyGraphError{status: http.StatusConflict, code: code, msg: message}
}

func dependencyGraphInvalidReference(code, message string) error {
	return &dependencyGraphError{status: http.StatusUnprocessableEntity, code: code, msg: message}
}

func dependencyGraphDatabase(message string, cause error) error {
	return &dependencyGraphError{status: http.StatusInternalServerError, code: "database_error", msg: message, cause: cause}
}

func dependencyGraphIntegrity(message string) error {
	return &dependencyGraphError{status: http.StatusInternalServerError, code: "graph_integrity", msg: message}
}

func writeDependencyGraphError(w http.ResponseWriter, err error) {
	var graphErr *dependencyGraphError
	if errors.As(err, &graphErr) {
		if graphErr.status >= http.StatusInternalServerError {
			slog.Error("dependency graph operation failed", "error", err)
			writeErrorCode(w, graphErr.status, graphErr.code, "dependency graph operation failed")
			return
		}
		writeErrorCode(w, graphErr.status, graphErr.code, graphErr.msg)
		return
	}
	slog.Error("dependency graph operation failed", "error", err)
	writeErrorCode(w, http.StatusInternalServerError, "database_error", "dependency graph operation failed")
}

func validateDependencyGraphText(value, field string, maxLength int, required bool) error {
	if required && strings.TrimSpace(value) == "" {
		return invalidDependencyGraph(field + " is required")
	}
	if utf8.RuneCountInString(value) > maxLength {
		return invalidDependencyGraph(fmt.Sprintf("%s exceeds %d characters", field, maxLength))
	}
	if value != strings.TrimSpace(value) {
		return invalidDependencyGraph(field + " must not have surrounding whitespace")
	}
	return nil
}

func canonicalDependencyGraphJSON(raw json.RawMessage, field string, requireObject bool) ([]byte, error) {
	if len(raw) == 0 {
		raw = json.RawMessage(`{}`)
	}
	var value any
	if err := json.Unmarshal(raw, &value); err != nil {
		return nil, invalidDependencyGraph(field + " must be valid JSON")
	}
	if requireObject {
		if _, ok := value.(map[string]any); !ok {
			return nil, invalidDependencyGraph(field + " must be a JSON object")
		}
	}
	canonical, err := json.Marshal(value)
	if err != nil {
		return nil, invalidDependencyGraph(field + " could not be normalized")
	}
	return canonical, nil
}

func validateDependencyGraphRoleShape(role *dependencyGraphRoleInput, field string) (pgtype.UUID, error) {
	if role == nil {
		return pgtype.UUID{}, nil
	}
	if role.Type != "member" && role.Type != "agent" && role.Type != "team" {
		return pgtype.UUID{}, invalidDependencyGraph(field + ".type must be member, agent, or team")
	}
	u, err := util.ParseUUID(role.ID)
	if err != nil || !u.Valid {
		return pgtype.UUID{}, invalidDependencyGraph(field + ".id must be a non-nil UUID")
	}
	role.ID = uuidToString(u)
	return u, nil
}

func validateDependencyGraphExecutorShape(executor *dependencyGraphRoleInput, field string) (pgtype.UUID, error) {
	id, err := validateDependencyGraphRoleShape(executor, field)
	if err != nil || executor == nil {
		return id, err
	}
	if executor.Type != "agent" && executor.Type != "team" {
		return pgtype.UUID{}, invalidDependencyGraph(field + ".type must be agent or team")
	}
	return id, nil
}

func validateDependencyGraphOwnerShape(owner *dependencyGraphRoleInput, field string) (pgtype.UUID, error) {
	id, err := validateDependencyGraphRoleShape(owner, field)
	if err != nil || owner == nil {
		return id, err
	}
	if owner.Type != "member" {
		return pgtype.UUID{}, invalidDependencyGraph(field + ".type must be member")
	}
	return id, nil
}

func validateDependencyGraphExecutionTarget(runtimeID, modelID *string, field string) (pgtype.UUID, pgtype.Text, error) {
	if runtimeID == nil && modelID == nil {
		return pgtype.UUID{}, pgtype.Text{}, nil
	}
	if runtimeID == nil || modelID == nil {
		return pgtype.UUID{}, pgtype.Text{}, invalidDependencyGraph(field + ".runtime_id and " + field + ".model_id must be provided together")
	}
	id, err := util.ParseUUID(*runtimeID)
	if err != nil || !id.Valid {
		return pgtype.UUID{}, pgtype.Text{}, invalidDependencyGraph(field + ".runtime_id must be a non-nil UUID")
	}
	if err := validateDependencyGraphText(*modelID, field+".model_id", 255, true); err != nil {
		return pgtype.UUID{}, pgtype.Text{}, err
	}
	return id, pgtype.Text{String: *modelID, Valid: true}, nil
}

type dependencyGraphAdjacency struct {
	to        int
	edgeIndex int
}

func dependencyGraphHasPath(adjacency [][]dependencyGraphAdjacency, source, target, ignoredEdge int) bool {
	queue := []int{source}
	seen := make(map[int]struct{}, len(adjacency))
	for len(queue) > 0 {
		current := queue[0]
		queue = queue[1:]
		if current == target {
			return true
		}
		if _, ok := seen[current]; ok {
			continue
		}
		seen[current] = struct{}{}
		for _, next := range adjacency[current] {
			if next.edgeIndex != ignoredEdge {
				queue = append(queue, next.to)
			}
		}
	}
	return false
}

func validateDependencyGraphPlan(input *dependencyGraphApplyInput) ([][]string, error) {
	if err := validateDependencyGraphText(input.Goal, "goal", dependencyGraphMaxGoal, true); err != nil {
		return nil, err
	}
	parentID, err := util.ParseUUID(input.ParentIssueID)
	if err != nil || !parentID.Valid {
		return nil, invalidDependencyGraph("parent_issue_id must be a non-nil UUID")
	}
	input.ParentIssueID = uuidToString(parentID)
	if len(input.Tasks) == 0 {
		return nil, invalidDependencyGraph("tasks must contain at least one task")
	}
	if len(input.Tasks) > dependencyGraphMaxTasks {
		return nil, invalidDependencyGraph(fmt.Sprintf("tasks cannot exceed %d entries", dependencyGraphMaxTasks))
	}
	if len(input.Edges) > dependencyGraphMaxEdges {
		return nil, invalidDependencyGraph(fmt.Sprintf("edges cannot exceed %d entries", dependencyGraphMaxEdges))
	}
	if input.Edges == nil {
		input.Edges = []dependencyGraphEdgeInput{}
	}

	taskIndexes := make(map[string]int, len(input.Tasks))
	for index := range input.Tasks {
		task := &input.Tasks[index]
		field := fmt.Sprintf("tasks[%d]", index)
		if err := validateDependencyGraphText(task.TempID, field+".temp_id", dependencyGraphMaxTempID, true); err != nil {
			return nil, err
		}
		if _, exists := taskIndexes[task.TempID]; exists {
			return nil, invalidDependencyGraph(fmt.Sprintf("duplicate task temp_id %q", task.TempID))
		}
		taskIndexes[task.TempID] = index
		if err := validateDependencyGraphText(task.Title, field+".title", dependencyGraphMaxTitle, true); err != nil {
			return nil, err
		}
		if err := validateDependencyGraphText(task.Description, field+".description", dependencyGraphMaxDescription, false); err != nil {
			return nil, err
		}
		if len(task.AcceptanceCriteria) == 0 {
			return nil, invalidDependencyGraph(field + ".acceptance_criteria must contain at least one criterion")
		}
		if task.AcceptanceCriteria == nil {
			task.AcceptanceCriteria = []string{}
		}
		for criterionIndex, criterion := range task.AcceptanceCriteria {
			if err := validateDependencyGraphText(criterion, fmt.Sprintf("%s.acceptance_criteria[%d]", field, criterionIndex), dependencyGraphMaxTextItem, true); err != nil {
				return nil, err
			}
		}
		if len(task.Outputs) == 0 {
			return nil, invalidDependencyGraph(field + ".outputs must contain at least one observable output")
		}
		if task.Outputs == nil {
			task.Outputs = []string{}
		}
		outputs := make(map[string]struct{}, len(task.Outputs))
		for outputIndex, output := range task.Outputs {
			if err := validateDependencyGraphText(output, fmt.Sprintf("%s.outputs[%d]", field, outputIndex), dependencyGraphMaxTextItem, true); err != nil {
				return nil, err
			}
			if _, exists := outputs[output]; exists {
				return nil, invalidDependencyGraph(fmt.Sprintf("%s.outputs contains duplicate output %q", field, output))
			}
			outputs[output] = struct{}{}
		}
		contextJSON, err := canonicalDependencyGraphJSON(task.Context, field+".context", true)
		if err != nil {
			return nil, err
		}
		task.Context = contextJSON
		if task.CandidateExecutors == nil {
			task.CandidateExecutors = []dependencyGraphRoleInput{}
		}
		if _, err := validateDependencyGraphOwnerShape(task.Owner, field+".owner"); err != nil {
			return nil, err
		}
		if _, err := validateDependencyGraphExecutorShape(task.Executor, field+".executor"); err != nil {
			return nil, err
		}
		for candidateIndex := range task.CandidateExecutors {
			if _, err := validateDependencyGraphExecutorShape(&task.CandidateExecutors[candidateIndex], fmt.Sprintf("%s.candidate_executors[%d]", field, candidateIndex)); err != nil {
				return nil, err
			}
		}
		if _, err := validateDependencyGraphRoleShape(task.Reviewer, field+".reviewer"); err != nil {
			return nil, err
		}
		if _, _, err := validateDependencyGraphExecutionTarget(task.RuntimeID, task.ModelID, field); err != nil {
			return nil, err
		}
	}

	adjacency := make([][]dependencyGraphAdjacency, len(input.Tasks))
	indegree := make([]int, len(input.Tasks))
	edgePairs := make(map[[2]int]struct{}, len(input.Edges))
	for edgeIndex := range input.Edges {
		edge := &input.Edges[edgeIndex]
		field := fmt.Sprintf("edges[%d]", edgeIndex)
		if err := validateDependencyGraphText(edge.From, field+".from", dependencyGraphMaxTempID, true); err != nil {
			return nil, err
		}
		if err := validateDependencyGraphText(edge.To, field+".to", dependencyGraphMaxTempID, true); err != nil {
			return nil, err
		}
		if edge.Type != dependencyGraphHardType {
			return nil, invalidDependencyGraph(field + ".type must be hard in V1")
		}
		if err := validateDependencyGraphText(edge.Reason, field+".reason", dependencyGraphMaxEdgeReason, true); err != nil {
			return nil, err
		}
		if err := validateDependencyGraphText(edge.ConsumedOutput, field+".consumed_output", dependencyGraphMaxTextItem, true); err != nil {
			return nil, err
		}
		from, fromOK := taskIndexes[edge.From]
		to, toOK := taskIndexes[edge.To]
		if !fromOK {
			return nil, invalidDependencyGraph(fmt.Sprintf("%s.from references unknown task %q", field, edge.From))
		}
		if !toOK {
			return nil, invalidDependencyGraph(fmt.Sprintf("%s.to references unknown task %q", field, edge.To))
		}
		if from == to {
			return nil, invalidDependencyGraph(fmt.Sprintf("%s cannot be a self dependency", field))
		}
		pair := [2]int{from, to}
		if _, exists := edgePairs[pair]; exists {
			return nil, invalidDependencyGraph(fmt.Sprintf("duplicate dependency edge %s -> %s", edge.From, edge.To))
		}
		edgePairs[pair] = struct{}{}
		foundOutput := false
		for _, output := range input.Tasks[from].Outputs {
			if output == edge.ConsumedOutput {
				foundOutput = true
				break
			}
		}
		if !foundOutput {
			return nil, invalidDependencyGraph(fmt.Sprintf("%s.consumed_output %q is not an output of %s", field, edge.ConsumedOutput, edge.From))
		}
		adjacency[from] = append(adjacency[from], dependencyGraphAdjacency{to: to, edgeIndex: edgeIndex})
		indegree[to]++
	}
	for edgeIndex, edge := range input.Edges {
		from := taskIndexes[edge.From]
		to := taskIndexes[edge.To]
		if dependencyGraphHasPath(adjacency, to, from, edgeIndex) {
			return nil, invalidDependencyGraph("dependency graph contains a cycle")
		}
	}
	for edgeIndex, edge := range input.Edges {
		from := taskIndexes[edge.From]
		to := taskIndexes[edge.To]
		if dependencyGraphHasPath(adjacency, from, to, edgeIndex) {
			return nil, invalidDependencyGraph(fmt.Sprintf("dependency edge %s -> %s is transitively redundant", edge.From, edge.To))
		}
	}

	current := make([]int, 0, len(input.Tasks))
	for index, degree := range indegree {
		if degree == 0 {
			current = append(current, index)
		}
	}
	waves := make([][]string, 0, len(input.Tasks))
	visited := 0
	for len(current) > 0 {
		wave := make([]string, 0, len(current))
		next := make([]int, 0)
		for _, index := range current {
			visited++
			wave = append(wave, input.Tasks[index].TempID)
			for _, dependent := range adjacency[index] {
				indegree[dependent.to]--
				if indegree[dependent.to] == 0 {
					next = append(next, dependent.to)
				}
			}
		}
		waves = append(waves, wave)
		current = next
	}
	if visited != len(input.Tasks) {
		return nil, invalidDependencyGraph("dependency graph contains a cycle")
	}
	return waves, nil
}

func dependencyGraphRequestHash(input *dependencyGraphApplyInput) (string, error) {
	encoded, err := json.Marshal(input)
	if err != nil {
		return "", err
	}
	digest := sha256.Sum256(encoded)
	return fmt.Sprintf("sha256:%x", digest[:]), nil
}

type dependencyGraphCursor struct {
	Version   int       `json:"v"`
	ProjectID *string   `json:"project_id"`
	UpdatedAt time.Time `json:"updated_at"`
	ID        string    `json:"id"`
	Offset    int       `json:"offset"`
}

type dependencyGraphAfter struct {
	updatedAt time.Time
	id        pgtype.UUID
	offset    int
}

func decodeDependencyGraphCursor(raw string, projectID *pgtype.UUID) (*dependencyGraphAfter, error) {
	if strings.TrimSpace(raw) == "" {
		return nil, nil
	}
	raw = strings.TrimSpace(raw)
	decoded, err := base64.RawURLEncoding.DecodeString(raw)
	if err != nil {
		decoded, err = base64.URLEncoding.DecodeString(raw)
	}
	if err != nil {
		decoded, err = base64.RawStdEncoding.DecodeString(raw)
	}
	if err != nil {
		decoded, err = base64.StdEncoding.DecodeString(raw)
	}
	if err != nil {
		return nil, invalidDependencyGraph("invalid dependency graph cursor")
	}
	var cursor dependencyGraphCursor
	if err := json.Unmarshal(decoded, &cursor); err != nil || cursor.Version != 1 || cursor.UpdatedAt.IsZero() || cursor.Offset < 0 || int64(cursor.Offset) > int64(1<<31-1) {
		return nil, invalidDependencyGraph("invalid dependency graph cursor")
	}
	if cursor.ProjectID != nil {
		parsed, parseErr := util.ParseUUID(*cursor.ProjectID)
		if parseErr != nil || !parsed.Valid {
			return nil, invalidDependencyGraph("invalid dependency graph cursor")
		}
		canonical := uuidToString(parsed)
		if canonical != *cursor.ProjectID {
			cursor.ProjectID = &canonical
		}
	}
	requestedProject := ""
	if projectID != nil {
		requestedProject = uuidToString(*projectID)
	}
	cursorProject := ""
	if cursor.ProjectID != nil {
		cursorProject = *cursor.ProjectID
	}
	if requestedProject != cursorProject {
		return nil, dependencyGraphConflict("cursor_project_mismatch", "dependency graph cursor does not belong to this project query")
	}
	id, err := util.ParseUUID(cursor.ID)
	if err != nil || !id.Valid {
		return nil, invalidDependencyGraph("invalid dependency graph cursor")
	}
	return &dependencyGraphAfter{updatedAt: cursor.UpdatedAt.UTC(), id: id, offset: cursor.Offset}, nil
}

func encodeDependencyGraphCursor(projectID *pgtype.UUID, plan db.DependencyGraphPlan) (string, error) {
	return encodeDependencyGraphCursorAt(projectID, plan, 0)
}

func encodeDependencyGraphCursorAt(projectID *pgtype.UUID, plan db.DependencyGraphPlan, offset int) (string, error) {
	if offset < 0 {
		return "", invalidDependencyGraph("invalid dependency graph cursor offset")
	}
	var cursorProject *string
	if projectID != nil {
		value := uuidToString(*projectID)
		cursorProject = &value
	}
	cursor := dependencyGraphCursor{
		Version:   1,
		ProjectID: cursorProject,
		UpdatedAt: plan.UpdatedAt.Time.UTC(),
		ID:        uuidToString(plan.ID),
		Offset:    offset,
	}
	encoded, err := json.Marshal(cursor)
	if err != nil {
		return "", err
	}
	return base64.RawURLEncoding.EncodeToString(encoded), nil
}

type dependencyGraphPlanResponse struct {
	ID                string  `json:"id"`
	WorkspaceID       string  `json:"workspace_id"`
	ParentIssueID     string  `json:"parent_issue_id"`
	IdempotencyKey    string  `json:"idempotency_key"`
	RequestHash       string  `json:"request_hash"`
	Goal              string  `json:"goal"`
	Status            string  `json:"status"`
	CreatedByType     string  `json:"created_by_type"`
	CreatedByID       string  `json:"created_by_id"`
	CreatedAt         string  `json:"created_at"`
	UpdatedAt         string  `json:"updated_at"`
	AttentionRequired bool    `json:"attention_required"`
	AttentionReason   *string `json:"attention_reason"`
}

type dependencyGraphNodeResponse struct {
	ID                 string          `json:"id"`
	PlanID             string          `json:"plan_id"`
	WorkspaceID        string          `json:"workspace_id"`
	TempID             string          `json:"temp_id"`
	IssueID            string          `json:"issue_id"`
	Issue              IssueResponse   `json:"issue"`
	Title              string          `json:"title"`
	Description        string          `json:"description"`
	AcceptanceCriteria json.RawMessage `json:"acceptance_criteria"`
	Context            json.RawMessage `json:"context"`
	Outputs            json.RawMessage `json:"outputs"`
	ExecutorType       *string         `json:"executor_type"`
	ExecutorID         *string         `json:"executor_id"`
	CandidateExecutors json.RawMessage `json:"candidate_executors"`
	Wave               int32           `json:"wave"`
	CreatedAt          string          `json:"created_at"`
	UpdatedAt          string          `json:"updated_at"`
	OwnerType          *string         `json:"owner_type"`
	OwnerID            *string         `json:"owner_id"`
	ReviewerType       *string         `json:"reviewer_type"`
	ReviewerID         *string         `json:"reviewer_id"`
	RuntimeID          *string         `json:"runtime_id"`
	ModelID            *string         `json:"model_id"`
	Status             string          `json:"status"`
	StatusCategory     string          `json:"status_category"`
	Ready              bool            `json:"ready"`
	BlockedBy          []string        `json:"blocked_by"`
	Readiness          dependencyGraphNodeReadinessResponse `json:"readiness"`
}

// dependencyGraphNodeReadinessResponse mirrors the frontend
// DependencyGraphNodeReadinessSchema: the derived gate state the task-graph
// page filters and counts on. Raw status/category stay on the node itself.
type dependencyGraphNodeReadinessResponse struct {
	State                  string `json:"state"`
	GateOpen               bool   `json:"gate_open"`
	SatisfiedPrerequisites int    `json:"satisfied_prerequisites"`
	TotalPrerequisites     int    `json:"total_prerequisites"`
	UnlockCondition        string `json:"unlock_condition"`
}

type dependencyGraphEdgeResponse struct {
	ID                     string `json:"id"`
	PlanID                 string `json:"plan_id"`
	WorkspaceID            string `json:"workspace_id"`
	FromIssueID            string `json:"from_issue_id"`
	ToIssueID              string `json:"to_issue_id"`
	From                   string `json:"from"`
	To                     string `json:"to"`
	Type                   string `json:"type"`
	Reason                 string `json:"reason"`
	ConsumedOutput         string `json:"consumed_output"`
	CreatedAt              string `json:"created_at"`
	PrerequisiteStatus     string `json:"prerequisite_status"`
	Satisfied              bool   `json:"satisfied"`
	SatisfiedPrerequisites int    `json:"satisfied_prerequisites"`
	TotalPrerequisites     int    `json:"total_prerequisites"`
	UnlockCondition        string `json:"unlock_condition"`
}

type dependencyGraphReadinessResponse struct {
	Total     int `json:"total"`
	Ready     int `json:"ready"`
	Running   int `json:"running"`
	Blocked   int `json:"blocked"`
	Done      int `json:"done"`
	Cancelled int `json:"cancelled"`
}

type dependencyGraphResponse struct {
	Plan      dependencyGraphPlanResponse   `json:"plan"`
	Nodes     []dependencyGraphNodeResponse `json:"nodes"`
	Edges     []dependencyGraphEdgeResponse `json:"edges"`
	Readiness dependencyGraphReadinessResponse `json:"readiness"`
}

func dependencyGraphPlanToResponse(plan db.DependencyGraphPlan) dependencyGraphPlanResponse {
	return dependencyGraphPlanResponse{
		ID:                uuidToString(plan.ID),
		WorkspaceID:       uuidToString(plan.WorkspaceID),
		ParentIssueID:     uuidToString(plan.ParentIssueID),
		IdempotencyKey:    plan.IdempotencyKey,
		RequestHash:       plan.RequestHash,
		Goal:              plan.Goal,
		Status:            plan.Status,
		CreatedByType:     plan.CreatedByType,
		CreatedByID:       uuidToString(plan.CreatedByID),
		CreatedAt:         timestampToString(plan.CreatedAt),
		UpdatedAt:         timestampToString(plan.UpdatedAt),
		AttentionRequired: plan.AttentionRequired,
		AttentionReason:   textToPtr(plan.AttentionReason),
	}
}

func dependencyGraphResponseMap(response dependencyGraphResponse, includeReplay bool, replayed bool) map[string]any {
	payload := map[string]any{
		"plan":      response.Plan,
		"nodes":     response.Nodes,
		"edges":     response.Edges,
		"readiness": response.Readiness,
	}
	if includeReplay {
		payload["replayed"] = replayed
	}
	return payload
}

func dependencyGraphJSONResponse(raw []byte, fallback string) (json.RawMessage, error) {
	if len(raw) == 0 {
		raw = []byte(fallback)
	}
	var value any
	if err := json.Unmarshal(raw, &value); err != nil {
		return nil, dependencyGraphIntegrity("dependency graph contains invalid stored JSON")
	}
	canonical, err := json.Marshal(value)
	if err != nil {
		return nil, dependencyGraphIntegrity("dependency graph JSON could not be encoded")
	}
	return json.RawMessage(canonical), nil
}

type dependencyGraphActor struct {
	Type        string
	ID          pgtype.UUID
	WorkspaceID pgtype.UUID
	Member      db.Member
}

func (h *Handler) dependencyGraphActorForRequest(w http.ResponseWriter, r *http.Request, workspaceID string) (dependencyGraphActor, bool) {
	if workspaceID == "" {
		writeError(w, http.StatusBadRequest, "workspace_id is required")
		return dependencyGraphActor{}, false
	}
	wsUUID, err := util.ParseUUID(workspaceID)
	if err != nil || !wsUUID.Valid {
		writeError(w, http.StatusBadRequest, "invalid workspace id")
		return dependencyGraphActor{}, false
	}
	userID, ok := requireUserID(w, r)
	if !ok {
		return dependencyGraphActor{}, false
	}
	userUUID, err := util.ParseUUID(userID)
	if err != nil || !userUUID.Valid {
		writeErrorCode(w, http.StatusUnauthorized, "invalid_actor", "user identity is invalid")
		return dependencyGraphActor{}, false
	}
	if h.Queries == nil {
		writeDependencyGraphError(w, dependencyGraphDatabase("dependency graph queries are unavailable", nil))
		return dependencyGraphActor{}, false
	}
	member, ok := h.workspaceMember(w, r, workspaceID)
	if !ok {
		return dependencyGraphActor{}, false
	}
	if member.WorkspaceID != wsUUID || member.UserID != userUUID {
		writeErrorCode(w, http.StatusForbidden, "invalid_actor", "actor is not a member of this workspace")
		return dependencyGraphActor{}, false
	}
	actorType, actorID := h.resolveActor(r, userID, workspaceID)
	if r.Header.Get("X-Agent-ID") != "" && actorType != "agent" {
		writeErrorCode(w, http.StatusForbidden, "invalid_actor", "agent actor could not be validated")
		return dependencyGraphActor{}, false
	}
	if r.Header.Get("X-Actor-Source") == "task_token" && actorType != "agent" {
		writeErrorCode(w, http.StatusForbidden, "invalid_actor", "task actor could not be validated")
		return dependencyGraphActor{}, false
	}
	if actorType == "member" {
		return dependencyGraphActor{Type: "member", ID: userUUID, WorkspaceID: wsUUID, Member: member}, true
	}
	if actorType != "agent" {
		writeErrorCode(w, http.StatusForbidden, "invalid_actor", "unsupported actor type")
		return dependencyGraphActor{}, false
	}
	actorUUID, err := util.ParseUUID(actorID)
	if err != nil || !actorUUID.Valid {
		writeErrorCode(w, http.StatusForbidden, "invalid_actor", "agent actor id is invalid")
		return dependencyGraphActor{}, false
	}
	agent, err := h.Queries.GetAgentInWorkspace(r.Context(), db.GetAgentInWorkspaceParams{ID: actorUUID, WorkspaceID: wsUUID})
	if err != nil || agent.ArchivedAt.Valid {
		writeErrorCode(w, http.StatusForbidden, "invalid_actor", "agent actor is not active in this workspace")
		return dependencyGraphActor{}, false
	}
	return dependencyGraphActor{Type: "agent", ID: actorUUID, WorkspaceID: wsUUID, Member: member}, true
}

func (h *Handler) dependencyGraphResponseForPlan(ctx context.Context, plan db.DependencyGraphPlan) (dependencyGraphResponse, error) {
	if h.Queries == nil {
		return dependencyGraphResponse{}, dependencyGraphDatabase("dependency graph queries are unavailable", nil)
	}
	if !plan.ID.Valid || !plan.WorkspaceID.Valid || !plan.ParentIssueID.Valid {
		return dependencyGraphResponse{}, dependencyGraphIntegrity("dependency graph plan contains an invalid identity")
	}
	if _, err := h.Queries.GetIssueInWorkspace(ctx, db.GetIssueInWorkspaceParams{ID: plan.ParentIssueID, WorkspaceID: plan.WorkspaceID}); err != nil {
		if isNotFound(err) {
			return dependencyGraphResponse{}, dependencyGraphIntegrity("dependency graph parent issue is missing")
		}
		return dependencyGraphResponse{}, dependencyGraphDatabase("load dependency graph parent issue", err)
	}
	nodes, err := h.Queries.ListDependencyGraphNodesByPlan(ctx, db.ListDependencyGraphNodesByPlanParams{PlanID: plan.ID, WorkspaceID: plan.WorkspaceID})
	if err != nil {
		return dependencyGraphResponse{}, dependencyGraphDatabase("load dependency graph nodes", err)
	}
	edges, err := h.Queries.ListDependencyGraphEdgesByPlan(ctx, db.ListDependencyGraphEdgesByPlanParams{PlanID: plan.ID, WorkspaceID: plan.WorkspaceID})
	if err != nil {
		return dependencyGraphResponse{}, dependencyGraphDatabase("load dependency graph edges", err)
	}
	issues := make(map[string]db.Issue, len(nodes))
	for _, node := range nodes {
		if node.PlanID != plan.ID || node.WorkspaceID != plan.WorkspaceID || !node.IssueID.Valid {
			return dependencyGraphResponse{}, dependencyGraphIntegrity("dependency graph node crosses its plan workspace")
		}
		issue, err := h.Queries.GetIssueInWorkspace(ctx, db.GetIssueInWorkspaceParams{ID: node.IssueID, WorkspaceID: plan.WorkspaceID})
		if err != nil {
			if isNotFound(err) {
				return dependencyGraphResponse{}, dependencyGraphIntegrity("dependency graph node issue is missing")
			}
			return dependencyGraphResponse{}, dependencyGraphDatabase("load dependency graph node issue", err)
		}
		key := uuidToString(node.IssueID)
		if _, exists := issues[key]; exists {
			return dependencyGraphResponse{}, dependencyGraphIntegrity("dependency graph contains duplicate node issues")
		}
		issues[key] = issue
	}
	incoming := make(map[string][]string, len(nodes))
	for _, edge := range edges {
		if edge.PlanID != plan.ID || edge.WorkspaceID != plan.WorkspaceID {
			return dependencyGraphResponse{}, dependencyGraphIntegrity("dependency graph edge crosses its plan workspace")
		}
		from := uuidToString(edge.FromIssueID)
		to := uuidToString(edge.ToIssueID)
		if _, ok := issues[from]; !ok {
			return dependencyGraphResponse{}, dependencyGraphIntegrity("dependency graph edge source is not a node")
		}
		if _, ok := issues[to]; !ok {
			return dependencyGraphResponse{}, dependencyGraphIntegrity("dependency graph edge target is not a node")
		}
		incoming[to] = append(incoming[to], from)
	}
	statusQueries := h.IssueStatusCatalog
	if statusQueries == nil {
		statusQueries = h.Queries
	}
	resolver := issuestatus.NewResolver(plan.WorkspaceID)
	issuePrefix := h.getIssuePrefix(ctx, plan.WorkspaceID)
	response := dependencyGraphResponse{
		Plan:  dependencyGraphPlanToResponse(plan),
		Nodes: make([]dependencyGraphNodeResponse, 0, len(nodes)),
		Edges: make([]dependencyGraphEdgeResponse, 0, len(edges)),
	}
	categoryOf := make(map[string]string, len(nodes))
	stateOf := make(map[string]string, len(nodes))
	for _, node := range nodes {
		issue := issues[uuidToString(node.IssueID)]
		category := resolver.Effective(ctx, statusQueries, issue.Status)
		blockedBy := append([]string(nil), incoming[uuidToString(node.IssueID)]...)
		if plan.Status == "active" {
			filtered := blockedBy[:0]
			for _, prerequisiteID := range blockedBy {
				prerequisite := issues[prerequisiteID]
				if resolver.Effective(ctx, statusQueries, prerequisite.Status) != issuestatus.Done {
					filtered = append(filtered, prerequisiteID)
				}
			}
			blockedBy = filtered
		} else {
			blockedBy = []string{}
		}
		if len(blockedBy) == 0 {
			blockedBy = []string{}
		}
		sort.Strings(blockedBy)
		acceptance, err := dependencyGraphJSONResponse(node.AcceptanceCriteria, "[]")
		if err != nil {
			return dependencyGraphResponse{}, err
		}
		contextJSON, err := dependencyGraphJSONResponse(node.Context, "{}")
		if err != nil {
			return dependencyGraphResponse{}, err
		}
		outputs, err := dependencyGraphJSONResponse(node.Outputs, "[]")
		if err != nil {
			return dependencyGraphResponse{}, err
		}
		candidates, err := dependencyGraphJSONResponse(node.CandidateExecutors, "[]")
		if err != nil {
			return dependencyGraphResponse{}, err
		}
		issueKey := uuidToString(node.IssueID)
		categoryOf[issueKey] = category
		satisfied := 0
		for _, prerequisiteID := range incoming[issueKey] {
			if prerequisite, ok := issues[prerequisiteID]; ok {
				if resolver.Effective(ctx, statusQueries, prerequisite.Status) == issuestatus.Done {
					satisfied++
				}
			}
		}
		gateOpen := plan.Status == "active" && len(blockedBy) == 0 && category != issuestatus.Done && category != issuestatus.Cancelled
		state := dependencyGraphNodeReadinessState(category, gateOpen, len(blockedBy) > 0)
		stateOf[issueKey] = state
		issueResponse := issueToResponse(issue, issuePrefix)
		h.fillStatusCategory(ctx, plan.WorkspaceID, &issueResponse)
		response.Nodes = append(response.Nodes, dependencyGraphNodeResponse{
			ID:                 uuidToString(node.ID),
			PlanID:             uuidToString(node.PlanID),
			WorkspaceID:        uuidToString(node.WorkspaceID),
			TempID:             node.TempID,
			IssueID:            uuidToString(node.IssueID),
			Title:              node.Title,
			Description:        node.Description,
			AcceptanceCriteria: acceptance,
			Context:            contextJSON,
			Outputs:            outputs,
			ExecutorType:       textToPtr(node.ExecutorType),
			ExecutorID:         uuidToPtr(node.ExecutorID),
			CandidateExecutors: candidates,
			Wave:               node.Wave,
			CreatedAt:          timestampToString(node.CreatedAt),
			UpdatedAt:          timestampToString(node.UpdatedAt),
			OwnerType:          textToPtr(node.OwnerType),
			OwnerID:            uuidToPtr(node.OwnerID),
			ReviewerType:       textToPtr(node.ReviewerType),
			ReviewerID:         uuidToPtr(node.ReviewerID),
			RuntimeID:          uuidToPtr(node.RuntimeID),
			ModelID:            textToPtr(node.ModelID),
			Status:             issue.Status,
			StatusCategory:     category,
			Ready:              gateOpen,
			BlockedBy:          blockedBy,
			Issue:              issueResponse,
			Readiness: dependencyGraphNodeReadinessResponse{
				State:                  state,
				GateOpen:               gateOpen,
				SatisfiedPrerequisites: satisfied,
				TotalPrerequisites:     len(incoming[issueKey]),
				UnlockCondition:        "",
			},
		})
	}
	for _, edge := range edges {
		fromKey := uuidToString(edge.FromIssueID)
		toKey := uuidToString(edge.ToIssueID)
		fromStatus := ""
		satisfied := false
		if fromIssue, ok := issues[fromKey]; ok {
			fromStatus = fromIssue.Status
			satisfied = categoryOf[fromKey] == issuestatus.Done
		}
		satisfiedCount := 0
		if satisfied {
			satisfiedCount = 1
		}
		response.Edges = append(response.Edges, dependencyGraphEdgeResponse{
			ID:                     uuidToString(edge.ID),
			PlanID:                 uuidToString(edge.PlanID),
			WorkspaceID:            uuidToString(edge.WorkspaceID),
			FromIssueID:            fromKey,
			ToIssueID:              toKey,
			From:                   fromKey,
			To:                     toKey,
			Type:                   edge.Type,
			Reason:                 edge.Reason,
			ConsumedOutput:         edge.ConsumedOutput,
			CreatedAt:              timestampToString(edge.CreatedAt),
			PrerequisiteStatus:     fromStatus,
			Satisfied:              satisfied,
			SatisfiedPrerequisites: satisfiedCount,
			TotalPrerequisites:     1,
			UnlockCondition:        "",
		})
	}
	for _, node := range response.Nodes {
		response.Readiness.Total++
		switch node.Readiness.State {
		case "ready":
			response.Readiness.Ready++
		case "running":
			response.Readiness.Running++
		case "blocked":
			response.Readiness.Blocked++
		case "done":
			response.Readiness.Done++
		case "cancelled":
			response.Readiness.Cancelled++
		}
	}
	return response, nil
}

// dependencyGraphNodeReadinessState derives the task-graph filter state from
// the issue category and the already-computed gate. Vocabulary matches the
// frontend GraphFilter/graph-utils states (ready/running/blocked + terminal
// done/cancelled, falling back to todo).
func dependencyGraphNodeReadinessState(category string, gateOpen bool, hasBlockers bool) string {
	switch category {
	case issuestatus.Done:
		return "done"
	case issuestatus.Cancelled:
		return "cancelled"
	case issuestatus.Blocked:
		return "blocked"
	case issuestatus.InProgress, issuestatus.InReview:
		return "running"
	}
	if hasBlockers {
		return "blocked"
	}
	if gateOpen {
		return "ready"
	}
	return "todo"
}

func dependencyGraphIdempotencyKey(r *http.Request) (string, error) {
	primary := r.Header.Get("Idempotency-Key")
	fallback := r.Header.Get("X-Idempotency-Key")
	if primary != "" && fallback != "" && strings.TrimSpace(primary) != strings.TrimSpace(fallback) {
		return "", dependencyGraphConflict("idempotency_key_conflict", "Idempotency-Key and X-Idempotency-Key do not match")
	}
	value := primary
	if value == "" {
		value = fallback
	}
	if strings.TrimSpace(value) == "" {
		return "", &dependencyGraphError{status: http.StatusBadRequest, code: "idempotency_key_required", msg: "Idempotency-Key header is required"}
	}
	if value != strings.TrimSpace(value) {
		return "", invalidDependencyGraph("idempotency key must not have surrounding whitespace")
	}
	if utf8.RuneCountInString(value) > dependencyGraphMaxIdempotencyKey {
		return "", invalidDependencyGraph(fmt.Sprintf("idempotency key exceeds %d characters", dependencyGraphMaxIdempotencyKey))
	}
	for _, r := range value {
		if r < 0x20 || r == 0x7f {
			return "", invalidDependencyGraph("idempotency key contains a control character")
		}
	}
	return value, nil
}

type dependencyGraphTaskAssignment struct {
	executorType       pgtype.Text
	executorID         pgtype.UUID
	ownerType          pgtype.Text
	ownerID            pgtype.UUID
	reviewerType       pgtype.Text
	reviewerID         pgtype.UUID
	candidateExecutors []byte
	runtimeID          pgtype.UUID
	modelID            pgtype.Text
}

func validateDependencyGraphActor(ctx context.Context, queries *db.Queries, workspaceID pgtype.UUID, actor dependencyGraphRoleInput, code string) error {
	id, err := util.ParseUUID(actor.ID)
	if err != nil || !id.Valid {
		return dependencyGraphInvalidReference(code, "role id is invalid")
	}
	switch actor.Type {
	case "member":
		if _, err := queries.GetMemberByUserAndWorkspace(ctx, db.GetMemberByUserAndWorkspaceParams{UserID: id, WorkspaceID: workspaceID}); err != nil {
			return dependencyGraphInvalidReference(code, "member is not in this workspace")
		}
	case "agent":
		agent, err := queries.GetAgentInWorkspace(ctx, db.GetAgentInWorkspaceParams{ID: id, WorkspaceID: workspaceID})
		if err != nil {
			return dependencyGraphInvalidReference(code, "agent is not in this workspace")
		}
		if agent.ArchivedAt.Valid {
			return dependencyGraphInvalidReference(code, "agent is archived")
		}
	case "team":
		team, err := queries.GetTeamInWorkspace(ctx, db.GetTeamInWorkspaceParams{ID: id, WorkspaceID: workspaceID})
		if err != nil {
			return dependencyGraphInvalidReference(code, "team is not in this workspace")
		}
		if team.ArchivedAt.Valid {
			return dependencyGraphInvalidReference(code, "team is archived")
		}
		leader, err := queries.GetAgent(ctx, team.LeaderID)
		if err != nil || leader.WorkspaceID != workspaceID || leader.ArchivedAt.Valid {
			return dependencyGraphInvalidReference(code, "team leader is not active")
		}
	default:
		return dependencyGraphInvalidReference(code, "role type is invalid")
	}
	return nil
}

func (h *Handler) validateDependencyGraphExecutor(ctx context.Context, r *http.Request, workspaceID pgtype.UUID, parent *db.Issue, executor *dependencyGraphRoleInput, field string) (pgtype.UUID, error) {
	id, err := validateDependencyGraphExecutorShape(executor, field)
	if err != nil || executor == nil {
		return id, err
	}
	if err := validateDependencyGraphActor(ctx, h.Queries, workspaceID, *executor, "invalid_executor"); err != nil {
		return pgtype.UUID{}, err
	}
	executorType := pgtype.Text{String: executor.Type, Valid: true}
	status, message := h.validateExecutorPair(ctx, r, uuidToString(workspaceID), executorType, id, scopeChildOf(parent))
	if status != 0 {
		return pgtype.UUID{}, &dependencyGraphError{status: status, code: "invalid_executor", msg: message}
	}
	return id, nil
}

func (h *Handler) validateDependencyGraphOwner(ctx context.Context, workspaceID pgtype.UUID, owner *dependencyGraphRoleInput, field string) (pgtype.UUID, error) {
	id, err := validateDependencyGraphOwnerShape(owner, field)
	if err != nil || owner == nil {
		return id, err
	}
	if err := validateDependencyGraphActor(ctx, h.Queries, workspaceID, *owner, "invalid_owner"); err != nil {
		return pgtype.UUID{}, err
	}
	return id, nil
}

func (h *Handler) validateDependencyGraphReviewer(ctx context.Context, r *http.Request, workspaceID pgtype.UUID, parent *db.Issue, reviewer *dependencyGraphRoleInput, field string) (pgtype.UUID, error) {
	id, err := validateDependencyGraphRoleShape(reviewer, field)
	if err != nil || reviewer == nil {
		return id, err
	}
	if err := validateDependencyGraphActor(ctx, h.Queries, workspaceID, *reviewer, "invalid_reviewer"); err != nil {
		return pgtype.UUID{}, err
	}
	if reviewer.Type == "member" {
		return id, nil
	}
	executorType := pgtype.Text{String: reviewer.Type, Valid: true}
	status, message := h.validateExecutorPair(ctx, r, uuidToString(workspaceID), executorType, id, scopeChildOf(parent))
	if status != 0 {
		return pgtype.UUID{}, &dependencyGraphError{status: status, code: "invalid_reviewer", msg: message}
	}
	return id, nil
}

func (h *Handler) validateDependencyGraphAssignments(ctx context.Context, r *http.Request, workspaceID pgtype.UUID, parent *db.Issue, input *dependencyGraphApplyInput) ([]dependencyGraphTaskAssignment, error) {
	assignments := make([]dependencyGraphTaskAssignment, len(input.Tasks))
	for index := range input.Tasks {
		task := &input.Tasks[index]
		assignment := dependencyGraphTaskAssignment{}
		if task.Owner != nil {
			ownerID, err := h.validateDependencyGraphOwner(ctx, workspaceID, task.Owner, fmt.Sprintf("tasks[%d].owner", index))
			if err != nil {
				return nil, err
			}
			assignment.ownerType = pgtype.Text{String: "member", Valid: true}
			assignment.ownerID = ownerID
		}
		if task.Executor != nil {
			executorID, err := h.validateDependencyGraphExecutor(ctx, r, workspaceID, parent, task.Executor, fmt.Sprintf("tasks[%d].executor", index))
			if err != nil {
				return nil, err
			}
			assignment.executorType = pgtype.Text{String: task.Executor.Type, Valid: true}
			assignment.executorID = executorID
		}
		if task.Reviewer != nil {
			reviewerID, err := h.validateDependencyGraphReviewer(ctx, r, workspaceID, parent, task.Reviewer, fmt.Sprintf("tasks[%d].reviewer", index))
			if err != nil {
				return nil, err
			}
			assignment.reviewerType = pgtype.Text{String: task.Reviewer.Type, Valid: true}
			assignment.reviewerID = reviewerID
		}
		seenCandidates := make(map[string]struct{}, len(task.CandidateExecutors))
		for candidateIndex := range task.CandidateExecutors {
			candidate := &task.CandidateExecutors[candidateIndex]
			candidateField := fmt.Sprintf("tasks[%d].candidate_executors[%d]", index, candidateIndex)
			if _, err := validateDependencyGraphExecutorShape(candidate, candidateField); err != nil {
				return nil, err
			}
			key := candidate.Type + "\x00" + candidate.ID
			if _, exists := seenCandidates[key]; exists {
				return nil, invalidDependencyGraph(fmt.Sprintf("tasks[%d].candidate_executors contains duplicate executor", index))
			}
			seenCandidates[key] = struct{}{}
			if _, err := h.validateDependencyGraphExecutor(ctx, r, workspaceID, parent, candidate, candidateField); err != nil {
				return nil, err
			}
		}
		candidateJSON, err := json.Marshal(task.CandidateExecutors)
		if err != nil {
			return nil, dependencyGraphDatabase("encode dependency graph candidates", err)
		}
		assignment.candidateExecutors = candidateJSON
		assignment.runtimeID, assignment.modelID, err = validateDependencyGraphExecutionTarget(task.RuntimeID, task.ModelID, fmt.Sprintf("tasks[%d]", index))
		if err != nil {
			return nil, err
		}
		if assignment.runtimeID.Valid {
			if _, err := h.Queries.GetAgentRuntimeForWorkspace(ctx, db.GetAgentRuntimeForWorkspaceParams{ID: assignment.runtimeID, WorkspaceID: workspaceID}); err != nil {
				return nil, dependencyGraphInvalidReference("invalid_runtime", "runtime is not in this workspace")
			}
		}
		assignments[index] = assignment
	}
	return assignments, nil
}

type dependencyGraphRootIssue struct {
	issue      db.Issue
	assignment dependencyGraphTaskAssignment
}

type dependencyGraphApplyResult struct {
	plan   db.DependencyGraphPlan
	issues []db.Issue
	nodes  []db.DependencyGraphNode
	roots  []dependencyGraphRootIssue
}

func (h *Handler) applyDependencyGraphTransaction(ctx context.Context, input *dependencyGraphApplyInput, waves [][]string, assignments []dependencyGraphTaskAssignment, parent db.Issue, actor dependencyGraphActor, policy service.IssueCountPolicy, idempotencyKey, requestHash string) (dependencyGraphApplyResult, error) {
	if h.TxStarter == nil {
		return dependencyGraphApplyResult{}, dependencyGraphDatabase("dependency graph transactions are unavailable", nil)
	}
	tx, err := h.TxStarter.Begin(ctx)
	if err != nil {
		return dependencyGraphApplyResult{}, dependencyGraphDatabase("begin dependency graph apply transaction", err)
	}
	defer func() { _ = tx.Rollback(ctx) }()
	var lockedID, lockedWorkspaceID, lockedProjectID pgtype.UUID
	err = tx.QueryRow(ctx, `
		SELECT id, workspace_id, project_id
		FROM issue
		WHERE id = $1 AND workspace_id = $2
		FOR UPDATE`, parent.ID, actor.WorkspaceID).Scan(&lockedID, &lockedWorkspaceID, &lockedProjectID)
	if err != nil {
		if isNotFound(err) {
			return dependencyGraphApplyResult{}, dependencyGraphNotFound("parent issue not found")
		}
		return dependencyGraphApplyResult{}, dependencyGraphDatabase("lock dependency graph parent issue", err)
	}
	if lockedID != parent.ID || lockedWorkspaceID != actor.WorkspaceID {
		return dependencyGraphApplyResult{}, dependencyGraphIntegrity("locked dependency graph parent issue crossed workspace boundary")
	}
	qtx := h.Queries.WithTx(tx)
	active, err := qtx.GetActiveDependencyGraphPlanForParent(ctx, db.GetActiveDependencyGraphPlanForParentParams{WorkspaceID: actor.WorkspaceID, ParentIssueID: lockedID})
	if err == nil {
		return dependencyGraphApplyResult{}, dependencyGraphConflict("active_plan_exists", fmt.Sprintf("an active dependency graph already exists for parent issue %s", uuidToString(active.ParentIssueID)))
	}
	if !isNotFound(err) {
		return dependencyGraphApplyResult{}, dependencyGraphDatabase("check active dependency graph for parent", err)
	}
	// The preflight checks above provide early, role-specific errors. Recheck
	// every persisted actor and runtime through the transaction-bound query set
	// so a concurrent archive/delete cannot leave a graph pointing at a stale
	// workspace object, matching the Rust service's atomic validation boundary.
	for index, task := range input.Tasks {
		if task.Owner != nil {
			if err := validateDependencyGraphActor(ctx, qtx, actor.WorkspaceID, *task.Owner, "invalid_owner"); err != nil {
				return dependencyGraphApplyResult{}, err
			}
		}
		if task.Executor != nil {
			if err := validateDependencyGraphActor(ctx, qtx, actor.WorkspaceID, *task.Executor, "invalid_executor"); err != nil {
				return dependencyGraphApplyResult{}, err
			}
		}
		if task.Reviewer != nil {
			if err := validateDependencyGraphActor(ctx, qtx, actor.WorkspaceID, *task.Reviewer, "invalid_reviewer"); err != nil {
				return dependencyGraphApplyResult{}, err
			}
		}
		for _, candidate := range task.CandidateExecutors {
			if err := validateDependencyGraphActor(ctx, qtx, actor.WorkspaceID, candidate, "invalid_executor"); err != nil {
				return dependencyGraphApplyResult{}, err
			}
		}
		if assignments[index].runtimeID.Valid {
			if _, err := qtx.GetAgentRuntimeForWorkspace(ctx, db.GetAgentRuntimeForWorkspaceParams{ID: assignments[index].runtimeID, WorkspaceID: actor.WorkspaceID}); err != nil {
				return dependencyGraphApplyResult{}, dependencyGraphInvalidReference("invalid_runtime", "runtime is not in this workspace")
			}
		}
	}
	plan, err := qtx.CreateDependencyGraphPlan(ctx, db.CreateDependencyGraphPlanParams{
		WorkspaceID:    actor.WorkspaceID,
		ParentIssueID:  lockedID,
		IdempotencyKey: idempotencyKey,
		RequestHash:    requestHash,
		Goal:           input.Goal,
		CreatedByType:  actor.Type,
		CreatedByID:    actor.ID,
		Status:         pgtype.Text{String: "active", Valid: true},
		ID:             dbid.NewV7(),
	})
	if err != nil {
		return dependencyGraphApplyResult{}, dependencyGraphDatabase("create dependency graph plan", err)
	}
	waveByTempID := make(map[string]int, len(input.Tasks))
	for wave, tempIDs := range waves {
		for _, tempID := range tempIDs {
			waveByTempID[tempID] = wave
		}
	}
	issues := make([]db.Issue, 0, len(input.Tasks))
	nodes := make([]db.DependencyGraphNode, 0, len(input.Tasks))
	issueByTempID := make(map[string]pgtype.UUID, len(input.Tasks))
	for index, task := range input.Tasks {
		number, err := service.AllocateIssueNumber(ctx, qtx, actor.WorkspaceID, policy)
		if err != nil {
			return dependencyGraphApplyResult{}, err
		}
		position, err := issueposition.NextTopPosition(ctx, tx, actor.WorkspaceID, issuestatus.Todo)
		if err != nil {
			return dependencyGraphApplyResult{}, dependencyGraphDatabase("allocate dependency graph issue position", err)
		}
		assignment := assignments[index]
		issue, err := qtx.CreateIssue(ctx, db.CreateIssueParams{
			ID:            dbid.NewV7(),
			WorkspaceID:   actor.WorkspaceID,
			Title:         task.Title,
			Description:   strToText(task.Description),
			Status:        issuestatus.Todo,
			Priority:      "none",
			ExecutorType:  assignment.executorType,
			ExecutorID:    assignment.executorID,
			CreatorType:   actor.Type,
			CreatorID:     actor.ID,
			ParentIssueID: lockedID,
			Position:      position,
			Number:        number,
			ProjectID:     lockedProjectID,
			OwnerType:     assignment.ownerType,
			OwnerID:       assignment.ownerID,
			ReviewerType:  assignment.reviewerType,
			ReviewerID:    assignment.reviewerID,
		})
		if err != nil {
			return dependencyGraphApplyResult{}, dependencyGraphDatabase("create dependency graph issue", err)
		}
		acceptanceJSON, err := json.Marshal(task.AcceptanceCriteria)
		if err != nil {
			return dependencyGraphApplyResult{}, dependencyGraphDatabase("encode dependency graph acceptance criteria", err)
		}
		outputsJSON, err := json.Marshal(task.Outputs)
		if err != nil {
			return dependencyGraphApplyResult{}, dependencyGraphDatabase("encode dependency graph outputs", err)
		}
		node, err := qtx.CreateDependencyGraphNode(ctx, db.CreateDependencyGraphNodeParams{
			PlanID:       plan.ID,
			WorkspaceID:  actor.WorkspaceID,
			TempID:       task.TempID,
			IssueID:      issue.ID,
			Title:        task.Title,
			Column6:      task.Description,
			Column7:      acceptanceJSON,
			Column8:      task.Context,
			Column9:      outputsJSON,
			Column10:     assignment.candidateExecutors,
			Wave:         int32(waveByTempID[task.TempID]),
			ExecutorType: assignment.executorType,
			ExecutorID:   assignment.executorID,
			OwnerType:    assignment.ownerType,
			OwnerID:      assignment.ownerID,
			ReviewerType: assignment.reviewerType,
			ReviewerID:   assignment.reviewerID,
			RuntimeID:    assignment.runtimeID,
			ModelID:      assignment.modelID,
			Column20:     dbid.NewV7(),
		})
		if err != nil {
			return dependencyGraphApplyResult{}, dependencyGraphDatabase("create dependency graph node", err)
		}
		if _, err := tx.Exec(ctx, `
			INSERT INTO dependency_graph_issue_created_outbox
			    (plan_id, node_id, workspace_id, issue_id, status, attempt)
			VALUES ($1, $2, $3, $4, 'pending', 0)
			ON CONFLICT (plan_id, node_id) DO NOTHING`, plan.ID, node.ID, actor.WorkspaceID, issue.ID); err != nil {
			return dependencyGraphApplyResult{}, dependencyGraphDatabase("create dependency graph issue outbox row", err)
		}
		issues = append(issues, issue)
		nodes = append(nodes, node)
		issueByTempID[task.TempID] = issue.ID
	}
	for _, edge := range input.Edges {
		fromIssueID := issueByTempID[edge.From]
		toIssueID := issueByTempID[edge.To]
		if !fromIssueID.Valid || !toIssueID.Valid {
			return dependencyGraphApplyResult{}, dependencyGraphIntegrity("dependency graph edge references a missing created issue")
		}
		if _, err := qtx.CreateDependencyGraphEdge(ctx, db.CreateDependencyGraphEdgeParams{
			PlanID:         plan.ID,
			WorkspaceID:    actor.WorkspaceID,
			FromIssueID:    fromIssueID,
			ToIssueID:      toIssueID,
			Type:           edge.Type,
			Reason:         edge.Reason,
			ConsumedOutput: edge.ConsumedOutput,
			ID:             dbid.NewV7(),
		}); err != nil {
			return dependencyGraphApplyResult{}, dependencyGraphDatabase("create dependency graph edge", err)
		}
	}
	if err := tx.Commit(ctx); err != nil {
		return dependencyGraphApplyResult{}, dependencyGraphDatabase("commit dependency graph apply", err)
	}
	result := dependencyGraphApplyResult{plan: plan, issues: issues, nodes: nodes, roots: make([]dependencyGraphRootIssue, 0, len(issues))}
	for index, task := range input.Tasks {
		if waveByTempID[task.TempID] == 0 {
			result.roots = append(result.roots, dependencyGraphRootIssue{issue: issues[index], assignment: assignments[index]})
		}
	}
	return result, nil
}

func dependencyGraphIsActivePlanConflict(err error) bool {
	var graphErr *dependencyGraphError
	return errors.As(err, &graphErr) && graphErr.code == "active_plan_exists"
}

func (h *Handler) writeDependencyGraphReplay(w http.ResponseWriter, r *http.Request, plan db.DependencyGraphPlan) {
	response, err := h.dependencyGraphResponseForPlan(r.Context(), plan)
	if err != nil {
		writeDependencyGraphError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, dependencyGraphResponseMap(response, true, true))
}

func (h *Handler) resolveDependencyGraphApplyConflict(w http.ResponseWriter, r *http.Request, workspaceID, parentIssueID pgtype.UUID, idempotencyKey, requestHash string) bool {
	plan, err := h.Queries.GetDependencyGraphPlanByIdempotency(r.Context(), db.GetDependencyGraphPlanByIdempotencyParams{WorkspaceID: workspaceID, IdempotencyKey: idempotencyKey})
	if err == nil {
		if plan.RequestHash != requestHash {
			writeDependencyGraphError(w, dependencyGraphConflict("idempotency_conflict", "idempotency key was used for a different dependency graph"))
		} else {
			h.writeDependencyGraphReplay(w, r, plan)
		}
		return true
	}
	if !isNotFound(err) {
		writeDependencyGraphError(w, dependencyGraphDatabase("resolve dependency graph idempotency conflict", err))
		return true
	}
	active, err := h.Queries.GetActiveDependencyGraphPlanForParent(r.Context(), db.GetActiveDependencyGraphPlanForParentParams{WorkspaceID: workspaceID, ParentIssueID: parentIssueID})
	if err == nil {
		writeDependencyGraphError(w, dependencyGraphConflict("active_plan_exists", fmt.Sprintf("an active dependency graph already exists for parent issue %s", uuidToString(active.ParentIssueID))))
		return true
	}
	if !isNotFound(err) {
		writeDependencyGraphError(w, dependencyGraphDatabase("resolve active dependency graph conflict", err))
		return true
	}
	return false
}

func (h *Handler) publishDependencyGraphIssues(ctx context.Context, result dependencyGraphApplyResult, actor dependencyGraphActor) {
	if h.Bus == nil || len(result.issues) != len(result.nodes) {
		return
	}
	prefix := h.getIssuePrefix(ctx, actor.WorkspaceID)
	for index, issue := range result.issues {
		response := issueToResponse(issue, prefix)
		h.fillStatusCategory(ctx, issue.WorkspaceID, &response)
		h.publish(protocol.EventIssueCreated, uuidToString(actor.WorkspaceID), actor.Type, uuidToString(actor.ID), map[string]any{
			"issue":                    response,
			"dependency_graph_plan_id": uuidToString(result.plan.ID),
			"dependency_graph_node_id": uuidToString(result.nodes[index].ID),
		})
		if h.DB == nil {
			continue
		}
		if _, err := h.DB.Exec(ctx, `
			UPDATE dependency_graph_issue_created_outbox
			SET status = 'published', published_at = now(), updated_at = now()
			WHERE plan_id = $1 AND node_id = $2 AND workspace_id = $3 AND status = 'pending'`, result.plan.ID, result.nodes[index].ID, actor.WorkspaceID); err != nil {
			slog.WarnContext(ctx, "dependency graph issue-created outbox update failed", "plan_id", uuidToString(result.plan.ID), "node_id", uuidToString(result.nodes[index].ID), "error", err)
		}
	}
}

func (h *Handler) enqueueDependencyGraphRoots(ctx context.Context, result dependencyGraphApplyResult) {
	if h.TaskService == nil || h.Queries == nil {
		return
	}
	for _, root := range result.roots {
		if !root.assignment.executorType.Valid || !root.assignment.executorID.Valid {
			continue
		}
		switch root.assignment.executorType.String {
		case "agent":
			if !h.shouldEnqueueAgentTask(ctx, root.issue) {
				continue
			}
			if _, err := h.TaskService.EnqueueTaskForIssue(ctx, root.issue); err != nil {
				slog.WarnContext(ctx, "dependency graph root task enqueue failed", "issue_id", uuidToString(root.issue.ID), "error", err)
			}
		case "team":
			team, err := h.Queries.GetTeamInWorkspace(ctx, db.GetTeamInWorkspaceParams{ID: root.assignment.executorID, WorkspaceID: root.issue.WorkspaceID})
			if err != nil || team.ArchivedAt.Valid {
				continue
			}
			if _, err := h.TaskService.EnqueueTaskForTeamLeader(ctx, root.issue, team.LeaderID, team.ID, pgtype.UUID{}); err != nil {
				slog.WarnContext(ctx, "dependency graph team root task enqueue failed", "issue_id", uuidToString(root.issue.ID), "team_id", uuidToString(team.ID), "error", err)
			}
		}
	}
}

func (h *Handler) ListDependencyGraphs(w http.ResponseWriter, r *http.Request) {
	wsID := h.resolveWorkspaceID(r)
	actor, ok := h.dependencyGraphActorForRequest(w, r, wsID)
	if !ok {
		return
	}
	wsUUID := actor.WorkspaceID
	q := r.URL.Query()
	var projectID *pgtype.UUID
	if v := strings.TrimSpace(q.Get("project_id")); v != "" {
		u, ok := parseUUIDOrBadRequest(w, v, "project id")
		if !ok {
			return
		}
		projectID = &u
	}
	limit := dependencyGraphDefaultPageSize
	if v := strings.TrimSpace(q.Get("limit")); v != "" {
		parsed, err := strconv.Atoi(v)
		if err != nil || parsed <= 0 || parsed > dependencyGraphMaxPageSize {
			writeDependencyGraphError(w, invalidDependencyGraph(fmt.Sprintf("limit must be between 1 and %d", dependencyGraphMaxPageSize)))
			return
		}
		limit = parsed
	}
	after, err := decodeDependencyGraphCursor(q.Get("cursor"), projectID)
	if err != nil {
		writeDependencyGraphError(w, err)
		return
	}
	offset := 0
	if after != nil {
		offset = after.offset
	}
	if int64(offset) > int64(1<<31-1-limit-1) {
		writeDependencyGraphError(w, invalidDependencyGraph("dependency graph cursor is too far ahead"))
		return
	}
	filterProject := pgtype.UUID{}
	if projectID != nil {
		filterProject = *projectID
	}
	plans, err := h.Queries.ListDependencyGraphPlans(r.Context(), db.ListDependencyGraphPlansParams{
		WorkspaceID: wsUUID,
		Column2:     filterProject,
		Limit:       int32(limit + 1),
		Offset:      int32(offset),
	})
	if err != nil {
		writeDependencyGraphError(w, dependencyGraphDatabase("list dependency graph plans", err))
		return
	}
	hasMore := len(plans) > limit
	if hasMore {
		plans = plans[:limit]
	}
	graphs := make([]map[string]any, 0, len(plans))
	for _, plan := range plans {
		response, err := h.dependencyGraphResponseForPlan(r.Context(), plan)
		if err != nil {
			writeDependencyGraphError(w, err)
			return
		}
		graphs = append(graphs, dependencyGraphResponseMap(response, false, false))
	}
	var nextCursor *string
	if hasMore && len(plans) > 0 {
		encoded, err := encodeDependencyGraphCursorAt(projectID, plans[len(plans)-1], offset+limit)
		if err != nil {
			writeDependencyGraphError(w, dependencyGraphDatabase("encode dependency graph cursor", err))
			return
		}
		nextCursor = &encoded
	}
	writeJSON(w, http.StatusOK, map[string]any{"graphs": graphs, "next_cursor": nextCursor})
}

func (h *Handler) GetDependencyGraphByID(w http.ResponseWriter, r *http.Request) {
	wsID := h.resolveWorkspaceID(r)
	actor, ok := h.dependencyGraphActorForRequest(w, r, wsID)
	if !ok {
		return
	}
	planID := chi.URLParam(r, "id")
	planUUID, ok := parseUUIDOrBadRequest(w, planID, "dependency graph id")
	if !ok {
		return
	}
	plan, err := h.Queries.GetDependencyGraphPlanByID(r.Context(), db.GetDependencyGraphPlanByIDParams{ID: planUUID, WorkspaceID: actor.WorkspaceID})
	if err != nil {
		if isNotFound(err) {
			writeDependencyGraphError(w, dependencyGraphNotFound("dependency graph not found"))
			return
		}
		writeDependencyGraphError(w, dependencyGraphDatabase("get dependency graph plan", err))
		return
	}
	response, err := h.dependencyGraphResponseForPlan(r.Context(), plan)
	if err != nil {
		writeDependencyGraphError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, dependencyGraphResponseMap(response, false, false))
}

func (h *Handler) GetIssueDependencyGraph(w http.ResponseWriter, r *http.Request) {
	wsID := h.resolveWorkspaceID(r)
	actor, ok := h.dependencyGraphActorForRequest(w, r, wsID)
	if !ok {
		return
	}
	issueID := chi.URLParam(r, "id")
	issueUUID, ok := parseUUIDOrBadRequest(w, issueID, "issue id")
	if !ok {
		return
	}
	if _, err := h.Queries.GetIssueInWorkspace(r.Context(), db.GetIssueInWorkspaceParams{ID: issueUUID, WorkspaceID: actor.WorkspaceID}); err != nil {
		if isNotFound(err) {
			writeDependencyGraphError(w, dependencyGraphNotFound("issue not found"))
			return
		}
		writeDependencyGraphError(w, dependencyGraphDatabase("get issue for dependency graph", err))
		return
	}
	plan, err := h.Queries.GetActiveDependencyGraphForIssue(r.Context(), db.GetActiveDependencyGraphForIssueParams{
		WorkspaceID:   actor.WorkspaceID,
		ParentIssueID: issueUUID,
	})
	if err != nil {
		if isNotFound(err) {
			writeJSON(w, http.StatusOK, map[string]any{"plan": nil, "nodes": []dependencyGraphNodeResponse{}, "edges": []dependencyGraphEdgeResponse{}})
			return
		}
		writeDependencyGraphError(w, dependencyGraphDatabase("get active dependency graph", err))
		return
	}
	response, err := h.dependencyGraphResponseForPlan(r.Context(), plan)
	if err != nil {
		writeDependencyGraphError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, dependencyGraphResponseMap(response, false, false))
}

func (h *Handler) ApplyIssueDependencyGraph(w http.ResponseWriter, r *http.Request) {
	wsID := h.resolveWorkspaceID(r)
	actor, ok := h.dependencyGraphActorForRequest(w, r, wsID)
	if !ok {
		return
	}
	parentIssueID := chi.URLParam(r, "id")
	parentUUID, ok := parseUUIDOrBadRequest(w, parentIssueID, "parent issue id")
	if !ok {
		return
	}
	idempotencyKey, err := dependencyGraphIdempotencyKey(r)
	if err != nil {
		writeDependencyGraphError(w, err)
		return
	}
	if r.Body == nil {
		writeDependencyGraphError(w, invalidDependencyGraph("request body is required"))
		return
	}
	decoder := json.NewDecoder(r.Body)
	decoder.DisallowUnknownFields()
	var input dependencyGraphApplyInput
	if err := decoder.Decode(&input); err != nil {
		writeDependencyGraphError(w, invalidDependencyGraph("request body must be valid dependency graph JSON"))
		return
	}
	var trailing any
	if err := decoder.Decode(&trailing); err != io.EOF {
		writeDependencyGraphError(w, invalidDependencyGraph("request body must contain exactly one JSON value"))
		return
	}
	if input.ParentIssueID != "" {
		bodyParent, err := util.ParseUUID(input.ParentIssueID)
		if err != nil || !bodyParent.Valid {
			writeDependencyGraphError(w, invalidDependencyGraph("parent_issue_id must be a non-nil UUID"))
			return
		}
		if bodyParent != parentUUID {
			writeDependencyGraphError(w, dependencyGraphConflict("parent_mismatch", "parent_issue_id must match the issue in the request path"))
			return
		}
	}
	input.ParentIssueID = uuidToString(parentUUID)
	waves, err := validateDependencyGraphPlan(&input)
	if err != nil {
		writeDependencyGraphError(w, err)
		return
	}
	parent, err := h.Queries.GetIssueInWorkspace(r.Context(), db.GetIssueInWorkspaceParams{ID: parentUUID, WorkspaceID: actor.WorkspaceID})
	if err != nil {
		if isNotFound(err) {
			writeDependencyGraphError(w, dependencyGraphNotFound("parent issue not found"))
			return
		}
		writeDependencyGraphError(w, dependencyGraphDatabase("get dependency graph parent issue", err))
		return
	}
	assignments, err := h.validateDependencyGraphAssignments(r.Context(), r, actor.WorkspaceID, &parent, &input)
	if err != nil {
		writeDependencyGraphError(w, err)
		return
	}
	requestHash, err := dependencyGraphRequestHash(&input)
	if err != nil {
		writeDependencyGraphError(w, dependencyGraphDatabase("hash dependency graph request", err))
		return
	}
	existing, err := h.Queries.GetDependencyGraphPlanByIdempotency(r.Context(), db.GetDependencyGraphPlanByIdempotencyParams{WorkspaceID: actor.WorkspaceID, IdempotencyKey: idempotencyKey})
	if err == nil {
		if existing.RequestHash != requestHash {
			writeDependencyGraphError(w, dependencyGraphConflict("idempotency_conflict", "idempotency key was used for a different dependency graph"))
			return
		}
		h.writeDependencyGraphReplay(w, r, existing)
		return
	}
	if !isNotFound(err) {
		writeDependencyGraphError(w, dependencyGraphDatabase("check dependency graph idempotency", err))
		return
	}
	policy := service.ResolveIssueCountPolicy(r.Context(), h.Entitlements, actor.WorkspaceID)
	result, err := h.applyDependencyGraphTransaction(r.Context(), &input, waves, assignments, parent, actor, policy, idempotencyKey, requestHash)
	if err != nil {
		if isUniqueViolation(err) || dependencyGraphIsActivePlanConflict(err) {
			if h.resolveDependencyGraphApplyConflict(w, r, actor.WorkspaceID, parentUUID, idempotencyKey, requestHash) {
				return
			}
		}
		if writeIssueLimitReached(w, err) {
			return
		}
		writeDependencyGraphError(w, err)
		return
	}
	h.publishDependencyGraphIssues(r.Context(), result, actor)
	h.enqueueDependencyGraphRoots(r.Context(), result)
	response, err := h.dependencyGraphResponseForPlan(r.Context(), result.plan)
	if err != nil {
		writeDependencyGraphError(w, err)
		return
	}
	writeJSON(w, http.StatusCreated, dependencyGraphResponseMap(response, true, false))
}

func (h *Handler) RetireDependencyGraph(w http.ResponseWriter, r *http.Request) {
	wsID := h.resolveWorkspaceID(r)
	actor, ok := h.dependencyGraphActorForRequest(w, r, wsID)
	if !ok {
		return
	}
	planID := chi.URLParam(r, "id")
	planUUID, ok := parseUUIDOrBadRequest(w, planID, "dependency graph id")
	if !ok {
		return
	}
	if h.TxStarter == nil {
		writeDependencyGraphError(w, dependencyGraphDatabase("dependency graph transactions are unavailable", nil))
		return
	}
	tx, err := h.TxStarter.Begin(r.Context())
	if err != nil {
		writeDependencyGraphError(w, dependencyGraphDatabase("begin dependency graph retire transaction", err))
		return
	}
	defer func() { _ = tx.Rollback(r.Context()) }()
	qtx := h.Queries.WithTx(tx)
	plan, err := qtx.GetDependencyGraphPlanForUpdate(r.Context(), db.GetDependencyGraphPlanForUpdateParams{ID: planUUID, WorkspaceID: actor.WorkspaceID})
	if err != nil {
		if isNotFound(err) {
			writeDependencyGraphError(w, dependencyGraphNotFound("dependency graph not found"))
			return
		}
		writeDependencyGraphError(w, dependencyGraphDatabase("lock dependency graph plan", err))
		return
	}
	if plan.Status != "active" {
		writeDependencyGraphError(w, dependencyGraphConflict("plan_not_active", "dependency graph plan is not active"))
		return
	}
	updated, err := qtx.UpdateDependencyGraphPlanStatus(r.Context(), db.UpdateDependencyGraphPlanStatusParams{ID: planUUID, WorkspaceID: actor.WorkspaceID, Status: "cancelled"})
	if err != nil {
		writeDependencyGraphError(w, dependencyGraphDatabase("retire dependency graph plan", err))
		return
	}
	if err := tx.Commit(r.Context()); err != nil {
		writeDependencyGraphError(w, dependencyGraphDatabase("commit dependency graph retire", err))
		return
	}
	response, err := h.dependencyGraphResponseForPlan(r.Context(), updated)
	if err != nil {
		writeDependencyGraphError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, dependencyGraphResponseMap(response, false, false))
}
