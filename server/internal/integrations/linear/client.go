package linear

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"time"
)

const (
	DefaultGraphQLURL = "https://api.linear.app/graphql"
	DefaultTokenURL   = "https://api.linear.app/oauth/token"
	DefaultRevokeURL  = "https://api.linear.app/oauth/revoke"
)

type Issue struct {
	ID, Identifier, Title, Description, StateID, StateType, ProjectID, TeamID, AssigneeID string
	Priority                                                                              int
	UpdatedAt                                                                             time.Time
	Deleted                                                                               bool
}
type IssueInput struct {
	TeamID, ProjectID, Title, Description, StateID, AssigneeID string
	Priority                                                   int
}
type Token struct {
	AccessToken, RefreshToken, Scope string
	ExpiresIn                        time.Duration
}
type API interface {
	RefreshToken(context.Context, string, string, string) (Token, error)
	RevokeToken(context.Context, string, string, string) error
	ListIssues(context.Context, string, string, string) ([]Issue, error)
	CreateIssue(context.Context, string, IssueInput) (Issue, error)
	UpdateIssue(context.Context, string, string, IssueInput) (Issue, error)
	DeleteIssue(context.Context, string, string) error
}
type HTTPClient struct {
	HTTP                            *http.Client
	GraphQLURL, TokenURL, RevokeURL string
}

func NewHTTPClient(client *http.Client) *HTTPClient {
	if client == nil {
		client = &http.Client{Timeout: 20 * time.Second}
	}
	return &HTTPClient{HTTP: client, GraphQLURL: DefaultGraphQLURL, TokenURL: DefaultTokenURL, RevokeURL: DefaultRevokeURL}
}
func endpoint(value, fallback string) string {
	if strings.TrimSpace(value) != "" {
		return value
	}
	return fallback
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
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 4096))
		return fmt.Errorf("linear http %d: %s", resp.StatusCode, strings.TrimSpace(string(body)))
	}
	if out == nil {
		return nil
	}
	return json.NewDecoder(io.LimitReader(resp.Body, 1<<20)).Decode(out)
}
func (c *HTTPClient) RefreshToken(ctx context.Context, refresh, clientID, clientSecret string) (Token, error) {
	var raw struct {
		AccessToken  string `json:"access_token"`
		RefreshToken string `json:"refresh_token"`
		Scope        string `json:"scope"`
		ExpiresIn    int64  `json:"expires_in"`
	}
	err := c.postForm(ctx, endpoint(c.TokenURL, DefaultTokenURL), url.Values{"grant_type": {"refresh_token"}, "refresh_token": {refresh}, "client_id": {clientID}, "client_secret": {clientSecret}}, &raw)
	if err != nil {
		return Token{}, err
	}
	if raw.AccessToken == "" {
		return Token{}, fmt.Errorf("linear refresh response omitted access_token")
	}
	if raw.RefreshToken == "" {
		raw.RefreshToken = refresh
	}
	return Token{raw.AccessToken, raw.RefreshToken, raw.Scope, time.Duration(raw.ExpiresIn) * time.Second}, nil
}
func (c *HTTPClient) RevokeToken(ctx context.Context, token, clientID, clientSecret string) error {
	return c.postForm(ctx, endpoint(c.RevokeURL, DefaultRevokeURL), url.Values{"token": {token}, "client_id": {clientID}, "client_secret": {clientSecret}}, nil)
}
func (c *HTTPClient) graphql(ctx context.Context, token, query string, variables map[string]any, out any) error {
	payload, _ := json.Marshal(map[string]any{"query": query, "variables": variables})
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, endpoint(c.GraphQLURL, DefaultGraphQLURL), bytes.NewReader(payload))
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", token)
	req.Header.Set("Content-Type", "application/json")
	resp, err := c.HTTP.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode/100 != 2 {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 4096))
		return fmt.Errorf("linear graphql %d: %s", resp.StatusCode, strings.TrimSpace(string(body)))
	}
	var env struct {
		Data   json.RawMessage `json:"data"`
		Errors []struct {
			Message string `json:"message"`
		} `json:"errors"`
	}
	if err := json.NewDecoder(io.LimitReader(resp.Body, 4<<20)).Decode(&env); err != nil {
		return err
	}
	if len(env.Errors) > 0 {
		return fmt.Errorf("linear graphql: %s", env.Errors[0].Message)
	}
	return json.Unmarshal(env.Data, out)
}

type issueNode struct {
	ID          string    `json:"id"`
	Identifier  string    `json:"identifier"`
	Title       string    `json:"title"`
	Description *string   `json:"description"`
	Priority    int       `json:"priority"`
	UpdatedAt   time.Time `json:"updatedAt"`
	State       *struct {
		ID   string `json:"id"`
		Type string `json:"type"`
	} `json:"state"`
	Project *struct {
		ID string `json:"id"`
	} `json:"project"`
	Team struct {
		ID string `json:"id"`
	} `json:"team"`
	Assignee *struct {
		ID string `json:"id"`
	} `json:"assignee"`
}

