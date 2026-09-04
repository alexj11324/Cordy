package channel

import "context"

// RuntimeObservation describes a provider-confirmed connection, not merely an
// enabled installation or ownership lease. Only stable codes and safe literal
// summaries belong here; never pass provider errors or credential-bearing URLs.
type RuntimeObservation struct {
	State        string
	ErrorCode    string
	ErrorSummary string
}

type RuntimeReporter func(context.Context, RuntimeObservation) bool

type runtimeReporterKey struct{}

// WithRuntimeReporter scopes observation ownership to one Connect lifetime.
// Context propagation keeps the existing transport interfaces unchanged.
func WithRuntimeReporter(ctx context.Context, reporter RuntimeReporter) context.Context {
	return context.WithValue(ctx, runtimeReporterKey{}, reporter)
}

func ReportRuntime(ctx context.Context, observation RuntimeObservation) bool {
	reporter, _ := ctx.Value(runtimeReporterKey{}).(RuntimeReporter)
	return reporter != nil && reporter(ctx, observation)
}

func ReportConnected(ctx context.Context) bool {
	return ReportRuntime(ctx, RuntimeObservation{State: "healthy"})
}
