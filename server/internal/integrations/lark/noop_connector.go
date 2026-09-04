package lark

import (
	"context"
	"log/slog"
)

// NoopConnector satisfies EventConnector by holding the run context
// open without dialing anything and without emitting any inbound
// events. It is the explicit degraded connector used when the real
// Lark long-connection transport cannot be initialized from deployment
// configuration. The normal router path constructs WSLongConnConnector;
// keeping this fallback lets the lease lifecycle, supervisor / renewer
// goroutines and shutdown plumbing stay observable while the operator
// repairs the endpoint configuration. Inbound messages are intentionally
// dropped in this mode.
type NoopConnector struct {
	logger *slog.Logger
}

// NewNoopConnector returns a connector that blocks until the Hub
// cancels its run context. Logger may be nil; callers typically pass
// slog.Default.
func NewNoopConnector(logger *slog.Logger) *NoopConnector {
	if logger == nil {
		logger = slog.Default()
	}
	return &NoopConnector{logger: logger}
}

// Run blocks until ctx is cancelled and then returns nil. A nil return
// tells the Hub the connection ended cleanly (no backoff retry storm
// on shutdown / lease loss). Because this is a deliberate configuration
// fallback rather than a transport error, the Hub keeps the degraded
// state visible in its logs until the connector can be initialized.
func (c *NoopConnector) Run(ctx context.Context, inst Installation, _ EventEmitter) error {
	c.logger.Info("lark noop connector: holding lease (long-conn configuration unavailable)",
		"installation_id", uuidString(inst.ID),
		"app_id", inst.AppID,
	)
	<-ctx.Done()
	c.logger.Info("lark noop connector: exiting on ctx cancel",
		"installation_id", uuidString(inst.ID),
	)
	return nil
}

// NoopConnectorFactory returns a ConnectorFactory that hands every
// installation a NoopConnector sharing the supplied logger. It remains
// available for tests and explicit degraded deployments; production's
// normal router path uses the real shared WS long-connection connector.
func NoopConnectorFactory(logger *slog.Logger) ConnectorFactory {
	c := NewNoopConnector(logger)
	return func(_ Installation) (EventConnector, error) {
		return c, nil
	}
}
