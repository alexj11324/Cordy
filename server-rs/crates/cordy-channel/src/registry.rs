//! Type→Factory registry with last-writer-wins semantics.
//!
//! Adding a
//! platform is "register a factory here", never "edit the core". The
//! Registry is safe for concurrent use.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use thiserror::Error;

use crate::channel::{BuiltChannel, Channel, Config, Factory, Type};
use crate::{Capability, LeaseGeneration, OutboundMessage, SendResult};

/// Returned by [`Registry::build`] when no Factory is registered for the
/// requested Type. Callers can match on it.
///
/// Port note: Go uses a sentinel + `errors.Is` with `%w: %q` wrapping;
/// Rust carries the offending type in a typed error variant instead.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("channel: no factory registered for type {0:?}")]
pub struct UnknownTypeError(pub Type);

/// Maps a channel [`Type`] to the [`Factory`] that builds it.
///
/// Registration is last-writer-wins: registering a Type that already has
/// a Factory replaces it silently. This mirrors the plugin-registry
/// pattern from the reference design (MUL-3506) where the last adapter
/// to register a type wins, so a deployment can override a built-in
/// adapter by registering its own afterwards without a removal step.
#[derive(Default)]
pub struct Registry {
    // Port note: Go holds an RWMutex; Rust's RwLock suffices because the
    // Factory handle itself is an Arc (clone under read lock).
    factories: RwLock<HashMap<Type, Factory>>,
}

impl Registry {
    /// Returns an empty Registry ready for use.
    pub fn new() -> Self {
        Self::default()
    }

    /// Binds `factory` to `t`, replacing any factory previously
    /// registered for `t` (last-writer-wins). An empty Type or a missing
    /// factory is ignored — registering either would only set up a
    /// guaranteed failure at build time, so the Registry refuses to
    /// record it.
    ///
    /// Callers express "no factory" by not calling `register`; an empty type
    /// is rejected here as an invalid registration.
    pub fn register(&self, t: Type, factory: Factory) {
        if t.0.is_empty() {
            return;
        }
        self.factories.write().unwrap().insert(t, factory);
    }

    /// Returns the Factory registered for `t`, if one exists.
    pub fn lookup(&self, t: &Type) -> Option<Factory> {
        self.factories.read().unwrap().get(t).cloned()
    }

