package linear

import (
	"context"
	"errors"
	"time"
)

// Comment preserves the provider identity and reply relationship. Callers must
// scope Issue.ID through a project binding before applying it locally.
type Comment struct {
	ID        string    `json:"id"`
	Body      string    `json:"body"`
	URL       string    `json:"url"`
	CreatedAt time.Time `json:"createdAt"`
	UpdatedAt time.Time `json:"updatedAt"`
	Issue     struct {
		ID string `json:"id"`
	} `json:"issue"`
	Parent *struct {
		ID string `json:"id"`
	} `json:"parent"`
	User *struct {
		ID   string `json:"id"`
		Name string `json:"name"`
	} `json:"user"`
}

const commentFields = `id body url createdAt updatedAt issue { id } parent { id } user { id name }`

func (c *HTTPClient) FetchComment(ctx context.Context, token, id string) (Comment, bool, error) {
	var response struct {
		Comments struct {
			Nodes []Comment `json:"nodes"`
		} `json:"comments"`
	}
	err := c.graphql(ctx, token, `query($id: ID!) { comments(filter: { id: { eq: $id } }, first: 1) { nodes { `+commentFields+` } } }`, map[string]any{"id": id}, &response)
	if err != nil {
		return Comment{}, false, err
	}
	if len(response.Comments.Nodes) == 0 {
		return Comment{}, false, nil
	}
	comment := response.Comments.Nodes[0]
	if comment.ID != id || comment.Issue.ID == "" || comment.UpdatedAt.IsZero() {
		return Comment{}, false, errors.New("Linear returned an incomplete comment")
	}
	return comment, true, nil
}

func (c *HTTPClient) ListComments(ctx context.Context, token, issueID string) ([]Comment, error) {
	var result []Comment
	cursor := ""
	for page := 0; page < maxIssuePages; page++ {
		var response struct {
			Comments struct {
				Nodes    []Comment `json:"nodes"`
				PageInfo struct {
					HasNextPage bool   `json:"hasNextPage"`
					EndCursor   string `json:"endCursor"`
				} `json:"pageInfo"`
			} `json:"comments"`
		}
		variables := map[string]any{"issue": issueID, "after": nil}
		if cursor != "" {
			variables["after"] = cursor
		}
		if err := c.graphql(ctx, token, `query($issue: ID!, $after: String) { comments(filter: { issue: { id: { eq: $issue } } }, first: 100, after: $after) { nodes { `+commentFields+` } pageInfo { hasNextPage endCursor } } }`, variables, &response); err != nil {
			return nil, err
		}
		for _, comment := range response.Comments.Nodes {
			if comment.ID == "" || comment.Issue.ID != issueID || comment.UpdatedAt.IsZero() {
				return nil, errors.New("Linear returned an invalid comment page")
			}
			result = append(result, comment)
		}
		if !response.Comments.PageInfo.HasNextPage {
			return result, nil
		}
		if response.Comments.PageInfo.EndCursor == "" || response.Comments.PageInfo.EndCursor == cursor {
			return nil, errors.New("Linear comment pagination did not advance")
		}
		cursor = response.Comments.PageInfo.EndCursor
	}
	return nil, errors.New("Linear comment pagination limit reached")
}

// CreateComment accepts a persisted UUID v4 so retries recover the same remote
// entity after a successful provider write and a lost local acknowledgement.
func (c *HTTPClient) CreateComment(ctx context.Context, token, id, issueID, parentID, body, author string) (Comment, error) {
	input := map[string]any{"id": id, "issueId": issueID, "body": body, "doNotSubscribeToIssue": true}
	if parentID != "" {
		input["parentId"] = parentID
	}
	if author != "" {
		input["createAsUser"] = author
	}
	var response struct {
		CommentCreate struct {
			Success bool    `json:"success"`
			Comment Comment `json:"comment"`
		} `json:"commentCreate"`
	}
	if err := c.graphql(ctx, token, `mutation($input: CommentCreateInput!) { commentCreate(input: $input) { success comment { `+commentFields+` } } }`, map[string]any{"input": input}, &response); err != nil {
		return Comment{}, err
	}
	if !response.CommentCreate.Success || response.CommentCreate.Comment.ID != id {
		return Comment{}, errors.New("Linear rejected comment creation")
	}
	return response.CommentCreate.Comment, nil
}

func (c *HTTPClient) UpdateComment(ctx context.Context, token, id, body string) error {
	var response struct {
		CommentUpdate struct {
			Success bool `json:"success"`
		} `json:"commentUpdate"`
	}
	if err := c.graphql(ctx, token, `mutation($id: String!, $input: CommentUpdateInput!) { commentUpdate(id: $id, input: $input) { success } }`, map[string]any{"id": id, "input": map[string]any{"body": body}}, &response); err != nil {
		return err
	}
	if !response.CommentUpdate.Success {
		return errors.New("Linear rejected comment update")
	}
	return nil
}

func (c *HTTPClient) DeleteComment(ctx context.Context, token, id string) error {
	var response struct {
		CommentDelete struct {
			Success bool `json:"success"`
		} `json:"commentDelete"`
	}
	if err := c.graphql(ctx, token, `mutation($id: String!) { commentDelete(id: $id) { success } }`, map[string]any{"id": id}, &response); err != nil {
		return err
	}
	if !response.CommentDelete.Success {
		return errors.New("Linear rejected comment deletion")
	}
	return nil
}
