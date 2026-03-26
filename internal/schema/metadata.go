package schema

import (
	"encoding/json"
	"fmt"
	"reflect"
	"strconv"
	"strings"
	"time"

	"github.com/spf13/cobra"
	"github.com/spf13/pflag"
)

const (
	commandMetadataAnnotation = "defi.schema.command"
	flagMetadataAnnotation    = "defi.schema.flag"
)

type CommandMetadata struct {
	Mutation         bool              `json:"mutation,omitempty"`
	InputModes       []string          `json:"input_modes,omitempty"`
	InputConstraints []InputConstraint `json:"input_constraints,omitempty"`
	Auth             []AuthRequirement `json:"auth,omitempty"`
	Request          *TypeSchema       `json:"request,omitempty"`
	Response         *TypeSchema       `json:"response,omitempty"`
}

type AuthRequirement struct {
	Kind        string              `json:"kind"`
	EnvVars     []string            `json:"env_vars,omitempty"`
	Optional    bool                `json:"optional,omitempty"`
	When        map[string][]string `json:"when,omitempty"`
	Description string              `json:"description,omitempty"`
}

type InputConstraint struct {
	Kind        string              `json:"kind"`
	Fields      []string            `json:"fields,omitempty"`
	When        map[string][]string `json:"when,omitempty"`
	Description string              `json:"description,omitempty"`
}

type FlagMetadata struct {
	Required bool     `json:"required,omitempty"`
	Enum     []string `json:"enum,omitempty"`
	Format   string   `json:"format,omitempty"`
}

type TypeSchema struct {
	Type                 string        `json:"type"`
	Format               string        `json:"format,omitempty"`
	Description          string        `json:"description,omitempty"`
	Enum                 []string      `json:"enum,omitempty"`
	Fields               []SchemaField `json:"fields,omitempty"`
	Items                *TypeSchema   `json:"items,omitempty"`
	AdditionalProperties *TypeSchema   `json:"additional_properties,omitempty"`
}

type SchemaField struct {
	Name        string     `json:"name"`
	Required    bool       `json:"required,omitempty"`
	Default     any        `json:"default,omitempty"`
	Description string     `json:"description,omitempty"`
	Schema      TypeSchema `json:"schema"`
}

func SetCommandMetadata(cmd *cobra.Command, meta CommandMetadata) error {
	raw, err := json.Marshal(meta)
	if err != nil {
		return fmt.Errorf("marshal command metadata: %w", err)
	}
	if cmd.Annotations == nil {
		cmd.Annotations = map[string]string{}
	}
	cmd.Annotations[commandMetadataAnnotation] = string(raw)
	return nil
}

func SetFlagMetadata(flags *pflag.FlagSet, name string, meta FlagMetadata) error {
	flag := flags.Lookup(name)
	if flag == nil {
		return fmt.Errorf("flag %q not found", name)
	}
	raw, err := json.Marshal(meta)
	if err != nil {
		return fmt.Errorf("marshal flag metadata: %w", err)
	}
	if flag.Annotations == nil {
		flag.Annotations = map[string][]string{}
	}
	flag.Annotations[flagMetadataAnnotation] = []string{string(raw)}
	return nil
}

func CommandMetadataFor(cmd *cobra.Command) CommandMetadata {
	if cmd == nil || cmd.Annotations == nil {
		return CommandMetadata{}
	}
	raw := strings.TrimSpace(cmd.Annotations[commandMetadataAnnotation])
	if raw == "" {
		return CommandMetadata{}
	}
	var meta CommandMetadata
	if err := json.Unmarshal([]byte(raw), &meta); err != nil {
		return CommandMetadata{}
	}
	return meta
}

func FlagMetadataFor(flag *pflag.Flag) FlagMetadata {
	if flag == nil || flag.Annotations == nil {
		return FlagMetadata{}
	}
	values := flag.Annotations[flagMetadataAnnotation]
	if len(values) == 0 {
		return FlagMetadata{}
	}
	var meta FlagMetadata
	if err := json.Unmarshal([]byte(values[0]), &meta); err != nil {
		return FlagMetadata{}
	}
	return meta
}

func SchemaFromType(value any) TypeSchema {
	return schemaFromReflectType(reflect.TypeOf(value))
}

