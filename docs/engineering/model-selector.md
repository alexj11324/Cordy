# Model selector and discovery

The shared selector presents runtime providers, models, and thinking effort in
three columns. Agent settings embed it directly; speed (when supported) and
concurrency are separate settings cards. Favorites preserve the runtime ID,
model ID, and thinking effort together. The runtime ID distinguishes separate
installations/accounts of the same provider.

## Entry points

| Entry                           | Shared component                        | Thinking override                                                               |
| ------------------------------- | --------------------------------------- | ------------------------------------------------------------------------------- |
| Agent general settings          | ModelPicker → ModelDropdown             | Saved with model/runtime in one PATCH                                           |
| Manual create / duplicate page  | AgentConfigurationPanel → ModelDropdown | Saved in the agent draft                                                        |
| AI builder setup                | BuilderSetup → ModelDropdown            | Saved in the agent draft                                                        |
| AI builder live configuration   | AgentConfigurationPanel → ModelDropdown | Runtime rebind completes before draft changes                                   |
| Quick create / duplicate dialog | CreateAgentDialog → ModelDropdown       | Saved in the create request                                                     |
| Desktop onboarding              | PatrickRuntimeChoice → ModelDropdown    | Existing bootstrap API accepts model only                                       |
| Web CLI onboarding              | PatrickRuntimeChoice → ModelDropdown    | Existing bootstrap API accepts model only                                       |
| Runtimes-page assistant setup   | PatrickRuntimeChoice → ModelDropdown    | Existing bootstrap API accepts model only                                       |
| Automation instructions         | ModelDropdown                           | Existing automation API overrides model only; effort is inherited from executor |

Model-only entries explain their inherited effort and only offer favorites that
match it. They never silently drop a different effort selected from a favorite.
Antigravity's discovered High/Medium/Low variants are grouped in the UI, while
requests still carry the original runtime-native model ID.

Chat and task pages select agents rather than configuring another model picker.
Desktop and web consume these same shared views. No separate mobile model picker
was found in the source inventory.

## Discovery audit (2026-09-06)

The frontend reads `runtimeModelsOptions`, which requests discovery through the
server and daemon. The frontend does not define a model catalog. The shared picker passes the
runtime's workspace ID through both the initial request and every poll, overriding
the global workspace slug so route initialization or later navigation cannot
strip or redirect discovery. Runtime UUIDs remain the catalog cache identity.

Selected rows use shadcn accent tokens, an inset border, and checkmarks. Manual
refresh spins while fetching and finishes its current revolution even when a
cached response is immediate; reduced-motion preferences disable rotation.

- Codex currently invokes `codex debug models --bundled`. This is the installed
  CLI's bundled snapshot, with a static fallback in `server/pkg/agent/models.go`.
- Claude currently uses a static model list with CLI capability annotation.
- ACP-based runtimes use `session/new` model/configuration responses. Several
  provider adapters have their own fallback or capability restrictions.
- Antigravity reads its native `agy models` command.
- Speed is currently implemented on the backend for Codex. The UI displays the
  speed row only when the returned catalog supports it or an existing override
  needs to be cleared. This does not imply other providers lack speed features.

The local GPT-6 omission and config parse failure were traced to terminal Codex
0.152.0 while the desktop Codex bundle was 0.153.4. After upgrading the standalone
CLI to the verified official 0.153.4 package, the unchanged experimental context
configuration loaded and both the CLI and Orvilo runtime catalog returned
`gpt-6-astra`. This machine-level upgrade is separate from this repository diff.

## ACP findings and remaining adapter work

[ACP v1 session config options](https://agentclientprotocol.com/protocol/v1/session-config-options)
provides `model`, `thought_level`, and `model_config` categories. Model-related
configuration can include speed tradeoffs. A category is placement metadata,
not proof that every model_config field represents speed. Config IDs and values
must be preserved, and responses to `session/set_config_option` replace the full
configuration state because options can depend on the selected model.

Boolean options in v1 require the client to advertise
`session.configOptions.boolean: {}`. The current generic ACP discovery handshake
does not advertise that capability, so it cannot establish that a provider has
no boolean Fast-mode option. Generic ACP speed discovery and application are
not implemented by this UI change. Nor does this change replace Codex's bundled
catalog strategy with account-refreshed discovery.

[ACP v2](https://agentclientprotocol.com/protocol/v2/session-config-options)
renames the option identifier to `configId` and requires the value shape in
`session/set_config_option`. The existing daemon negotiates v1; v2 examples must
not be copied into its messages without implementing version negotiation.

The T3 Code visual adaptation and retained MIT license are documented in
[third-party/t3code.md](../../third-party/t3code.md).
