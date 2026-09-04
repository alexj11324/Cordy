// Package linear contains the provider boundary for the Linear integration.
//
// Keeping OAuth, GraphQL response validation, pagination, marker handling, and
// provider error classification here makes the handler and sync worker operate
// on a small, typed contract instead of reimplementing Linear semantics at
// every call site.
package linear

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"
)

const (
	DefaultAuthorizeURL = "https://linear.app/oauth/authorize"
	DefaultGraphQLURL   = "https://api.linear.app/graphql"
	DefaultTokenURL     = "https://api.linear.app/oauth/token"
	DefaultRevokeURL    = "https://api.linear.app/oauth/revoke"
	OAuthScope          = "read,write,issues:create,app:assignable"
	patchbayMarker      = "[patchbay:issue="
	maxResponseBytes    = 4 << 20
	maxIssuePages       = 500
	maxImportIssues     = 50_000
	maxPreviewIssues    = 10_000
)

type ErrorKind string

const (
	ErrorProvider          ErrorKind = "provider"
	ErrorInvalidResponse   ErrorKind = "invalid_response"
	ErrorInvalidGrant      ErrorKind = "invalid_grant"
	ErrorRateLimited       ErrorKind = "rate_limited"
	ErrorMutationRejected  ErrorKind = "mutation_rejected"
)

type ProviderError struct {
	Kind    ErrorKind
	Status  int
	Message string
}

func (e *ProviderError) Error() string {
	if e == nil {
		return ""
	}
	if e.Status > 0 {
		return fmt.Sprintf("linear %s (%d): %s", e.Kind, e.Status, e.Message)
	}
	return fmt.Sprintf("linear %s: %s", e.Kind, e.Message)
}

func IsKind(err error, kind ErrorKind) bool {
	var providerErr *ProviderError
	return errors.As(err, &providerErr) && providerErr.Kind == kind
}

type Issue struct {
	ID, Identifier, Title, Description, StateID, StateType, ProjectID, TeamID string
	AssigneeID                                                               string
	DueDate                                                                  *string
	Priority                                                                 int
	UpdatedAt                                                                time.Time
	Deleted                                                                  bool
}

type IssueInput struct {
	TeamID, ProjectID, Title, Description, StateID string
	AssigneeID                                     *string
	DueDate                                        *string
	PatchbayIssueID                                string
	Priority                                       int
	ClearAssignee                                  bool
}

type Token struct {
	AccessToken, RefreshToken, Scope string
	ExpiresIn                        time.Duration
}

type Identity struct {
	ID, Name, OrganizationID, OrganizationName, ActorID string
}

type CatalogTeam struct {
	ID, Name, Key, OrganizationID string
}

type CatalogProject struct {
	ID, Name, TeamID string
}

type CatalogState struct {
	ID, Name, Type, TeamID, Color string
}

type CatalogUser struct {
	ID, Name, Email string
	Active          bool
}

type CatalogLabel struct {
	ID, Name, Color, ParentID, TeamID string
	IsGroup                bool
}

type Catalog struct {
	Teams          []CatalogTeam
	ProjectCatalog []CatalogProject
	States         []CatalogState
	Users          []CatalogUser
	Labels         []CatalogLabel
}

// DryRunCounts is intentionally bounded. A preview must not turn a malformed
// or unexpectedly large provider project into an unbounded request.
type DryRunCounts struct {
	RemoteIssues      int
	UnmappedStatuses  int
	Truncated         bool
}

type API interface {
	ExchangeAuthorizationCode(context.Context, string, string, string, string, string) (Token, error)
	RefreshToken(context.Context, string, string, string) (Token, error)
	RevokeToken(context.Context, string, string, string) error
	DiscoverIdentity(context.Context, string) (Identity, error)
	Catalog(context.Context, string) (Catalog, error)
	ValidateBinding(context.Context, string, string, string) error
	DryRunCounts(context.Context, string, string, string, map[string]any) (DryRunCounts, error)
	FetchIssue(context.Context, string, string) (Issue, bool, error)
	ListIssues(context.Context, string, string, string) ([]Issue, error)
	CreateIssue(context.Context, string, IssueInput) (Issue, error)
	UpdateIssue(context.Context, string, string, IssueInput) (Issue, error)
	DeleteIssue(context.Context, string, string) error
}

