# Execution Lane contract

PR1 gives every `agent_task_queue` row a deterministic, persisted
`execution_lane_key`. The key is routing metadata only; provider session and
work-directory continuity remain the task's persisted `session_id` and
`work_dir`, and no internal reasoning transcript is copied between runs.

The canonical forms are:

- `chat:<chat_session_id>` for direct or channel chat tasks.
- `issue:<issue_id>:agent:<agent_id>:main` for the issue's main task.
- `issue:<issue_id>:agent:<agent_id>:side:<side_thread_key>` for a side chat.
- `agent:<agent_id>:default` for unscoped agent work.

The side-thread key is the persisted `side_chat_root_comment_id`; malformed
side-chat rows that only retain `side_chat_parent_task_id` still remain in a
separate lane instead of silently falling back to the issue main lane.

`dispatched`, `running`, and `waiting_local_directory` are active writers.
Claim and deferred promotion use the lane key as their serialization guard.
`queued` is runnable work but is not an active writer; ordinary `deferred` is
also not an active writer because it must wait for its fire time or causal
parent. Existing channel-media pending rows remain a deliberate exception in
the enqueue/promotion dedupe path, and message-bus continuations remain
deferred until their main-task parent is terminal. Agent capacity is unchanged
and still counts the three active-writer states across all lanes. The partial
unique index is defense in depth: service claims already lock the Agent row
before checking capacity, while the index protects direct or concurrently
executing SQL paths. Migration 405 intentionally fails fast if pre-existing
active rows already violate the new invariant; it does not cancel or rewrite
work to make production data appear valid.

Retry, waiting, requeue, rerun, supersede, and coordinator-created child rows
must preserve the identity fields/context from which this key is generated.
Message-bus continuations are new main-lane turns and retain persisted
provider session/work-directory state until their parent terminal transaction
has committed. The next PRs have these concrete boundaries:

- PR2: add a per-lane sequence and FIFO causal gate; keep priority meaningful
  only when selecting between lanes, and define retry/deferred ordering rules.
- PR3: add generalized resource leases with read/write/control modes for
  repositories, devices, browsers, databases, and files; make acquisition and
  release transactional and observable.
- PR4: resolve mention-free channel follow-ups through the session-aware
  resolver, then apply invoke permission checks before enqueueing or steering.

None of those mechanisms are part of PR1.
