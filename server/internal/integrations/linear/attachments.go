package linear

import (
	"context"
	"errors"
	"net/url"
)

// Linear upserts attachments by URL within the issue, making retries safe.
func (c *HTTPClient) UpsertAttachment(ctx context.Context, token, issueID, title, rawURL string) error {
	if err := validateAttachmentURL(rawURL); err != nil {
		return err
	}
	var response struct {
		AttachmentCreate struct {
			Success bool `json:"success"`
		} `json:"attachmentCreate"`
	}
	if err := c.graphql(ctx, token, `mutation($input: AttachmentCreateInput!) { attachmentCreate(input: $input) { success } }`, map[string]any{"input": map[string]any{"issueId": issueID, "title": title, "url": rawURL}}, &response); err != nil {
		return err
	}
	if !response.AttachmentCreate.Success {
		return errors.New("Linear rejected attachment")
	}
	return nil
}

// DeleteAttachmentByURL keeps the integration stateless: Linear documents URL
// as an attachment identity and exposes attachmentsForURL specifically for
// integrations that do not persist provider attachment IDs.
func (c *HTTPClient) DeleteAttachmentByURL(ctx context.Context, token, issueID, rawURL string) error {
	if err := validateAttachmentURL(rawURL); err != nil {
		return err
	}
	var listed struct {
		AttachmentsForURL struct {
			Nodes []struct {
				ID    string `json:"id"`
				Issue struct {
					ID string `json:"id"`
				} `json:"issue"`
			} `json:"nodes"`
		} `json:"attachmentsForURL"`
	}
	if err := c.graphql(ctx, token, `query($url: String!) { attachmentsForURL(url: $url) { nodes { id issue { id } } } }`, map[string]any{"url": rawURL}, &listed); err != nil {
		return err
	}
	for _, attachment := range listed.AttachmentsForURL.Nodes {
		if attachment.Issue.ID != issueID {
			continue
		}
		var deleted struct {
			AttachmentDelete struct {
				Success bool `json:"success"`
			} `json:"attachmentDelete"`
		}
		if err := c.graphql(ctx, token, `mutation($id: String!) { attachmentDelete(id: $id) { success } }`, map[string]any{"id": attachment.ID}, &deleted); err != nil {
			return err
		}
		if !deleted.AttachmentDelete.Success {
			return errors.New("Linear rejected attachment deletion")
		}
	}
	return nil
}

func validateAttachmentURL(rawURL string) error {
	u, err := url.Parse(rawURL)
	if err != nil || u.Scheme != "https" || u.Host == "" || u.User != nil {
		return errors.New("Linear attachment requires an HTTPS URL without credentials")
	}
	return nil
}