type HTTPClient struct {
	HTTP                                              *http.Client
	AuthorizeURL, GraphQLURL, TokenURL, RevokeURL     string
}

func NewHTTPClient(client *http.Client) *HTTPClient {
	if client == nil {
		client = &http.Client{Timeout: 30 * time.Second}
	}
	return &HTTPClient{
		HTTP: client, AuthorizeURL: DefaultAuthorizeURL, GraphQLURL: DefaultGraphQLURL,
		TokenURL: DefaultTokenURL, RevokeURL: DefaultRevokeURL,
	}
}

func endpoint(value, fallback string) string {
	if strings.TrimSpace(value) != "" {
		return value
	}
	return fallback
}

func boundedBody(body io.Reader) io.Reader {
	return io.LimitReader(body, maxResponseBytes)
}

func responseText(resp *http.Response) string {
	body, _ := io.ReadAll(boundedBody(resp.Body))
	return strings.TrimSpace(string(body))
}

func classifyHTTPError(status int, body string) error {
	kind := ErrorProvider
	lower := strings.ToLower(body)
	if status == http.StatusTooManyRequests {
		kind = ErrorRateLimited
	}
	if strings.Contains(lower, "invalid_grant") || strings.Contains(lower, "invalid grant") {
		kind = ErrorInvalidGrant
	}
	return &ProviderError{Kind: kind, Status: status, Message: body}
}

func (c *HTTPClient) postForm(ctx context.Context, ep string, values url.Values, out any) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, ep, strings.NewReader(values.Encode()))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	resp, err := c.HTTP.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode/100 != 2 {
		return classifyHTTPError(resp.StatusCode, responseText(resp))
	}
	if out == nil {
		return nil
	}
	if err := json.NewDecoder(boundedBody(resp.Body)).Decode(out); err != nil {
		return &ProviderError{Kind: ErrorInvalidResponse, Status: resp.StatusCode, Message: err.Error()}
	}
	return nil
}

type tokenResponse struct {
	AccessToken  string          `json:"access_token"`
	RefreshToken string          `json:"refresh_token"`
	Scope        json.RawMessage `json:"scope"`
	ExpiresIn    int64           `json:"expires_in"`
}

func parseScopes(raw json.RawMessage) string {
	if len(raw) == 0 || string(raw) == "null" {
		return ""
	}
	var value string
	if json.Unmarshal(raw, &value) == nil {
		return strings.TrimSpace(value)
	}
	var values []string
	if json.Unmarshal(raw, &values) == nil {
		return strings.Join(values, " ")
	}
	return ""
}

func strictToken(raw tokenResponse, fallbackRefresh string) (Token, error) {
	if strings.TrimSpace(raw.AccessToken) == "" || raw.ExpiresIn <= 0 {
		return Token{}, &ProviderError{Kind: ErrorInvalidResponse, Message: "token response must contain access_token and positive expires_in"}
	}
	refresh := strings.TrimSpace(raw.RefreshToken)
	if refresh == "" {
		refresh = strings.TrimSpace(fallbackRefresh)
	}
	if refresh == "" {
		return Token{}, &ProviderError{Kind: ErrorInvalidResponse, Message: "token response omitted refresh_token"}
	}
	return Token{AccessToken: raw.AccessToken, RefreshToken: refresh, Scope: parseScopes(raw.Scope), ExpiresIn: time.Duration(raw.ExpiresIn) * time.Second}, nil
}

func (c *HTTPClient) ExchangeAuthorizationCode(ctx context.Context, code, redirectURI, verifier, clientID, clientSecret string) (Token, error) {
	var raw tokenResponse
	err := c.postForm(ctx, endpoint(c.TokenURL, DefaultTokenURL), url.Values{
		"grant_type": {"authorization_code"}, "code": {code}, "redirect_uri": {redirectURI},
		"client_id": {clientID}, "client_secret": {clientSecret}, "code_verifier": {verifier},
	}, &raw)
	if err != nil {
		return Token{}, err
	}
	return strictToken(raw, "")
}

func (c *HTTPClient) RefreshToken(ctx context.Context, refresh, clientID, clientSecret string) (Token, error) {
	var raw tokenResponse
	err := c.postForm(ctx, endpoint(c.TokenURL, DefaultTokenURL), url.Values{
		"grant_type": {"refresh_token"}, "refresh_token": {refresh},
		"client_id": {clientID}, "client_secret": {clientSecret},
	}, &raw)
	if err != nil {
		return Token{}, err
	}
	return strictToken(raw, refresh)
}

