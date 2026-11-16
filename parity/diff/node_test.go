package diff

import (
	"testing"
)

func TestParseKeepsLexicalForm(t *testing.T) {
	tests := []struct {
		name   string
		in     string
		render string
		lead   string
		trail  string
	}{
		{"compact", `{"b":1.0,"a":[true,null,"x"]}`, `{"b":1.0,"a":[true,null,"x"]}`, "", ""},
		{"escapes kept", esc(`{"s":"^u0026^u00e9\n"}`), esc(`{"s":"^u0026^u00e9\n"}`), "", ""},
		{"escaped key", esc(`{"^u0061":1}`), esc(`{"^u0061":1}`), "", ""},
		{"whitespace around root", " \n{ \"a\" : 1 }\n", `{"a":1}`, " \n", "\n"},
		{"scalar root", "-0.5e10", "-0.5e10", "", ""},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			n, err := parseExact([]byte(tt.in))
			if err != nil {
				t.Fatal(err)
			}
			if got := string(render(n)); got != tt.render {
				t.Errorf("render = %s, want %s", got, tt.render)
			}
			if n.lead != tt.lead || n.trail != tt.trail || !n.exact {
				t.Errorf("lead %q trail %q exact %v", n.lead, n.trail, n.exact)
			}
		})
	}
}

func TestParseRejects(t *testing.T) {
	for _, in := range []string{"", "{", `{"a":1,}`, "[1] [2]", `"\x"`, "nul"} {
		if _, err := parseExact([]byte(in)); err == nil {
			t.Errorf("parseExact(%q) succeeded", in)
		}
	}
}

func TestCanonicalIgnoresSpelling(t *testing.T) {
	tests := []struct {
		a, b  string
		equal bool
	}{
		{`{"a":1,"b":[1,2]}`, `{"b":[1,2.0],"a":1e0}`, true},
		{`10E-1`, `1`, true},
		{esc(`"^u0026"`), `"&"`, true},
		{`0.1`, `0.10000000000000001`, false},
		{`[1,2]`, `[2,1]`, false},
		{`{"a":null}`, `{}`, false},
		{`""`, `null`, false},
	}
	for _, tt := range tests {
		a, err := parseExact([]byte(tt.a))
		if err != nil {
			t.Fatal(err)
		}
		b, err := parseExact([]byte(tt.b))
		if err != nil {
			t.Fatal(err)
		}
		if got := semanticEqual(a, b); got != tt.equal {
			t.Errorf("semanticEqual(%s, %s) = %v, want %v", tt.a, tt.b, got, tt.equal)
		}
	}
}

func TestStringSpellingClass(t *testing.T) {
	tests := []struct {
		g, r, want string
	}{
		{`"a&b"`, esc(`"a^u0026b"`), "html_escape"},
		{esc(`"^u003e"`), `">"`, "html_escape"},
		{esc(`"^u00e9"`), esc(`"^u00E9"`), "unicode_escape"},
		{`"a/b"`, `"a\/b"`, "unicode_escape"},
		{esc(`"^ud83d^ude00"`), esc(`"^uD83D^uDE00"`), "unicode_escape"},
		{`"same"`, `"same"`, ""},
	}
	for _, tt := range tests {
		if got := stringSpellingClass(tt.g, tt.r); got != tt.want {
			t.Errorf("stringSpellingClass(%s, %s) = %q, want %q", tt.g, tt.r, got, tt.want)
		}
	}
}

func TestTrailingClass(t *testing.T) {
	tests := []struct{ g, r, want string }{
		{"\n", "", "trailing_newline"},
		{"", "\n", "trailing_newline"},
		{" \n", " ", "trailing_newline"},
		{"\r\n", "\n", "whitespace"},
		{"\n", "\n", ""},
	}
	for _, tt := range tests {
		if got := trailingClass(tt.g, tt.r); got != tt.want {
			t.Errorf("trailingClass(%q, %q) = %q, want %q", tt.g, tt.r, got, tt.want)
		}
	}
}

func TestAlign(t *testing.T) {
	tests := []struct {
		name string
		g, r []string
		want []alignOp
	}{
		{"equal", []string{"a", "b"}, []string{"a", "b"}, []alignOp{{0, 0}, {1, 1}}},
		{"deletion", []string{"a", "b", "c"}, []string{"a", "c"}, []alignOp{{0, 0}, {1, -1}, {2, 1}}},
		{"insertion", []string{"a"}, []string{"x", "a"}, []alignOp{{-1, 0}, {0, 1}}},
		{"replacement pairs by position", []string{"a", "b"}, []string{"a", "c"}, []alignOp{{0, 0}, {1, 1}}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := align(tt.g, tt.r)
			if len(got) != len(tt.want) {
				t.Fatalf("align = %v, want %v", got, tt.want)
			}
			for i := range got {
				if got[i] != tt.want[i] {
					t.Fatalf("align = %v, want %v", got, tt.want)
				}
			}
		})
	}
}
