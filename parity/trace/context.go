package trace

import "context"

type bidderKey struct{}

// WithBidder returns a context that attributes outbound calls and records made
// under it to bidder. The exchange wraps each bidder's request context with it,
// which is how the parity transport labels a call as kind "bidder".
func WithBidder(ctx context.Context, bidder string) context.Context {
	return context.WithValue(ctx, bidderKey{}, bidder)
}

// BidderFrom returns the bidder set by WithBidder.
func BidderFrom(ctx context.Context) (string, bool) {
	if ctx == nil {
		return "", false
	}
	bidder, ok := ctx.Value(bidderKey{}).(string)
	return bidder, ok && bidder != ""
}