func (c *HTTPClient) RevokeToken(ctx context.Context, token, clientID, clientSecret string) error {
	return c.postForm(ctx, endpoint(c.RevokeURL, DefaultRevokeURL), url.Values{
		"token": {token}, "client_id": {clientID}, "client_secret": {clientSecret},
	}, nil)
}

func (c *HTTPClient) graphql(ctx context.Context, token, query string, variables map[string]any, out any) error {
	payload, err := json.Marshal(map[string]any{"query": query, "variables": variables})
	if err != nil {
		return err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, endpoint(c.GraphQLURL, DefaultGraphQLURL), bytes.NewReader(payload))
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", "Bearer "+strings.TrimSpace(token))
	req.Header.Set("Content-Type", "application/json")
	resp, err := c.HTTP.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode/100 != 2 {
		return classifyHTTPError(resp.StatusCode, responseText(resp))
	}
	var env struct {
		Data   json.RawMessage `json:"data"`
		Errors []struct {
			Message string `json:"message"`
		} `json:"errors"`
	}
	if err := json.NewDecoder(boundedBody(resp.Body)).Decode(&env); err != nil {
		return &ProviderError{Kind: ErrorInvalidResponse, Status: resp.StatusCode, Message: err.Error()}
	}
	if len(env.Errors) > 0 {
		return &ProviderError{Kind: ErrorProvider, Status: resp.StatusCode, Message: env.Errors[0].Message}
	}
	if len(env.Data) == 0 || string(env.Data) == "null" {
		return &ProviderError{Kind: ErrorInvalidResponse, Status: resp.StatusCode, Message: "GraphQL response omitted data"}
	}
	if err := json.Unmarshal(env.Data, out); err != nil {
		return &ProviderError{Kind: ErrorInvalidResponse, Status: resp.StatusCode, Message: err.Error()}
	}
	return nil
}

type issueNode struct {
	ID          string    `json:"id"`
	Identifier  string    `json:"identifier"`
	Title       string    `json:"title"`
	Description *string   `json:"description"`
	DueDate     *string   `json:"dueDate"`
	Priority    int       `json:"priority"`
	UpdatedAt   time.Time `json:"updatedAt"`
	State       *struct {
		ID   string `json:"id"`
		Type string `json:"type"`
	} `json:"state"`
	Project *struct {
		ID string `json:"id"`
	} `json:"project"`
	Team *struct {
		ID string `json:"id"`
	} `json:"team"`
	Assignee *struct {
		ID string `json:"id"`
	} `json:"assignee"`
}

func issueFromNode(n issueNode) (Issue, error) {
	if strings.TrimSpace(n.ID) == "" || strings.TrimSpace(n.Identifier) == "" || n.UpdatedAt.IsZero() {
		return Issue{}, &ProviderError{Kind: ErrorInvalidResponse, Message: "Linear issue omitted id, identifier, or updatedAt"}
	}
	o := Issue{ID: n.ID, Identifier: n.Identifier, Title: n.Title, Priority: n.Priority, UpdatedAt: n.UpdatedAt, DueDate: n.DueDate}
	if n.Description != nil {
		o.Description = *n.Description
	}
	if n.State != nil {
		o.StateID, o.StateType = n.State.ID, n.State.Type
	}
	if n.Project != nil {
		o.ProjectID = n.Project.ID
	}
	if n.Team != nil {
		o.TeamID = n.Team.ID
	}
	if n.Assignee != nil {
		o.AssigneeID = n.Assignee.ID
	}
	return o, nil
}

const issueFields = `id identifier title description dueDate priority updatedAt state{id type} project{id} team{id} assignee{id}`

