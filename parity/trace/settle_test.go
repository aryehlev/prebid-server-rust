package trace

import (
	"testing"
	"time"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/stretchr/testify/assert"
)

func TestSettle(t *testing.T) {
	tests := []struct {
		name    string
		tracked int
		release bool
		timeout time.Duration
		want    bool
	}{
		{name: "nothing tracked", tracked: 0, timeout: time.Millisecond, want: true},
		{name: "tracked goroutines finish", tracked: 3, release: true, timeout: 5 * time.Second, want: true},
		{name: "a goroutine outlives the timeout", tracked: 2, release: false, timeout: 20 * time.Millisecond, want: false},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			s := newSession(&parity.Envelope{CaseID: tt.name})
			release := make(chan struct{})
			for i := 0; i < tt.tracked; i++ {
				done := s.Track()
				go func() {
					<-release
					done()
				}()
			}
			if tt.release {
				close(release)
			}
			assert.Equal(t, tt.want, s.Settle(tt.timeout))
			if !tt.release {
				close(release)
				assert.True(t, s.Settle(5*time.Second), "settles once the goroutines finish")
			}
		})
	}
}

func TestSettleNilSession(t *testing.T) {
	var s *Session
	done := s.Track()
	done()
	assert.True(t, s.Settle(time.Millisecond))
}

func TestSettleWaitsForDeferredWork(t *testing.T) {
	s := newSession(&parity.Envelope{CaseID: "deferred"})
	results := make(chan int, 1)
	recorded := make(chan struct{}, 1)
	go func() {
		defer s.Track()()
		defer func() {
			time.Sleep(10 * time.Millisecond)
			recorded <- struct{}{}
		}()
		results <- 1
	}()
	<-results
	assert.True(t, s.Settle(5*time.Second))
	select {
	case <-recorded:
	default:
		t.Fatal("Settle returned before the deferred work ran")
	}
}
