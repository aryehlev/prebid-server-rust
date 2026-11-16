package diff

import (
	"regexp"
	"strconv"
	"strings"
)

// Root is the JSON path of a compared value.
const Root = "$"

// Meta members address parts of a stage entry or HTTP message that are not
// inside its JSON value. They use an '@' prefix so they cannot collide with a
// member of the value itself: $['@errors'][0].code, $['@headers']['Content-Type'].
const (
	MetaErrors   = "@errors"
	MetaWarnings = "@warnings"
	MetaWrites   = "@writes"
	MetaSeq      = "@seq"
	MetaRaw      = "@raw"
	MetaStatus   = "@status"
	MetaHeaders  = "@headers"
	MetaMethod   = "@method"
	MetaPath     = "@path"
	MetaQuery    = "@query"
	MetaBody     = "@body"
)

var identifier = regexp.MustCompile(`^[A-Za-z_][A-Za-z0-9_]*$`)

// child appends an object member to a JSON path. Identifier-like names use dot
// notation; everything else uses ['name'] with \ and ' escaped.
func child(path, key string) string {
	if identifier.MatchString(key) {
		return path + "." + key
	}
	return path + "['" + escapeKey(key) + "']"
}

// meta appends a meta member (always bracketed) to a JSON path.
func meta(path, key string) string {
	return path + "['" + escapeKey(key) + "']"
}

// index appends an array index to a JSON path.
func index(path string, i int) string {
	return path + "[" + strconv.Itoa(i) + "]"
}

func escapeKey(key string) string {
	key = strings.ReplaceAll(key, `\`, `\\`)
	return strings.ReplaceAll(key, `'`, `\'`)
}

var arrayIndex = regexp.MustCompile(`\[[0-9]+\]`)

// PathPattern replaces every array index in a JSON path with [*], which is the
// path form used in mismatch signatures: $.imp[3].ext -> $.imp[*].ext.
func PathPattern(jsonPath string) string {
	return arrayIndex.ReplaceAllString(jsonPath, "[*]")
}

// pathSegments splits a JSON path into member names and "*" for indices, for
// the categorization rules: $.imp[0]['a b'].x -> [imp * a b x].
func pathSegments(jsonPath string) []string {
	var out []string
	s := strings.TrimPrefix(jsonPath, Root)
	for len(s) > 0 {
		switch {
		case strings.HasPrefix(s, "."):
			end := strings.IndexAny(s[1:], ".[")
			if end < 0 {
				out = append(out, s[1:])
				return out
			}
			out = append(out, s[1:end+1])
			s = s[end+1:]
		case strings.HasPrefix(s, "['"):
			name, rest := bracketKey(s[2:])
			out = append(out, name)
			s = rest
		case strings.HasPrefix(s, "["):
			end := strings.IndexByte(s, ']')
			if end < 0 {
				return out
			}
			out = append(out, "*")
			s = s[end+1:]
		default:
			return out
		}
	}
	return out
}

// bracketKey reads an escaped name up to the closing '] and returns it with
// the remainder of the path.
func bracketKey(s string) (string, string) {
	var b strings.Builder
	for i := 0; i < len(s); i++ {
		switch {
		case s[i] == '\\' && i+1 < len(s):
			i++
			b.WriteByte(s[i])
		case s[i] == '\'' && i+1 < len(s) && s[i+1] == ']':
			return b.String(), s[i+2:]
		default:
			b.WriteByte(s[i])
		}
	}
	return b.String(), ""
}