func (c *HTTPClient) DiscoverIdentity(ctx context.Context, token string) (Identity, error) {
	var data struct {
		Viewer struct {
			ID   string `json:"id"`
			Name string `json:"name"`
		} `json:"viewer"`
		Organization struct {
			ID   string `json:"id"`
			Name string `json:"name"`
		} `json:"organization"`
	}
	err := c.graphql(ctx, token, `query PatchbayIdentity{viewer{id name} organization{id name}}`, nil, &data)
	if err != nil {
		return Identity{}, err
	}
	if data.Viewer.ID == "" || data.Organization.ID == "" {
		return Identity{}, &ProviderError{Kind: ErrorInvalidResponse, Message: "Linear identity omitted viewer or organization"}
	}
	return Identity{ID: data.Viewer.ID, Name: data.Viewer.Name, OrganizationID: data.Organization.ID, OrganizationName: data.Organization.Name, ActorID: data.Viewer.ID}, nil
}

func (c *HTTPClient) FetchIssue(ctx context.Context, token, issueID string) (Issue, bool, error) {
	var data struct {
		Issue *issueNode `json:"issue"`
	}
	if err := c.graphql(ctx, token, `query PatchbayIssue($id:ID!){issue(id:$id){`+issueFields+`}}`, map[string]any{"id": issueID}, &data); err != nil {
		return Issue{}, false, err
	}
	if data.Issue == nil {
		return Issue{}, false, nil
	}
	issue, err := issueFromNode(*data.Issue)
	return issue, true, err
}

func (c *HTTPClient) ListIssues(ctx context.Context, token, projectID, teamID string) ([]Issue, error) {
	const q = `query PatchbayIssues($project:ID!,$after:String){issues(first:100,after:$after,filter:{project:{id:{eq:$project}}}){nodes{` + issueFields + `}pageInfo{hasNextPage endCursor}}}`
	var all []Issue
	var after any
	var previous string
	for page := 0; ; page++ {
		if page >= maxIssuePages {
			return nil, &ProviderError{Kind: ErrorInvalidResponse, Message: "Linear issue pagination exceeded safety limit"}
		}
		var data struct {
			Issues struct {
				Nodes    []issueNode `json:"nodes"`
				PageInfo struct {
					HasNext bool    `json:"hasNextPage"`
					End    *string `json:"endCursor"`
				} `json:"pageInfo"`
			} `json:"issues"`
		}
		if err := c.graphql(ctx, token, q, map[string]any{"project": projectID, "after": after}, &data); err != nil {
			return nil, err
		}
		for _, n := range data.Issues.Nodes {
			i, err := issueFromNode(n)
			if err != nil {
				return nil, err
			}
			if teamID == "" || i.TeamID == teamID {
				all = append(all, i)
				if len(all) > maxImportIssues {
					return nil, &ProviderError{Kind: ErrorInvalidResponse, Message: "Linear issue import exceeded safety limit"}
				}
			}
		}
		if !data.Issues.PageInfo.HasNext {
			break
		}
		if data.Issues.PageInfo.End == nil || strings.TrimSpace(*data.Issues.PageInfo.End) == "" || *data.Issues.PageInfo.End == previous {
			return nil, &ProviderError{Kind: ErrorInvalidResponse, Message: "Linear pagination returned an invalid cursor"}
		}
		previous, after = *data.Issues.PageInfo.End, *data.Issues.PageInfo.End
	}
	return all, nil
}

type pageInfo struct {
	HasNext bool    `json:"hasNextPage"`
	End     *string `json:"endCursor"`
}

func nextCatalogCursor(previous *string, info pageInfo) (*string, error) {
	if !info.HasNext { return nil, nil }
	if info.End == nil || strings.TrimSpace(*info.End) == "" || (previous != nil && *previous == *info.End) {
		return nil, &ProviderError{Kind: ErrorInvalidResponse, Message: "Linear catalog returned an invalid cursor"}
	}
	return info.End, nil
}