func SchemaFromFlagBindings(cmd *cobra.Command, binding any) (TypeSchema, error) {
	typ := reflect.TypeOf(binding)
	for typ.Kind() == reflect.Pointer {
		typ = typ.Elem()
	}
	if typ.Kind() != reflect.Struct {
		return TypeSchema{}, fmt.Errorf("binding must be a struct, got %s", typ.Kind())
	}

	fields := make([]SchemaField, 0, typ.NumField())
	for i := 0; i < typ.NumField(); i++ {
		field := typ.Field(i)
		if !field.IsExported() {
			continue
		}
		jsonName := jsonFieldName(field)
		if jsonName == "" {
			continue
		}
		flagName := strings.TrimSpace(field.Tag.Get("flag"))
		if flagName == "" {
			flagName = jsonName
		}
		flag := cmd.Flags().Lookup(flagName)
		fieldSchema := SchemaField{
			Name:     jsonName,
			Required: strings.EqualFold(strings.TrimSpace(field.Tag.Get("required")), "true"),
			Schema:   schemaFromReflectType(field.Type),
		}
		if flag != nil {
			fieldSchema.Description = flag.Usage
			fieldSchema.Default = parseFlagDefault(flag)
			flagMeta := MergedFlagMetadata(flag)
			if !fieldSchema.Required {
				fieldSchema.Required = flagMeta.Required
			}
			if fieldSchema.Schema.Format == "" {
				fieldSchema.Schema.Format = fieldMetaFormat(field, flagMeta)
			}
			if len(fieldSchema.Schema.Enum) == 0 {
				fieldSchema.Schema.Enum = fieldMetaEnum(field, flagMeta, flag)
			}
		} else {
			if format := strings.TrimSpace(field.Tag.Get("format")); format != "" {
				fieldSchema.Schema.Format = format
			}
			if enumTag := strings.TrimSpace(field.Tag.Get("enum")); enumTag != "" {
				fieldSchema.Schema.Enum = splitSchemaEnum(enumTag)
			}
		}
		fields = append(fields, fieldSchema)
	}

	return TypeSchema{Type: "object", Fields: fields}, nil
}

func schemaFromReflectType(typ reflect.Type) TypeSchema {
	return schemaFromReflectTypeSeen(typ, map[reflect.Type]bool{})
}

func schemaFromReflectTypeSeen(typ reflect.Type, seen map[reflect.Type]bool) TypeSchema {
	for typ.Kind() == reflect.Pointer {
		typ = typ.Elem()
	}
	if typ == nil {
		return TypeSchema{Type: "any"}
	}
	if seen[typ] {
		return TypeSchema{Type: "object"}
	}

	if typ == reflect.TypeOf(time.Time{}) {
		return TypeSchema{Type: "string", Format: "date-time"}
	}

	switch typ.Kind() {
	case reflect.Bool:
		return TypeSchema{Type: "boolean"}
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64,
		reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		return TypeSchema{Type: "integer"}
	case reflect.Float32, reflect.Float64:
		return TypeSchema{Type: "number"}
	case reflect.String:
		return TypeSchema{Type: "string"}
	case reflect.Slice, reflect.Array:
		itemSchema := schemaFromReflectTypeSeen(typ.Elem(), seen)
		return TypeSchema{Type: "array", Items: &itemSchema}
	case reflect.Map:
		valueSchema := schemaFromReflectTypeSeen(typ.Elem(), seen)
		return TypeSchema{Type: "object", AdditionalProperties: &valueSchema}
	case reflect.Interface:
		return TypeSchema{Type: "any"}
	case reflect.Struct:
		seen[typ] = true
		defer delete(seen, typ)
		fields := make([]SchemaField, 0, typ.NumField())
		for i := 0; i < typ.NumField(); i++ {
			field := typ.Field(i)
			if !field.IsExported() {
				continue
			}
			jsonName := jsonFieldName(field)
			if jsonName == "" {
				continue
			}
			fieldSchema := SchemaField{
				Name:     jsonName,
				Required: !strings.Contains(field.Tag.Get("json"), ",omitempty"),
				Schema:   schemaFromReflectTypeSeen(field.Type, seen),
			}
			fields = append(fields, fieldSchema)
		}
		return TypeSchema{Type: "object", Fields: fields}
	default:
		return TypeSchema{Type: strings.ToLower(typ.Kind().String())}
	}
}