    /// Instantiates a Channel for `cfg.r#type` using the registered
    /// Factory. Returns [`UnknownTypeError`] when no Factory is
    /// registered, and otherwise returns whatever the Factory returns.
    pub async fn build(&self, cfg: Config) -> anyhow::Result<BuiltChannel> {
        let factory = self
            .lookup(&cfg.r#type)
            .ok_or_else(|| UnknownTypeError(cfg.r#type.clone()))?;
        let generation = cfg.generation.clone();
        let channel = factory(cfg).await?;
        Ok(match generation {
            Some(generation) => Arc::new(FencedChannel {
                channel,
                generation,
            }) as BuiltChannel,
            None => channel,
        })
    }

    /// Returns the registered types sorted lexicographically, so the
    /// result is stable across calls (map iteration order is not). Useful
    /// for diagnostics and for enumerating which platforms a deployment
    /// supports.
    pub fn types(&self) -> Vec<Type> {
        let mut out: Vec<Type> = self.factories.read().unwrap().keys().cloned().collect();
        out.sort();
        out
    }
}

struct FencedChannel {
    channel: BuiltChannel,
    generation: Arc<LeaseGeneration>,
}

#[async_trait::async_trait]
impl Channel for FencedChannel {
    fn r#type(&self) -> Type {
        self.channel.r#type()
    }

    async fn connect(&self, ctx: tokio_util::sync::CancellationToken) -> anyhow::Result<()> {
        self.generation.ensure_active()?;
        // The supervisor backs generation with this same run token. Await the
        // adapter so its cancellation cleanup runs instead of dropping the
        // connect future midway through sender/media teardown.
        self.channel.connect(ctx).await
    }

    async fn disconnect(&self) -> anyhow::Result<()> {
        self.channel.disconnect().await
    }

    async fn send(&self, out: OutboundMessage) -> anyhow::Result<SendResult> {
        self.generation.ensure_active()?;
        tokio::select! {
            biased;
            _ = self.generation.cancelled() => Err(crate::GenerationExpired.into()),
            result = self.channel.send(out) => result,
        }
    }

    fn capabilities(&self) -> Capability {
        self.channel.capabilities()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capability;
    use crate::message::OutboundMessage;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Minimal stub channel capturing what its factory saw.
    #[derive(Debug)]
    struct StubChannel(Type);
    #[async_trait::async_trait]
    impl crate::channel::Channel for StubChannel {
        fn r#type(&self) -> Type {
            self.0.clone()
        }
        async fn connect(&self, _ctx: tokio_util::sync::CancellationToken) -> anyhow::Result<()> {
            Ok(())
        }
        async fn disconnect(&self) -> anyhow::Result<()> {
            Ok(())
        }
        async fn send(&self, _out: OutboundMessage) -> anyhow::Result<crate::message::SendResult> {
            Ok(crate::message::SendResult::default())
        }
        fn capabilities(&self) -> Capability {
            Capability::TEXT
        }
    }

    fn stub_factory(seen_types: std::sync::Arc<AtomicUsize>) -> Factory {
        std::sync::Arc::new(move |cfg| {
            let seen = seen_types.clone();
            Box::pin(async move {
                seen.fetch_add(1, Ordering::SeqCst);
                Ok(std::sync::Arc::new(StubChannel(cfg.r#type)) as BuiltChannel)
            })
        })
    }

    #[tokio::test]
    async fn build_unknown_type_returns_typed_error() {
        let reg = Registry::new();
        let err = match reg
            .build(Config {
                r#type: Type("nosuch".to_string()),
                ..Default::default()
            })
            .await
        {
            Ok(_) => panic!("expected unknown-type error"),
            Err(err) => err,
        };
        assert_eq!(
            err.downcast_ref::<UnknownTypeError>(),
            Some(&UnknownTypeError(Type("nosuch".to_string())))
        );
        // Error text mirrors Go's sentinel wrapping shape.
        assert!(err.to_string().contains("nosuch"));
    }

    #[tokio::test]
    async fn build_uses_registered_factory_and_passes_config_through() {
        let reg = Registry::new();
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        reg.register(Type("feishu".to_string()), stub_factory(calls.clone()));
        let built = reg
            .build(Config {
                r#type: Type("feishu".to_string()),
                raw: json!({"app_id": "cli_a"}),
                id: Some(uuid::Uuid::nil()),
                handler: None,
                generation: None,
            })
            .await
            .unwrap();
        assert_eq!(built.r#type().0, "feishu");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn registration_is_last_writer_wins() {
        let reg = Registry::new();
        let first = std::sync::Arc::new(AtomicUsize::new(0));
        let second = std::sync::Arc::new(AtomicUsize::new(0));
        reg.register(Type("feishu".to_string()), stub_factory(first.clone()));
        reg.register(Type("feishu".to_string()), stub_factory(second.clone()));
        let f = reg.lookup(&Type("feishu".to_string())).unwrap();
        // Invoke and check only the SECOND factory runs.
        futures_lite_fallback(f);
        assert_eq!(first.load(Ordering::SeqCst), 0);
        assert_eq!(second.load(Ordering::SeqCst), 1);
    }

    fn futures_lite_fallback(f: Factory) {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async {
                f(Config {
                    r#type: Type("feishu".to_string()),
                    ..Default::default()
                })
                .await
                .unwrap();
            });
    }

    #[test]
    fn register_ignores_empty_type() {
        let reg = Registry::new();
        reg.register(
            Type(String::new()),
            stub_factory(std::sync::Arc::new(AtomicUsize::new(0))),
        );
        assert!(reg.types().is_empty());
    }

    #[test]
    fn types_sorted_lexicographically() {
        let reg = Registry::new();
        for name in ["wecom", "slack", "feishu"] {
            reg.register(
                Type(name.to_string()),
                stub_factory(std::sync::Arc::new(AtomicUsize::new(0))),
            );
        }
        let names: Vec<String> = reg.types().iter().map(|t| t.0.clone()).collect();
        assert_eq!(names, ["feishu", "slack", "wecom"]);
    }
}
