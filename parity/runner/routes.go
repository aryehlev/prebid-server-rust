package runner

import (
	"fmt"
	"sort"
	"strings"
)

// Route matching of the exec dispatch check.
//
// A health document lists the main router's product routes (Health.Endpoints)
// as httprouter v1.3.0 patterns, the way router/router.go registers them:
// "/info/bidders/:bidderName" or "/static/*filepath". Exec dispatches every
// request to the main router, so a side can answer any path: one a route
// matches through its handler, and any other one with the router's own
// answer, which is what the main port sends (httprouter.New's
// RedirectTrailingSlash, RedirectFixedPath, HandleMethodNotAllowed and
// HandleOPTIONS: a 404, a 405 with Allow, a redirect or an OPTIONS answer).
// The check only keeps out what one side cannot answer as the other would
// because it lacks a route.

// routeMatch reports whether path matches the httprouter v1.3.0 pattern, as
// a lookup in the router's tree finds the pattern's handle (tree.go
// getValue): static text matches itself; ":name" matches the bytes up to the
// next "/" or the end of the path, at least one when the parameter ends the
// pattern; "*name" ends a pattern and matches the rest of the path, empty
// included. The method is not matched.
func routeMatch(pattern, path string) bool {
	for {
		i := strings.IndexAny(pattern, ":*")
		if i < 0 {
			return path == pattern
		}
		if !strings.HasPrefix(path, pattern[:i]) {
			return false
		}
		path, pattern = path[i:], pattern[i:]
		if pattern[0] == '*' {
			return true
		}
		value := strings.IndexByte(path, '/')
		if value < 0 {
			value = len(path)
		}
		name := strings.IndexByte(pattern, '/')
		if name < 0 {
			return value == len(path) && value > 0
		}
		path, pattern = path[value:], pattern[name:]
	}
}

// matchingRoute returns the first route that path matches, or "".
func matchingRoute(routes []string, path string) string {
	for _, r := range routes {
		if routeMatch(r, path) {
			return r
		}
	}
	return ""
}

// routeSupport returns why the pair cannot compare a request to path, or ""
// when it can. A side whose health document lists no routes serves every
// route.
//
//   - When a route of each side matches path, both handlers answer it.
//   - When a route of one side matches path and no route of the other does,
//     the other side does not serve that route yet: not-yet-instrumented.
//   - When no route matches path, both routers answer it themselves. Their
//     answer depends on the routes they have (a redirect names the route
//     httprouter found near the path, a 405 lists the methods of the path's
//     route), so the path is compared only when both sides list the same
//     routes.
func routeSupport(sides [2]*side, path string) string {
	var hit [2]string
	for i, s := range sides {
		routes := s.srv.Health.Endpoints
		if len(routes) == 0 {
			hit[i] = "no endpoints, so every route"
			continue
		}
		hit[i] = matchingRoute(routes, path)
	}
	switch {
	case hit[0] != "" && hit[1] != "":
		return ""
	case hit[0] == "" && hit[1] == "":
		if diff := routeDifference(sides); diff != "" {
			return fmt.Sprintf("no route matches %s, and %s: the routers' own answers (404, 405 or a redirect) can differ for that alone", path, diff)
		}
		return ""
	}
	lacking, serving := sides[0], sides[1]
	route := hit[1]
	if hit[0] != "" {
		lacking, serving, route = sides[1], sides[0], hit[0]
	}
	return fmt.Sprintf("%s (%s) exec cannot dispatch to %s: it lists no route matching it, and %s (%s) lists %s",
		lacking.impl, lacking.name, path, serving.impl, serving.name, route)
}

// routeDifference describes the routes only one side lists, or returns ""
// when both list the same set.
func routeDifference(sides [2]*side) string {
	var parts []string
	for i, s := range sides {
		if only := missingFrom(s.srv.Health.Endpoints, sides[1-i].srv.Health.Endpoints); len(only) > 0 {
			parts = append(parts, fmt.Sprintf("only %s (%s) lists %s", s.impl, s.name, strings.Join(only, ", ")))
		}
	}
	return strings.Join(parts, "; ")
}

// missingFrom returns the routes of list that other does not hold, sorted
// and without repeats.
func missingFrom(list, other []string) []string {
	have := map[string]bool{}
	for _, r := range other {
		have[r] = true
	}
	var out []string
	for _, r := range list {
		if !have[r] {
			have[r] = true
			out = append(out, r)
		}
	}
	sort.Strings(out)
	return out
}
