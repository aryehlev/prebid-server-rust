// Package gen generates parity cases: schema-aware OpenRTB requests for
// /openrtb2/auction, /openrtb2/amp and /openrtb2/video, and structured
// mutations of those requests and of every corpus case, with case IDs that
// regenerate each case exactly.
//
// # Case IDs
//
//	gen:<seed>:<index>:<base>[+<step>...]          built from scratch
//	<corpus case id>#mut:<seed>:<index>:<step>[+<step>...]  derived from a corpus case
//
// base is auction, amp or video. A step is name[@variant]. Generator.Case
// draws the chain; Generator.Regenerate replays an ID. Each base and each step
// draws from its own stream, Derive(seed, labels...), keyed by the case, the
// step name, its variant and how often the same step occurred earlier, so
// replaying an ID never depends on how the chain was drawn. A step whose
// precondition does not hold on replay is skipped with a note.
//
// # Determinism
//
// The RNG is PCG (math/rand/v2) with every bounded draw implemented in this
// package, JSON trees encode with sorted members, map iteration never feeds
// the RNG, and stored data is keyed by content hashes. The same checkout and
// toolchain therefore give byte-identical envelopes for the same ID. The
// gzip steps depend on compress/flate output, which a Go release may change.
//
// # Requests
//
// Base requests are built from the Go openrtb2 types: a skeleton Go validation
// accepts (one channel, valid media, imp.ext.prebid.bidder params) plus a
// reflection pass that sets random optional fields from parity/fixtures/pools.
// Bidders come from static/bidder-info (enabled bidders, their declared
// channels and media types); bidders the oracle answers with an OpenRTB
// template and that have known-valid params are preferred so auctions reach
// the exchange. Params are fixture examples or instances of the bidder's
// static/bidder-params schema that the production validator accepts; the
// near-miss mode breaks exactly one schema rule.
//
// # Steps
//
// Steps run in phases: feature steps (media, imps, bidders, aliases, currency,
// floors, multibid, targeting, cache, events, stored requests and responses,
// accounts, debug, tmax, privacy signals in OpenRTB 2.5 and 2.6 locations,
// seed grafts, mock outcomes, AMP query parameters, video pods), structural
// steps (drop, duplicate, retype, boundary numbers, unicode, null, empty,
// unknown fields, deep nesting), raw steps (truncation, trailing data,
// duplicate keys, escapes, BOM, invalid UTF-8, lone surrogates, alternate and
// oversized bodies), HTTP steps (method, headers, cookie, client address,
// query) and encoding steps (gzip). Every step declares what it does to a
// valid request (Preserves, Uncertain, Invalidates, Malformed); the worst one
// sets the case tag gen_valid, gen_uncertain, gen_invalid or gen_malformed.
// Mutators lists them.
//
// # Feedback
//
// Feedback reduces traces to coverage signatures and keeps cases that add new
// ones, weighting them by the deepest stage reached; Scheduler mixes fresh
// cases with children (Generator.Child) of kept cases.
package gen
