package diff

import (
	"regexp"
	"strings"

	"github.com/xeipuuv/gojsonschema"
)

// schemaItem matches one complete gojsonschema result error as
// ResultErrorFields.String formats it with the default locale: "<field>:
// <description>". The descriptions are built from the library's own templates
// so the rule follows the version Go links.
var schemaItem = regexp.MustCompile(`^` + schemaField + `: (?:` + strings.Join(schemaDescriptions(), "|") + `)$`)

// schemaTemplates are the gojsonschema description templates that validation
// (not schema parsing) produces.
func schemaTemplates() []string {
	l := gojsonschema.DefaultLocale{}
	return []string{
		l.False(), l.Required(), l.InvalidType(), l.NumberAnyOf(), l.NumberOneOf(), l.NumberAllOf(),
		l.NumberNot(), l.MissingDependency(), l.Internal(), l.Const(), l.Enum(), l.ArrayNotEnoughItems(),
		l.ArrayNoAdditionalItems(), l.ArrayMinItems(), l.ArrayMaxItems(), l.Unique(), l.ArrayContains(),
		l.ArrayMinProperties(), l.ArrayMaxProperties(), l.AdditionalPropertyNotAllowed(),
		l.InvalidPropertyPattern(), l.InvalidPropertyName(), l.StringGTE(), l.StringLTE(),
		l.DoesNotMatchPattern(), l.DoesNotMatchFormat(), l.MultipleOf(), l.NumberGTE(), l.NumberGT(),
		l.NumberLTE(), l.NumberLT(), l.ConditionThen(), l.ConditionElse(),
	}
}

// Placeholder value patterns. A template that ends in a placeholder is
// followed directly by the next item's field, so those values must not be
// able to run into it: type names and numbers are matched exactly, enum lists
// as JSON scalars, and free-text names without spaces or colons.
const (
	schemaTypeName  = `(?:array|boolean|integer|null|number|object|string)`
	schemaTypeValue = `(?:\[` + schemaTypeName + `(?:,` + schemaTypeName + `)*\]|` + schemaTypeName + `|undefined)`
	schemaNumber    = `-?[0-9]+(?:\.[0-9]+)?(?:[eE][-+]?[0-9]+)?`
	schemaScalar    = `(?:"(?:[^"\\]|\\.)*"|` + schemaNumber + `|true|false|null)`
	schemaList      = schemaScalar + `(?:, ` + schemaScalar + `)*`
	schemaName      = `[^\s:]+`
)

// schemaPlaceholders maps each template placeholder to its value pattern.
var schemaPlaceholders = map[string]string{
	"expected": schemaTypeValue, "given": schemaTypeValue,
	"min": schemaNumber, "max": schemaNumber, "multiple": schemaNumber, "i": schemaNumber, "j": schemaNumber,
	"allowed": schemaList,
	"field":   schemaName, "property": schemaName, "dependency": schemaName, "type": schemaName,
}

var schemaPlaceholder = regexp.MustCompile(`\\\{\\\{\\\.([a-z]+)\\\}\\\}`)

// schemaQuoted matches a placeholder the template encloses in quotes, such as
// "Does not match pattern '{{.pattern}}'". Its value cannot contain the quote:
// otherwise "a: Does not match pattern 'x'b: Does not match pattern 'y'" would
// also be one item whose pattern is "x'b: Does not match pattern 'y", and the
// list would not split one way only.
var schemaQuoted = regexp.MustCompile(`(['"])\\\{\\\{\\\.[a-z]+\\\}\\\}(['"])`)

// schemaDescriptions converts the templates into regular expressions.
// Quoted placeholders match anything but their quote; other placeholders
// without a dedicated pattern (error) are enclosed by literal text in every
// template, so they match lazily.
func schemaDescriptions() []string {
	out := make([]string, 0, len(schemaTemplates()))
	for _, t := range schemaTemplates() {
		quoted := schemaQuoted.ReplaceAllStringFunc(regexp.QuoteMeta(t), func(m string) string {
			q := schemaQuoted.FindStringSubmatch(m)
			if q[1] != q[2] {
				return m
			}
			return q[1] + `[^` + q[1] + `]*` + q[2]
		})
		out = append(out, schemaPlaceholder.ReplaceAllStringFunc(quoted, func(m string) string {
			name := schemaPlaceholder.FindStringSubmatch(m)[1]
			if p, ok := schemaPlaceholders[name]; ok {
				return p
			}
			return `.+?`
		}))
	}
	return out
}
