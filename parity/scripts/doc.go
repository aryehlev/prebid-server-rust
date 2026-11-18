// Package scripts holds the tests of the parity shell scripts in this
// directory. It has no code of its own.
//
// It is a Go module of its own, with no requirements, and its tests run from
// this directory (go test ./...): build-go.sh leaves parity/scripts out of the
// tree it assembles, so the scripts and their tests never compile or run
// inside it.
package scripts
