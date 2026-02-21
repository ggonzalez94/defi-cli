package schema

import (
	"fmt"
	"strings"

	"github.com/spf13/cobra"
	"github.com/spf13/pflag"
)

type CommandSchema struct {
	Path        string          `json:"path"`
	Use         string          `json:"use"`
	Short       string          `json:"short"`
	Aliases     []string        `json:"aliases,omitempty"`
	Flags       []FlagSchema    `json:"flags,omitempty"`
	Subcommands []CommandSchema `json:"subcommands,omitempty"`
}

type FlagSchema struct {
	Name      string `json:"name"`
	Shorthand string `json:"shorthand,omitempty"`
	Type      string `json:"type"`
	Usage     string `json:"usage"`
	Default   string `json:"default,omitempty"`
}

func Build(root *cobra.Command, commandPath string) (CommandSchema, error) {
	cmd := root
	if strings.TrimSpace(commandPath) != "" {
		parts := strings.Fields(strings.TrimSpace(commandPath))
		for _, p := range parts {
			found := false
			for _, c := range cmd.Commands() {
				if c.Name() == p || contains(c.Aliases, p) {
					cmd = c
					found = true
					break
				}
			}
			if !found {
				return CommandSchema{}, fmt.Errorf("command not found: %s", commandPath)
			}
		}
	}
	return serialize(cmd), nil
}

func serialize(cmd *cobra.Command) CommandSchema {
	s := CommandSchema{
		Path:    strings.TrimSpace(cmd.CommandPath()),
		Use:     cmd.Use,
		Short:   cmd.Short,
		Aliases: cmd.Aliases,
		Flags:   collectFlags(cmd),
	}

	subs := cmd.Commands()
	for _, sub := range subs {
		if sub.Hidden {
			continue
		}
		s.Subcommands = append(s.Subcommands, serialize(sub))
	}

	return s
}

func collectFlags(cmd *cobra.Command) []FlagSchema {
	items := []FlagSchema{}
	cmd.NonInheritedFlags().VisitAll(func(f *pflag.Flag) {
		item := FlagSchema{
			Name:      f.Name,
			Shorthand: f.Shorthand,
			Type:      f.Value.Type(),
			Usage:     f.Usage,
			Default:   f.DefValue,
		}
		items = append(items, item)
	})
	return items
}

func contains(items []string, target string) bool {
	for _, item := range items {
		if item == target {
			return true
		}
	}
	return false
}