func (c *HTTPClient) Catalog(ctx context.Context, token string) (Catalog, error) {
	var out Catalog
	var after *string
	for page := 0; ; page++ {
		if page >= maxIssuePages { return Catalog{}, &ProviderError{Kind: ErrorInvalidResponse, Message: "Linear team pagination exceeded safety limit"} }
		var data struct { Teams struct { Nodes []struct { ID, Name, Key string; Organization struct { ID string `json:"id"` } `json:"organization"` }; PageInfo pageInfo `json:"pageInfo"` } `json:"teams"` }
		if err := c.graphql(ctx, token, `query PatchbayCatalogTeams($after:String){teams(first:250,after:$after){nodes{id name key organization{id}}pageInfo{hasNextPage endCursor}}}`, map[string]any{"after": after}, &data); err != nil { return Catalog{}, err }
		for _, team := range data.Teams.Nodes { out.Teams = append(out.Teams, CatalogTeam{ID: team.ID, Name: team.Name, Key: team.Key, OrganizationID: team.Organization.ID}) }
		next, err := nextCatalogCursor(after, data.Teams.PageInfo); if err != nil { return Catalog{}, err }; if next == nil { break }; after = next
	}
	after = nil
	for page := 0; ; page++ {
		if page >= maxIssuePages { return Catalog{}, &ProviderError{Kind: ErrorInvalidResponse, Message: "Linear project pagination exceeded safety limit"} }
		var data struct { Projects struct { Nodes []struct { ID, Name string; Teams []struct { ID string `json:"id"` } `json:"teams"` }; PageInfo pageInfo `json:"pageInfo"` } `json:"projects"` }
		if err := c.graphql(ctx, token, `query PatchbayCatalogProjects($after:String){projects(first:250,after:$after){nodes{id name teams{id}}pageInfo{hasNextPage endCursor}}}`, map[string]any{"after": after}, &data); err != nil { return Catalog{}, err }
		for _, project := range data.Projects.Nodes { if len(project.Teams) == 0 { out.ProjectCatalog = append(out.ProjectCatalog, CatalogProject{ID: project.ID, Name: project.Name}) }; for _, team := range project.Teams { out.ProjectCatalog = append(out.ProjectCatalog, CatalogProject{ID: project.ID, Name: project.Name, TeamID: team.ID}) } }
		next, err := nextCatalogCursor(after, data.Projects.PageInfo); if err != nil { return Catalog{}, err }; if next == nil { break }; after = next
	}
	after = nil
	for page := 0; ; page++ {
		if page >= maxIssuePages { return Catalog{}, &ProviderError{Kind: ErrorInvalidResponse, Message: "Linear workflow state pagination exceeded safety limit"} }
		var data struct { States struct { Nodes []struct { ID, Name, Type, Color string; Team struct { ID string `json:"id"` } `json:"team"` }; PageInfo pageInfo `json:"pageInfo"` } `json:"workflowStates"` }
		if err := c.graphql(ctx, token, `query PatchbayCatalogStates($after:String){workflowStates(first:250,after:$after){nodes{id name type color team{id}}pageInfo{hasNextPage endCursor}}}`, map[string]any{"after": after}, &data); err != nil { return Catalog{}, err }
		for _, state := range data.States.Nodes { out.States = append(out.States, CatalogState{ID: state.ID, Name: state.Name, Type: state.Type, TeamID: state.Team.ID, Color: state.Color}) }
		next, err := nextCatalogCursor(after, data.States.PageInfo); if err != nil { return Catalog{}, err }; if next == nil { break }; after = next
	}
	after = nil
	for page := 0; ; page++ {
		if page >= maxIssuePages { return Catalog{}, &ProviderError{Kind: ErrorInvalidResponse, Message: "Linear user pagination exceeded safety limit"} }
		var data struct { Users struct { Nodes []CatalogUser `json:"nodes"`; PageInfo pageInfo `json:"pageInfo"` } `json:"users"` }
		if err := c.graphql(ctx, token, `query PatchbayCatalogUsers($after:String){users(first:250,after:$after){nodes{id name email active}pageInfo{hasNextPage endCursor}}}`, map[string]any{"after": after}, &data); err != nil { return Catalog{}, err }
		out.Users = append(out.Users, data.Users.Nodes...)
		next, err := nextCatalogCursor(after, data.Users.PageInfo); if err != nil { return Catalog{}, err }; if next == nil { break }; after = next
	}
	after = nil
	for page := 0; ; page++ {
		if page >= maxIssuePages { return Catalog{}, &ProviderError{Kind: ErrorInvalidResponse, Message: "Linear label pagination exceeded safety limit"} }
		var data struct { Labels struct { Nodes []struct { ID, Name, Color string; IsGroup bool `json:"isGroup"`; Parent *struct { ID string `json:"id"` } `json:"parent"`; Team *struct { ID string `json:"id"` } `json:"team"` }; PageInfo pageInfo `json:"pageInfo"` } `json:"issueLabels"` }
		if err := c.graphql(ctx, token, `query PatchbayCatalogLabels($after:String){issueLabels(first:250,after:$after){nodes{id name color isGroup parent{id} team{id}}pageInfo{hasNextPage endCursor}}}`, map[string]any{"after": after}, &data); err != nil { return Catalog{}, err }
		for _, label := range data.Labels.Nodes { parent, team := "", ""; if label.Parent != nil { parent = label.Parent.ID }; if label.Team != nil { team = label.Team.ID }; out.Labels = append(out.Labels, CatalogLabel{ID: label.ID, Name: label.Name, Color: label.Color, ParentID: parent, TeamID: team, IsGroup: label.IsGroup}) }
		next, err := nextCatalogCursor(after, data.Labels.PageInfo); if err != nil { return Catalog{}, err }; if next == nil { break }; after = next
	}
	return out, nil
}

