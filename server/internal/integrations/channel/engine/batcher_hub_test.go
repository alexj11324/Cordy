package engine

import (
	"context"
	"errors"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

func TestHubFlushKeepsOtherChatsAndRunsEachContextOnce(t *testing.T) {
	factory := &fakeTimerFactory{}
	batcher := newTestBatcher(factory)
	var first, second, unrelated atomic.Int32
	batcher.Schedule("chat:1", func() { first.Add(1) })
	batcher.Schedule("chat:2", func() { second.Add(1) })
	batcher.Schedule("other:1", func() { unrelated.Add(1) })
	stale := factory.all[0]
	if err := batcher.FlushPrefix(context.Background(), "chat:"); err != nil {
		t.Fatal(err)
	}
	if first.Load() != 1 || second.Load() != 1 || unrelated.Load() != 0 || batcher.pendingCount() != 1 {
		t.Fatal("Hub flush dropped a context or flushed an unrelated conversation")
	}
	stale.fn()
	if first.Load() != 1 {
		t.Fatal("the old timer duplicated a context after the Hub flush")
	}
	factory.fireArmed()
	if unrelated.Load() != 1 {
		t.Fatal("the unrelated Chat lost its own debounce window")
	}
}

// Observing Done proves FlushPrefix has reached its wait, without sleeping or
// assuming the scheduler has run the flushing goroutine within an interval.
type observedWaitContext struct {
	context.Context
	once sync.Once
	seen chan struct{}
}

func (c *observedWaitContext) Done() <-chan struct{} {
	c.once.Do(func() { close(c.seen) })
	return c.Context.Done()
}

func TestHubFlushWaitsForAnAlreadyFiringTimer(t *testing.T) {
	factory := &fakeTimerFactory{}
	batcher := newTestBatcher(factory)
	started, release := make(chan struct{}), make(chan struct{})
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	batcher.Schedule("chat:1", func() { close(started); <-release })
	go factory.all[0].fn()
	select {
	case <-started:
	case <-ctx.Done():
		close(release)
		t.Fatal("timer never started")
	}
	observed := &observedWaitContext{Context: ctx, seen: make(chan struct{})}
	finished := make(chan error, 1)
	go func() { finished <- batcher.FlushPrefix(observed, "chat:") }()
	select {
	case <-observed.seen:
	case <-ctx.Done():
		close(release)
		t.Fatal("Hub flush did not observe the in-flight context")
	}
	select {
	case <-finished:
		close(release)
		t.Fatal("Agent switch passed a timer whose old-Agent enqueue was still running")
	default:
	}
	close(release)
	if err := <-finished; err != nil {
		t.Fatal(err)
	}
	batcher.FlushAll()
}

func TestCancelledHubFlushDoesNotCancelTheOldRun(t *testing.T) {
	factory := &fakeTimerFactory{}
	batcher := newTestBatcher(factory)
	started, release := make(chan struct{}), make(chan struct{})
	batcher.Schedule("chat:1", func() { close(started); <-release })
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	finished := make(chan error, 1)
	go func() { finished <- batcher.FlushPrefix(ctx, "chat:") }()
	select {
	case <-started:
	case <-ctx.Done():
		close(release)
		t.Fatal("flush never started the old run")
	}
	cancel()
	if err := <-finished; !errors.Is(err, context.Canceled) {
		close(release)
		t.Fatalf("cancelled switch error = %v", err)
	}
	close(release)
	batcher.FlushAll()
}