func issueFromNode(n issueNode) Issue {
	o := Issue{ID: n.ID, Identifier: n.Identifier, Title: n.Title, Priority: n.Priority, UpdatedAt: n.UpdatedAt, TeamID: n.Team.ID}
	if n.Description != nil {
		o.Description = *n.Description
	}
	if n.State != nil {
		o.StateID, o.StateType = n.State.ID, n.State.Type
	}
	if n.Project != nil {
		o.ProjectID = n.Project.ID
	}
	if n.Assignee != nil {
		o.AssigneeID = n.Assignee.ID
	}
	return o
}
func (c *HTTPClient) ListIssues(ctx context.Context, token, projectID, teamID string) ([]Issue, error) {
	const q = `query PatchbayIssues($project:ID!,$after:String){issues(first:100,after:$after,filter:{project:{id:{eq:$project}}}){nodes{id identifier title description priority updatedAt state{id type} project{id} team{id} assignee{id}} pageInfo{hasNextPage endCursor}}}`
	var all []Issue
	var after any
	for {
		var data struct {
			Issues struct {
				Nodes    []issueNode `json:"nodes"`
				PageInfo struct {
					HasNext bool    `json:"hasNextPage"`
					End     *string `json:"endCursor"`
				} `json:"pageInfo"`
			} `json:"issues"`
		}
		if err := c.graphql(ctx, token, q, map[string]any{"project": projectID, "after": after}, &data); err != nil {
			return nil, err
		}
		for _, n := range data.Issues.Nodes {
			i := issueFromNode(n)
			if teamID == "" || i.TeamID == teamID {
				all = append(all, i)
			}
		}
		if !data.Issues.PageInfo.HasNext || data.Issues.PageInfo.End == nil {
			break
		}
		after = *data.Issues.PageInfo.End
	}
	return all, nil
}
func inputMap(in IssueInput) map[string]any {
	m := map[string]any{"teamId": in.TeamID, "title": in.Title, "description": in.Description, "priority": in.Priority}
	if in.ProjectID != "" {
		m["projectId"] = in.ProjectID
	}
	if in.StateID != "" {
		m["stateId"] = in.StateID
	}
	if in.AssigneeID != "" {
		m["assigneeId"] = in.AssigneeID
	}
	return m
}
func (c *HTTPClient) CreateIssue(ctx context.Context, token string, in IssueInput) (Issue, error) {
	var d struct {
		Result struct {
			Success bool      `json:"success"`
			Issue   issueNode `json:"issue"`
		} `json:"issueCreate"`
	}
	if err := c.graphql(ctx, token, `mutation PatchbayCreateIssue($input:IssueCreateInput!){issueCreate(input:$input){success issue{id identifier title description priority updatedAt state{id type} project{id} team{id} assignee{id}}}}`, map[string]any{"input": inputMap(in)}, &d); err != nil {
		return Issue{}, err
	}
	if !d.Result.Success {
		return Issue{}, fmt.Errorf("linear issueCreate returned success=false")
	}
	return issueFromNode(d.Result.Issue), nil
}
func (c *HTTPClient) UpdateIssue(ctx context.Context, token, id string, in IssueInput) (Issue, error) {
	var d struct {
		Result struct {
			Success bool      `json:"success"`
			Issue   issueNode `json:"issue"`
		} `json:"issueUpdate"`
	}
	if err := c.graphql(ctx, token, `mutation PatchbayUpdateIssue($id:String!,$input:IssueUpdateInput!){issueUpdate(id:$id,input:$input){success issue{id identifier title description priority updatedAt state{id type} project{id} team{id} assignee{id}}}}`, map[string]any{"id": id, "input": inputMap(in)}, &d); err != nil {
		return Issue{}, err
	}
	if !d.Result.Success {
		return Issue{}, fmt.Errorf("linear issueUpdate returned success=false")
	}
	return issueFromNode(d.Result.Issue), nil
}
func (c *HTTPClient) DeleteIssue(ctx context.Context, token, id string) error {
	var d struct {
		Result struct {
			Success bool `json:"success"`
		} `json:"issueDelete"`
	}
	if err := c.graphql(ctx, token, `mutation PatchbayDeleteIssue($id:String!){issueDelete(id:$id){success}}`, map[string]any{"id": id}, &d); err != nil {
		return err
	}
	if !d.Result.Success {
		return fmt.Errorf("linear issueDelete returned success=false")
	}
	return nil
}
