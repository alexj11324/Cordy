package main

import (
	"context"
	"fmt"
	"net/url"
	"os"
	"strconv"
	"unicode/utf8"

	"github.com/patchbay-ai/patchbay/server/internal/cli"
	"github.com/spf13/cobra"
)

func init() {
	memory := &cobra.Command{Use: "memory", Short: "Read and update persistent notes owned by an automation"}
	for _, operation := range []string{"list", "read", "write", "delete"} {
		cmd := &cobra.Command{Use: operation + " <automation-id> [name.md]", Short: operation + " automation memory", Args: cobra.RangeArgs(1, 2), RunE: runAutomationMemory}
		cmd.Flags().String("output", "json", "Output format (json)")
		if operation == "list" {
			cmd.Use = "list <automation-id>"
			cmd.Args = exactArgs(1)
		}
		if operation == "write" {
			cmd.Flags().String("content", "", "Markdown content (use --file for multiline notes)")
			cmd.Flags().String("file", "", "Read UTF-8 Markdown content from this local file")
			cmd.MarkFlagsMutuallyExclusive("content", "file")
			cmd.MarkFlagsOneRequired("content", "file")
		}
		if operation == "write" || operation == "delete" {
			cmd.Flags().Int64("revision", -1, "Revision returned by read; use 0 to create a new note")
			_ = cmd.MarkFlagRequired("revision")
		}
		memory.AddCommand(cmd)
	}
	automationCmd.AddCommand(memory)
}

func runAutomationMemory(cmd *cobra.Command, args []string) error {
	output, _ := cmd.Flags().GetString("output")
	if output != "json" {
		return fmt.Errorf("automation memory supports --output json")
	}
	if _, err := requireWorkspaceID(cmd); err != nil {
		return err
	}
	client, err := newAPIClient(cmd)
	if err != nil {
		return err
	}
	ctx, cancel := cli.APIContext(context.Background())
	defer cancel()
	path := "/api/automations/" + url.PathEscape(args[0]) + "/memories"
	if cmd.Name() != "list" {
		name := "MEMORIES.md"
		if len(args) == 2 {
			name = args[1]
		}
		path += "/" + url.PathEscape(name)
	}
	var result any
	switch cmd.Name() {
	case "list", "read":
		err = client.GetJSON(ctx, path, &result)
	case "write":
		revision, _ := cmd.Flags().GetInt64("revision")
		if revision < 0 {
			return fmt.Errorf("--revision must be nonnegative")
		}
		content, _ := cmd.Flags().GetString("content")
		if cmd.Flags().Changed("file") {
			file, _ := cmd.Flags().GetString("file")
			data, readErr := os.ReadFile(file)
			if readErr != nil {
				return readErr
			}
			content = string(data)
		}
		if len(content) > 65536 {
			return fmt.Errorf("memory content must be at most 64 KiB")
		}
		if !utf8.ValidString(content) {
			return fmt.Errorf("memory content must be valid UTF-8")
		}
		err = client.PutJSON(ctx, path, map[string]any{"content": content, "expected_revision": revision}, &result)
	case "delete":
		revision, _ := cmd.Flags().GetInt64("revision")
		if revision < 1 {
			return fmt.Errorf("--revision must be positive")
		}
		err = client.DeleteJSON(ctx, path+"?revision="+strconv.FormatInt(revision, 10))
		result = map[string]bool{"deleted": true}
	}
	if err != nil {
		return fmt.Errorf("%s automation memory: %w", cmd.Name(), err)
	}
	return cli.PrintJSON(cmd.OutOrStdout(), result)
}