func (c *HTTPClient) ValidateBinding(ctx context.Context, token, projectID, teamID string) error {
	var data struct { Project *struct { Teams []struct { ID string `json:"id"` } `json:"teams"` } `json:"project"` }
	if err := c.graphql(ctx, token, `query PatchbayBinding($project:String!){project(id:$project){teams{id}}}`, map[string]any{"project": projectID}, &data); err != nil { return err }
	if data.Project == nil { return &ProviderError{Kind: ErrorMutationRejected, Message: "Linear project does not exist"} }
	for _, team := range data.Project.Teams { if team.ID == teamID { return nil } }
	return &ProviderError{Kind: ErrorMutationRejected, Message: "Linear project is not associated with the selected team"}
}

func (c *HTTPClient) DryRunCounts(ctx context.Context, token, projectID, teamID string, statusMapping map[string]any) (DryRunCounts, error) {
	const q = `query PatchbayIssuePreview($project:ID!,$after:String){issues(first:100,after:$after,filter:{project:{id:{eq:$project}}}){nodes{` + issueFields + `}pageInfo{hasNextPage endCursor}}}`
	result := DryRunCounts{}
	var after *string
	var previous string
	for page := 0; ; page++ {
		if page >= maxIssuePages { return DryRunCounts{}, &ProviderError{Kind: ErrorInvalidResponse, Message: "Linear preview pagination exceeded safety limit"} }
		var data struct { Issues struct { Nodes []issueNode `json:"nodes"`; PageInfo struct { HasNext bool `json:"hasNextPage"`; End *string `json:"endCursor"` } `json:"pageInfo"` } `json:"issues"` }
		if err := c.graphql(ctx,token,q,map[string]any{"project":projectID,"after":after},&data); err != nil { return DryRunCounts{},err }
		for _, node := range data.Issues.Nodes {
			issue, err := issueFromNode(node); if err != nil { return DryRunCounts{},err }
			if teamID != "" && issue.TeamID != teamID { continue }
			if result.RemoteIssues >= maxPreviewIssues { result.Truncated=true; break }
			result.RemoteIssues++
			mapped := false
			if value, ok := statusMapping[issue.StateID]; ok { if text, ok := value.(string); ok { mapped = strings.TrimSpace(text) != "" } else { mapped = value != nil } }
			if !mapped { result.UnmappedStatuses++ }
		}
		if result.Truncated || !data.Issues.PageInfo.HasNext { break }
		if data.Issues.PageInfo.End == nil || strings.TrimSpace(*data.Issues.PageInfo.End)=="" || *data.Issues.PageInfo.End==previous { return DryRunCounts{},&ProviderError{Kind:ErrorInvalidResponse,Message:"Linear preview returned an invalid cursor"} }
		previous,after=*data.Issues.PageInfo.End,data.Issues.PageInfo.End
	}
	return result,nil
}

func inputMap(in IssueInput, create bool) map[string]any {
	description := DescriptionWithPatchbayMarker(in.Description, in.PatchbayIssueID)
	m := map[string]any{"title": in.Title, "description": description, "priority": in.Priority, "dueDate": in.DueDate}
	if create { m["teamId"], m["projectId"] = in.TeamID, in.ProjectID }
	if in.StateID != "" { m["stateId"] = in.StateID }
	if in.AssigneeID != nil { m["assigneeId"] = *in.AssigneeID } else if in.ClearAssignee { m["assigneeId"] = nil }
	return m
}

