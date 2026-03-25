package app

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"reflect"
	"strconv"
	"strings"

	clierr "github.com/ggonzalez94/defi-cli/internal/errors"
	"github.com/ggonzalez94/defi-cli/internal/execution"
	execsigner "github.com/ggonzalez94/defi-cli/internal/execution/signer"
	"github.com/ggonzalez94/defi-cli/internal/ows"
	"github.com/ggonzalez94/defi-cli/internal/schema"
	"github.com/spf13/cobra"
	"github.com/spf13/pflag"
)

const (
	inputJSONFlagName = "input-json"
	inputFileFlagName = "input-file"
)

type structuredInputSource struct {
	inputJSON *string
	inputFile *string
}

type structuredInputOptions struct {
	Mutation         bool
	InputConstraints []schema.InputConstraint
	Auth             []schema.AuthRequirement
	Response         *schema.TypeSchema
	Request          *schema.TypeSchema
}

func configureStructuredInput[T any](cmd *cobra.Command, opts structuredInputOptions) {
	source := addStructuredInputFlags(cmd)
	applyBindingFlagMetadata[T](cmd)
	request := opts.Request
	if request == nil {
		req, err := schema.SchemaFromFlagBindings(cmd, zeroValue[T]())
		if err != nil {
			panic(err)
		}
		request = &req
	}
	response := opts.Response
	if response == nil && opts.Mutation {
		defaultResponse := schema.SchemaFromType(execution.Action{})
		response = &defaultResponse
	}
	setCommandMetadataOrPanic(cmd, schema.CommandMetadata{
		Mutation:         opts.Mutation,
		InputModes:       []string{"flags", "json", "file", "stdin"},
		InputConstraints: opts.InputConstraints,
		Auth:             opts.Auth,
		Request:          request,
		Response:         response,
	})

	prevPreRunE := cmd.PreRunE
	cmd.PreRunE = func(cmd *cobra.Command, args []string) error {
		if err := applyStructuredFlagInput(cmd, source); err != nil {
			return err
		}
		if err := normalizeAndValidateCommandFlags(cmd); err != nil {
			return err
		}
		if prevPreRunE != nil {
			return prevPreRunE(cmd, args)
		}
		return nil
	}
}

func annotateStructuredSubmitCommand[T any](cmd *cobra.Command, _ T) {
	response := schema.SchemaFromType(execution.Action{})
	configureStructuredInput[T](cmd, structuredInputOptions{
		Mutation: true,
		Auth:     executionSubmitAuthRequirements(),
		Response: &response,
	})
	if err := schema.SetFlagMetadata(cmd.Flags(), "action-id", schema.FlagMetadata{Required: true, Format: "action-id"}); err != nil {
		panic(err)
	}
	_ = cmd.MarkFlagRequired("action-id")
}

func zeroValue[T any]() T {
	var zero T
	return zero
}

func annotateStructuredFlagCommand(cmd *cobra.Command, opts structuredInputOptions) {
	source := addStructuredInputFlags(cmd)
	request := opts.Request
	if request == nil {
		request = commandFlagRequestSchema(cmd)
	}
	setCommandMetadataOrPanic(cmd, schema.CommandMetadata{
		Mutation:         opts.Mutation,
		InputModes:       []string{"flags", "json", "file", "stdin"},
		InputConstraints: opts.InputConstraints,
		Auth:             opts.Auth,
		Request:          request,
		Response:         opts.Response,
	})

	prevPreRunE := cmd.PreRunE
	cmd.PreRunE = func(cmd *cobra.Command, args []string) error {
		if err := applyStructuredFlagInput(cmd, source); err != nil {
			return err
		}
		if err := normalizeAndValidateCommandFlags(cmd); err != nil {
			return err
		}
		if prevPreRunE != nil {
			return prevPreRunE(cmd, args)
		}
		return nil
	}
}

func annotateExecutionStatusCommand(cmd *cobra.Command) {
	if err := schema.SetFlagMetadata(cmd.Flags(), "action-id", schema.FlagMetadata{Required: true, Format: "action-id"}); err != nil {
		panic(err)
	}
	_ = cmd.MarkFlagRequired("action-id")
	response := schema.SchemaFromType(execution.Action{})
	setCommandMetadataOrPanic(cmd, schema.CommandMetadata{
		Request:  commandFlagRequestSchema(cmd),
		Response: &response,
	})
}

