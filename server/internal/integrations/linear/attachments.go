package linear

import (
	"context"
	"errors"
	"net/url"
)

// Linear upserts attachments by URL within the issue, making retries safe.
func (c *HTTPClient) UpsertAttachment(ctx context.Context, token, issueID, title, rawURL string) error {
	u, err := url.Parse(rawURL)
	if err != nil || u.Scheme != "https" || u.Host == "" || u.User != nil {
		return errors.New("Linear attachment requires an HTTPS URL without credentials")
	}
	var response struct {
		AttachmentCreate struct {
			Success bool `json:"success"`
		} `json:"attachmentCreate"`
	}
	if err = c.graphql(ctx, token, `mutation($input: AttachmentCreateInput!) { attachmentCreate(input: $input) { success } }`, map[string]any{"input": map[string]any{"issueId": issueID, "title": title, "url": rawURL}}, &response); err != nil {
		return err
	}
	if !response.AttachmentCreate.Success {
		return errors.New("Linear rejected attachment")
	}
	return nil
}