func (c *HTTPClient) mutateIssue(ctx context.Context, token, operation, id string, input IssueInput) (Issue, error) {
	query := ""
	if operation == "create" { query = `mutation PatchbayCreateIssue($input:IssueCreateInput!){issueCreate(input:$input){success userErrors{message} issue{`+issueFields+`}}}` } else { query = `mutation PatchbayUpdateIssue($id:String!,$input:IssueUpdateInput!){issueUpdate(id:$id,input:$input){success userErrors{message} issue{`+issueFields+`}}}` }
	var envelope struct {
		Create struct { Success bool `json:"success"`; Issue *issueNode `json:"issue"`; UserErrors []struct { Message string `json:"message"` } `json:"userErrors"` } `json:"issueCreate"`
		Update struct { Success bool `json:"success"`; Issue *issueNode `json:"issue"`; UserErrors []struct { Message string `json:"message"` } `json:"userErrors"` } `json:"issueUpdate"`
	}
	variables := map[string]any{"input": inputMap(input, operation == "create")}
	if operation != "create" { variables["id"] = id }
	if err := c.graphql(ctx, token, query, variables, &envelope); err != nil { return Issue{}, err }
	var success bool; var node *issueNode; var messages []struct{ Message string `json:"message"` }
	if operation == "create" { success, node, messages = envelope.Create.Success, envelope.Create.Issue, envelope.Create.UserErrors } else { success, node, messages = envelope.Update.Success, envelope.Update.Issue, envelope.Update.UserErrors }
	if len(messages) > 0 { return Issue{}, &ProviderError{Kind: ErrorMutationRejected, Message: messages[0].Message} }
	if !success || node == nil { return Issue{}, &ProviderError{Kind: ErrorMutationRejected, Message: "Linear mutation returned success=false or no issue"} }
	return issueFromNode(*node)
}

func (c *HTTPClient) CreateIssue(ctx context.Context, token string, in IssueInput) (Issue, error) { return c.mutateIssue(ctx, token, "create", "", in) }
func (c *HTTPClient) UpdateIssue(ctx context.Context, token, id string, in IssueInput) (Issue, error) { return c.mutateIssue(ctx, token, "update", id, in) }

func (c *HTTPClient) DeleteIssue(ctx context.Context, token, id string) error {
	var data struct { Result struct { Success bool `json:"success"`; UserErrors []struct { Message string `json:"message"` } `json:"userErrors"` } `json:"issueDelete"` }
	if err := c.graphql(ctx, token, `mutation PatchbayDeleteIssue($id:String!){issueDelete(id:$id){success userErrors{message}}}`, map[string]any{"id": id}, &data); err != nil { return err }
	if len(data.Result.UserErrors) > 0 { return &ProviderError{Kind: ErrorMutationRejected, Message: data.Result.UserErrors[0].Message} }
	if !data.Result.Success { return &ProviderError{Kind: ErrorMutationRejected, Message: "Linear issueDelete returned success=false"} }
	return nil
}

func PatchbayIssueMarker(issueID string) string { return patchbayMarker + strings.TrimSpace(issueID) + "]" }

func DescriptionWithPatchbayMarker(description, issueID string) string {
	description = StripPatchbayIssueMarker(description)
	if strings.TrimSpace(issueID) == "" { return description }
	if description == "" { return PatchbayIssueMarker(issueID) }
	return description + "\n\n" + PatchbayIssueMarker(issueID)
}

func StripPatchbayIssueMarker(description string) string {
	for {
		start := strings.Index(description, patchbayMarker)
		if start < 0 { return strings.TrimSpace(description) }
		end := strings.Index(description[start:], "]")
		if end < 0 { return strings.TrimSpace(description) }
		description = strings.TrimSpace(description[:start] + description[start+end+1:])
	}
}

func PatchbayIssueIDFromDescription(description string) string {
	start := strings.Index(description, patchbayMarker)
	if start < 0 { return "" }
	value := description[start+len(patchbayMarker):]
	end := strings.IndexByte(value, ']')
	if end < 0 { return "" }
	return strings.TrimSpace(value[:end])
}

func SHA256Hex(value string) string { sum := sha256.Sum256([]byte(value)); return hex.EncodeToString(sum[:]) }

func ParseInt64(value string) (int64, error) { return strconv.ParseInt(strings.TrimSpace(value), 10, 64) }