func setCommandMetadataOrPanic(cmd *cobra.Command, meta schema.CommandMetadata) {
	if err := schema.SetCommandMetadata(cmd, meta); err != nil {
		panic(err)
	}
}

func executionSubmitAuthRequirements() []schema.AuthRequirement {
	return []schema.AuthRequirement{
		{
			Kind:    "wallet",
			EnvVars: []string{ows.EnvOWSToken},
			Description: "Primary auth for wallet-backed execution (execution_backend=ows): set DEFI_OWS_TOKEN in the environment. " +
				"Submit uses the persisted wallet_id and does not accept owner private keys.",
		},
		{
			Kind: "signer",
			EnvVars: []string{
				execsigner.EnvPrivateKey,
				execsigner.EnvPrivateKeyFile,
				execsigner.EnvKeystorePath,
				execsigner.EnvKeystorePassword,
				execsigner.EnvKeystorePasswordFile,
			},
			Optional:    true,
			Description: "Deprecated compatibility auth for legacy_local actions only: provide a local signer via --private-key or env/file/keystore inputs.",
		},
	}
}

func standardExecutionIdentityInputConstraints() []schema.InputConstraint {
	return []schema.InputConstraint{{
		Kind:        "exactly_one_of",
		Fields:      []string{"wallet", "from_address"},
		Description: "Provide exactly one execution identity input. Prefer wallet-backed planning with `wallet`; `from_address` is deprecated compatibility for legacy local signing.",
	}}
}

func swapPlanIdentityInputConstraints() []schema.InputConstraint {
	return []schema.InputConstraint{
		{
			Kind:        "required",
			Fields:      []string{"from_address"},
			When:        map[string][]string{"provider": {"tempo"}},
			Description: "Tempo planning requires `from_address` and does not support `wallet` yet.",
		},
		{
			Kind:        "forbidden",
			Fields:      []string{"wallet"},
			When:        map[string][]string{"provider": {"tempo"}},
			Description: "Tempo planning rejects `wallet`; use `from_address`.",
		},
		{
			Kind:        "exactly_one_of",
			Fields:      []string{"wallet", "from_address"},
			When:        map[string][]string{"provider": {"taikoswap"}},
			Description: "TaikoSwap planning requires exactly one execution identity input. Prefer wallet-backed planning with `wallet`; `from_address` is deprecated compatibility.",
		},
	}
}

func addStructuredInputFlags(cmd *cobra.Command) *structuredInputSource {
	source := newStructuredInputSource()
	if cmd.Flags().Lookup(inputJSONFlagName) == nil {
		cmd.Flags().StringVar(source.inputJSON, inputJSONFlagName, "", "Structured request JSON")
	}
	if cmd.Flags().Lookup(inputFileFlagName) == nil {
		cmd.Flags().StringVar(source.inputFile, inputFileFlagName, "", "Path to structured request JSON file ('-' for stdin)")
	}
	if err := schema.SetFlagMetadata(cmd.Flags(), inputJSONFlagName, schema.FlagMetadata{Format: "json"}); err != nil {
		panic(err)
	}
	if err := schema.SetFlagMetadata(cmd.Flags(), inputFileFlagName, schema.FlagMetadata{Format: "path"}); err != nil {
		panic(err)
	}
	return source
}

func newStructuredInputSource() *structuredInputSource {
	return &structuredInputSource{
		inputJSON: new(string),
		inputFile: new(string),
	}
}

func commandUsesStructuredInput(cmd *cobra.Command) bool {
	if cmd == nil {
		return false
	}
	return cmd.LocalFlags().Lookup(inputJSONFlagName) != nil || cmd.LocalFlags().Lookup(inputFileFlagName) != nil
}

func commandFlagRequestSchema(cmd *cobra.Command) *schema.TypeSchema {
	if cmd == nil {
		return nil
	}
	fields := make([]schema.SchemaField, 0)
	cmd.LocalFlags().VisitAll(func(flag *pflag.Flag) {
		if flag == nil || flag.Hidden || flag.Name == "help" || flag.Name == inputJSONFlagName || flag.Name == inputFileFlagName {
			return
		}
		meta := schema.MergedFlagMetadata(flag)
		fields = append(fields, schema.SchemaField{
			Name:        strings.ReplaceAll(flag.Name, "-", "_"),
			Required:    meta.Required,
			Description: flag.Usage,
			Schema:      typeSchemaForFlag(flag, meta),
		})
	})
	request := schema.TypeSchema{Type: "object", Fields: fields}
	return &request
}

