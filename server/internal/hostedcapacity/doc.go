// Package hostedcapacity enforces the managed deployment's per-workspace cap
// on concurrent hosted channel installations (Cloud entitlement gate
// im_installation_limit). It is the Go port of the Rust hosted installation
// capacity feature.
//
// Over-capacity installations are PAUSED, never revoked: their status stays
// 'installed', credentials and bindings survive, and every work-finding query
// (WebSocket lease acquisition, inbound routing, supervisor enumeration)
// filters on channel_installation.hosted_paused_at IS NULL. Reconcile is the
// only writer of the pause marker and runs under the same workspace row lock
// as admission, so a subscription change and a concurrent install cannot
// interleave.
//
// A zero/disabled resolver changes nothing: self-hosted deployments never
// construct it. An enabled resolver whose Cloud policy cannot be trusted
// (unreachable, absent gate, invalid) fails CLOSED — installs answer 503
// rather than silently consuming capacity the host cannot afford.
package hostedcapacity
