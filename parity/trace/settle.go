package trace

import "time"

// Track registers one goroutine of the case that can still change observable
// state after the handler has written its response, and returns the function
// to call when the goroutine finishes. The exchange's bidder goroutines are the
// example: each one sends its result to the auction and only then runs its
// deferred RecordAdapterRequest (exchange/exchange.go:808-810 and :865), so the
// adapter_requests counter can move after the response is complete. The exec
// handler calls Settle before it takes the closing metrics snapshot.
//
// Track must be called before the goroutine hands its result to the handler,
// so every registration happens before the handler returns. On a nil session
// it returns a no-op.
func (s *Session) Track() func() {
	if s == nil {
		return func() {}
	}
	s.pending.Add(1)
	return s.pending.Done
}

// Settle waits until every goroutine registered with Track has finished, or
// until timeout elapses. It reports whether they all finished; a nil session is
// always settled.
func (s *Session) Settle(timeout time.Duration) bool {
	if s == nil {
		return true
	}
	done := make(chan struct{})
	go func() {
		s.pending.Wait()
		close(done)
	}()
	timer := time.NewTimer(timeout)
	defer timer.Stop()
	select {
	case <-done:
		return true
	case <-timer.C:
		return false
	}
}