func typeSchemaForFlag(flag *pflag.Flag, meta schema.FlagMetadata) schema.TypeSchema {
	switch flag.Value.Type() {
	case "bool":
		return schema.TypeSchema{Type: "boolean", Enum: append([]string(nil), meta.Enum...), Format: meta.Format}
	case "int", "int8", "int16", "int32", "int64",
		"uint", "uint8", "uint16", "uint32", "uint64":
		return schema.TypeSchema{Type: "integer", Enum: append([]string(nil), meta.Enum...), Format: meta.Format}
	case "float32", "float64":
		return schema.TypeSchema{Type: "number", Enum: append([]string(nil), meta.Enum...), Format: meta.Format}
	case "stringSlice", "stringArray":
		item := schema.TypeSchema{Type: "string"}
		return schema.TypeSchema{Type: "array", Items: &item, Format: meta.Format}
	default:
		return schema.TypeSchema{Type: "string", Enum: append([]string(nil), meta.Enum...), Format: meta.Format}
	}
}

func applyStructuredFlagInput(cmd *cobra.Command, source *structuredInputSource) error {
	if source == nil {
		return nil
	}
	payload, err := readStructuredInput(cmd, stringPointerValue(source.inputJSON), stringPointerValue(source.inputFile))
	if err != nil || len(payload) == 0 {
		return err
	}
	explicit := changedFlagNames(cmd)
	var raw map[string]json.RawMessage
	if err := json.Unmarshal(payload, &raw); err != nil {
		return clierr.Wrap(clierr.CodeUsage, "parse structured input", err)
	}
	for key, rawValue := range raw {
		flagName := strings.ReplaceAll(strings.TrimSpace(key), "_", "-")
		flag := cmd.LocalFlags().Lookup(flagName)
		if flag == nil || flag.Hidden || flag.Name == inputJSONFlagName || flag.Name == inputFileFlagName {
			return clierr.New(clierr.CodeUsage, fmt.Sprintf("structured input field %q is not supported by %s", key, trimRootPath(cmd.CommandPath())))
		}
		if explicit[flagName] {
			continue
		}
		if bytes.Equal(bytes.TrimSpace(rawValue), []byte("null")) {
			return clierr.New(clierr.CodeUsage, fmt.Sprintf("structured input field %q cannot be null", key))
		}
		flagValue, err := decodeRawFlagValue(flag, rawValue)
		if err != nil {
			return clierr.Wrap(clierr.CodeUsage, fmt.Sprintf("decode structured input field %q", key), err)
		}
		if err := cmd.Flags().Set(flagName, flagValue); err != nil {
			return clierr.Wrap(clierr.CodeUsage, fmt.Sprintf("apply structured input field %q", key), err)
		}
	}
	return nil
}

func changedFlagNames(cmd *cobra.Command) map[string]bool {
	changed := map[string]bool{}
	cmd.Flags().Visit(func(flag *pflag.Flag) {
		changed[flag.Name] = true
	})
	return changed
}

func decodeRawFlagValue(flag *pflag.Flag, raw json.RawMessage) (string, error) {
	switch flag.Value.Type() {
	case "string":
		var value string
		if err := json.Unmarshal(raw, &value); err != nil {
			return "", err
		}
		return value, nil
	case "bool":
		var value bool
		if err := json.Unmarshal(raw, &value); err != nil {
			return "", err
		}
		return strconv.FormatBool(value), nil
	case "int", "int8", "int16", "int32", "int64":
		var value int64
		if err := json.Unmarshal(raw, &value); err != nil {
			return "", err
		}
		return strconv.FormatInt(value, 10), nil
	case "uint", "uint8", "uint16", "uint32", "uint64":
		var value uint64
		if err := json.Unmarshal(raw, &value); err != nil {
			return "", err
		}
		return strconv.FormatUint(value, 10), nil
	case "float32", "float64":
		var value float64
		if err := json.Unmarshal(raw, &value); err != nil {
			return "", err
		}
		return strconv.FormatFloat(value, 'f', -1, 64), nil
	case "stringSlice", "stringArray":
		var values []string
		if err := json.Unmarshal(raw, &values); err == nil {
			return strings.Join(values, ","), nil
		}
		var value string
		if err := json.Unmarshal(raw, &value); err != nil {
			return "", err
		}
		return value, nil
	default:
		return "", fmt.Errorf("unsupported flag type %s", flag.Value.Type())
	}
}