func MergedFlagMetadata(flag *pflag.Flag) FlagMetadata {
	meta := FlagMetadataFor(flag)
	if !meta.Required {
		meta.Required = isRequiredFlag(flag)
	}
	if len(meta.Enum) == 0 {
		meta.Enum = inferEnumValues(flag.Usage)
	}
	return meta
}

func isRequiredFlag(flag *pflag.Flag) bool {
	if flag == nil || flag.Annotations == nil {
		return false
	}
	values, ok := flag.Annotations[cobra.BashCompOneRequiredFlag]
	return ok && len(values) > 0 && strings.EqualFold(values[0], "true")
}

func parseFlagDefault(flag *pflag.Flag) any {
	if flag == nil {
		return nil
	}
	raw := flag.DefValue
	switch flag.Value.Type() {
	case "bool":
		if v, err := strconv.ParseBool(raw); err == nil {
			return v
		}
	case "int", "int8", "int16", "int32", "int64":
		if v, err := strconv.ParseInt(raw, 10, 64); err == nil {
			return v
		}
	case "uint", "uint8", "uint16", "uint32", "uint64":
		if v, err := strconv.ParseUint(raw, 10, 64); err == nil {
			return v
		}
	case "float32", "float64":
		if v, err := strconv.ParseFloat(raw, 64); err == nil {
			return v
		}
	case "stringSlice":
		return parseStringSliceDefault(raw)
	}
	return raw
}

func parseStringSliceDefault(raw string) []string {
	raw = strings.TrimSpace(raw)
	if raw == "" || raw == "[]" {
		return []string{}
	}
	raw = strings.TrimPrefix(raw, "[")
	raw = strings.TrimSuffix(raw, "]")
	items := strings.Split(raw, ",")
	out := make([]string, 0, len(items))
	for _, item := range items {
		item = strings.TrimSpace(item)
		if item != "" {
			out = append(out, item)
		}
	}
	return out
}

func inferEnumValues(usage string) []string {
	start := strings.Index(usage, "(")
	end := strings.LastIndex(usage, ")")
	if start < 0 || end <= start {
		return nil
	}
	body := strings.TrimSpace(usage[start+1 : end])
	if body == "" {
		return nil
	}
	if strings.Contains(body, "|") {
		parts := strings.Split(body, "|")
		out := make([]string, 0, len(parts))
		for _, part := range parts {
			part = sanitizeEnumValue(part)
			if part != "" {
				out = append(out, part)
			}
		}
		if len(out) > 0 {
			return out
		}
	}
	if strings.Contains(body, "=") && strings.Contains(body, ",") {
		parts := strings.Split(body, ",")
		out := make([]string, 0, len(parts))
		for _, part := range parts {
			left, _, ok := strings.Cut(strings.TrimSpace(part), "=")
			left = sanitizeEnumValue(left)
			if ok && left != "" {
				out = append(out, left)
			}
		}
		if len(out) > 0 {
			return out
		}
	}
	return nil
}

func sanitizeEnumValue(raw string) string {
	value := strings.TrimSpace(raw)
	if value == "" {
		return ""
	}
	fields := strings.Fields(value)
	if len(fields) == 0 {
		return ""
	}
	return strings.TrimRight(fields[0], ",;.)]")
}

func splitSchemaEnum(raw string) []string {
	parts := strings.Split(raw, ",")
	out := make([]string, 0, len(parts))
	for _, part := range parts {
		part = strings.TrimSpace(part)
		if part != "" {
			out = append(out, part)
		}
	}
	return out
}

func fieldMetaFormat(field reflect.StructField, meta FlagMetadata) string {
	if format := strings.TrimSpace(field.Tag.Get("format")); format != "" {
		return format
	}
	return strings.TrimSpace(meta.Format)
}

func fieldMetaEnum(field reflect.StructField, meta FlagMetadata, flag *pflag.Flag) []string {
	if enumTag := strings.TrimSpace(field.Tag.Get("enum")); enumTag != "" {
		return splitSchemaEnum(enumTag)
	}
	if len(meta.Enum) > 0 {
		return append([]string(nil), meta.Enum...)
	}
	if flag != nil {
		return inferEnumValues(flag.Usage)
	}
	return nil
}

func jsonFieldName(field reflect.StructField) string {
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
