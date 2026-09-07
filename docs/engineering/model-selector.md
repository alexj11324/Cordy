# Model selector and discovery

The shared selector presents execution settings in
four columns: provider, model, thinking effort, and speed. Surfaces show a
compact trigger (provider icon, model, effort, speed, and a chevron) and open
the four-column picker in a popover. Concurrency remains a separate settings
card. Speed options
come from the selected model catalog. A provider without advertised tiers shows
an unavailable state; model-only entry points show inherited speed. Favorites preserve the runtime ID, model ID, thinking effort, and speed
together. The runtime ID distinguishes separate installations/accounts of the
same provider.

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
  Fast mode is advertised only for Opus 5 and Opus 4.8 when the installed
  Claude Code is ≥2.1.205; Execute writes `fastMode` into the task
  `--settings` JSON. Sonnet/Haiku keep an empty speed column — they do
  not have a native speed dial.
- ACP-based runtimes use `session/new` model/configuration responses. Several
  provider adapters have their own fallback or capability restrictions.
- Antigravity reads its native `agy models` command.
- Speed is currently implemented on the backend for Codex (`codex debug
  models`), Claude Code Fast mode, and for ACP runtimes whose Execute path applies
  `session/set_config_option` (reasonix, hermes/jcode, dim, kimi). Discovery
  is catalog-driven: a speed-like `configOptions` entry (id/name such as
  `fast_mode` / `fast-mode` / `service_tier`, or `model_config` whose copy
  talks about speed/fast — not context size) becomes `service_tiers` on the
  current model. The UI displays the speed column when that catalog is
  non-empty. Generic ACP boolean Fast-mode options require the client to
  advertise `session.configOptions.boolean: {}`; discovery and the
  ACP-execute initialize handshakes now do that. Copilot/Grok/CodeBuddy
  discover over ACP but execute through their own CLI, so they are not
  annotated.

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
`session.configOptions.boolean: {}`. Discovery and the ACP-execute
initialize handshakes advertise that capability so a boolean Fast-mode
option can appear in `session/new`. Speed-like options are mapped onto
`service_tiers` and applied with `session/set_config_option`, using the
same catalog-driven pattern as thinking effort. Codex keeps its bundled
`codex debug models` catalog rather than going through ACP.

`model_config` is also used for context size, so only options whose
id/name/description look like speed, fast, service tier, latency, or
priority are treated as the speed column.

[ACP v2](https://agentclientprotocol.com/protocol/v2/session-config-options)
renames the option identifier to `configId` and requires the value shape in
`session/set_config_option`. The existing daemon negotiates v1; v2 examples must
not be copied into its messages without implementing version negotiation.

The T3 Code visual adaptation and retained MIT license are documented in
[third-party/t3code.md](../../third-party/t3code.md).

Changing speed saves it with the model and thinking effort in one update.
Favorites store runtime, model, thinking effort, and speed together. Restoring
a favorite reapplies that exact speed when the catalog still advertises it;
legacy favorites without a stored speed keep a compatible speed on the same
runtime. Switching runtimes clears the previous speed override. Host-managed
runtimes (no model catalog) explain that the host chooses the model instead of
offering a fake "Default" row.