func stringPointerValue(value *string) string {
	if value == nil {
		return ""
	}
	return *value
}

func normalizeStringSlice(values []string) []string {
	out := make([]string, 0, len(values))
	for _, value := range values {
		value = strings.TrimSpace(value)
		if value != "" {
			out = append(out, value)
		}
	}
	return out
}

func readStructuredInput(cmd *cobra.Command, inputJSON, inputFile string) ([]byte, error) {
	jsonInput := strings.TrimSpace(inputJSON)
	fileInput := strings.TrimSpace(inputFile)
	if jsonInput != "" && fileInput != "" {
		return nil, clierr.New(clierr.CodeUsage, "use only one of --input-json or --input-file")
	}
	if jsonInput != "" {
		return []byte(jsonInput), nil
	}
	if fileInput == "" {
		return nil, nil
	}
	if fileInput == "-" {
		buf, err := io.ReadAll(cmd.InOrStdin())
		if err != nil {
			return nil, clierr.Wrap(clierr.CodeUsage, "read structured input from stdin", err)
		}
		return buf, nil
	}
	path, err := canonicalizeCLIPath(fileInput)
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeUsage, "resolve --input-file", err)
	}
	buf, err := os.ReadFile(path)
	if err != nil {
		return nil, clierr.Wrap(clierr.CodeUsage, "read structured input file", err)
	}
	return buf, nil
}

func applyBindingFlagMetadata[T any](cmd *cobra.Command) {
	for _, field := range bindingFields[T]() {
		meta := schema.FlagMetadata{}
		if field.Required {
			meta.Required = true
		}
		if len(field.Enum) > 0 {
			meta.Enum = append([]string(nil), field.Enum...)
		}
		if field.Format != "" {
			meta.Format = field.Format
		}
		if !meta.Required && len(meta.Enum) == 0 && meta.Format == "" {
			continue
		}
		if err := schema.SetFlagMetadata(cmd.Flags(), field.FlagName, meta); err != nil {
			panic(err)
		}
	}
}

type bindingField struct {
	FlagName string
	Required bool
	Format   string
	Enum     []string
}

func bindingFields[T any]() []bindingField {
	var zero T
	typ := reflect.TypeOf(zero)
	for typ.Kind() == reflect.Pointer {
		typ = typ.Elem()
	}
	if typ.Kind() != reflect.Struct {
		panic(fmt.Sprintf("structured input binding must be a struct, got %s", typ.Kind()))
	}
	fields := make([]bindingField, 0, typ.NumField())
	for i := 0; i < typ.NumField(); i++ {
		field := typ.Field(i)
		if !field.IsExported() {
			continue
		}
		jsonName := jsonBindingFieldName(field)
		if jsonName == "" {
			continue
		}
		flagName := strings.TrimSpace(field.Tag.Get("flag"))
		if flagName == "" {
			continue
		}
		fields = append(fields, bindingField{
			FlagName: flagName,
			Required: strings.EqualFold(strings.TrimSpace(field.Tag.Get("required")), "true"),
			Format:   strings.TrimSpace(field.Tag.Get("format")),
			Enum:     splitBindingEnum(field.Tag.Get("enum")),
		})
	}
	return fields
}

func jsonBindingFieldName(field reflect.StructField) string {
	tag := field.Tag.Get("json")
	if tag == "-" {
		return ""
	}
	if tag == "" {
		return field.Name
	}
	name, _, _ := strings.Cut(tag, ",")
	if name == "" {
		return field.Name
	}
	return name
}

func splitBindingEnum(raw string) []string {
	parts := strings.Split(strings.TrimSpace(raw), ",")
	values := make([]string, 0, len(parts))
	for _, part := range parts {
		part = strings.TrimSpace(part)
		if part != "" {
			values = append(values, part)
		}
	}
	return values
}
