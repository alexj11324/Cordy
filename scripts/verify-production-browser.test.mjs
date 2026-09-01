import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import {
  buildProductionSmokeDependencyPlan,
  buildProductionSmokeGraphIdempotencyKey,
  buildGoogleOAuthProbeUrl,
  decodeClerkFrontendApi,
  findProductionSmokeGraph,
  findProductionSmokeParentIssue,
  isExpectedBrowserRequestCancellation,
  PRODUCTION_SMOKE_DEPENDENT_ACCEPTANCE,
  PRODUCTION_SMOKE_DEPENDENT_TASK_TITLE,
  PRODUCTION_SMOKE_FIRST_TASK_TITLE,
  PRODUCTION_SMOKE_GRAPH_GOAL,
  PRODUCTION_SMOKE_PARENT_TITLE,
  PRODUCTION_SMOKE_SECOND_TASK_TITLE,
  requireBrowserReceipt,
  requireGoogleOAuthNavigation,
  requireNoDefaultExecutionAgent,
  requireProductionSmokeGraph,
  requireProductionSmokeGraphContract,
  requireProtectedNavigation,
} from "./verify-production-browser-contract.mjs";

const SOURCE_SHA = "a".repeat(40);
const EXPECTED_BUILD = `sha-${SOURCE_SHA}`;
const browserVerifierSource = await readFile(
  new URL("./verify-production-browser.mjs", import.meta.url),
  "utf8",
);

function productionSmokeGraph() {
  return {
    plan: { goal: PRODUCTION_SMOKE_GRAPH_GOAL },
    nodes: [
      {
        temp_id: "task-1",
        title: PRODUCTION_SMOKE_FIRST_TASK_TITLE,
        executor_type: null,
        executor_id: null,
        acceptance_criteria: ["first criterion"],
        issue: { identifier: "SMOKE-2" },
      },
      {
        temp_id: "task-2",
        title: PRODUCTION_SMOKE_SECOND_TASK_TITLE,
        executor_type: null,
        executor_id: null,
        acceptance_criteria: ["second criterion"],
        issue: { identifier: "SMOKE-3" },
      },
      {
        temp_id: "task-3",
        title: PRODUCTION_SMOKE_DEPENDENT_TASK_TITLE,
        executor_type: null,
        executor_id: null,
        acceptance_criteria: [PRODUCTION_SMOKE_DEPENDENT_ACCEPTANCE],
        issue: { identifier: "SMOKE-4" },
      },
    ],
    edges: [
      {
        from: "task-1",
        to: "task-3",
        type: "hard",
        consumed_output: "First prerequisite output",
        satisfied: false,
      },
      {
        from: "task-2",
        to: "task-3",
        type: "hard",
        consumed_output: "Second prerequisite output",
        satisfied: true,
      },
    ],
  };
}

test("extracts short-lived browser credentials only from the matching receipt", () => {
  assert.deepEqual(
    requireBrowserReceipt(
      {
        ok: true,
        action: "deploy",
        source_sha: SOURCE_SHA,
        browser_auth: {
          sign_in_ticket: "ticket-value",
          testing_token: "testing-value",
        },
      },
      SOURCE_SHA,
    ),
    { signInTicket: "ticket-value", testingToken: "testing-value" },
  );
  assert.throws(
    () =>
      requireBrowserReceipt(
        { ok: true, action: "deploy", source_sha: "b".repeat(40) },
        SOURCE_SHA,
      ),
    /does not match/u,
  );
});

test("a login redirect is not protected-page acceptance", () => {
  assert.throws(
    () =>
      requireProtectedNavigation({
        url: "https://patchbay.aspectlylabs.com/login",
        status: 307,
        actualBuild: EXPECTED_BUILD,
        expectedBuild: EXPECTED_BUILD,
        expectedPath: "/production-smoke/task-graph",
      }),
    /HTTP 307/u,
  );
});

test("protected acceptance requires the exact route and deployed Web build", () => {
  assert.doesNotThrow(() =>
    requireProtectedNavigation({
      url: "https://patchbay.aspectlylabs.com/production-smoke/issues",
      status: 200,
      actualBuild: EXPECTED_BUILD,
      expectedBuild: EXPECTED_BUILD,
      expectedPath: "/production-smoke/issues",
    }),
  );
  assert.throws(
    () =>
      requireProtectedNavigation({
        url: "https://patchbay.aspectlylabs.com/production-smoke/issues",
        status: 200,
        actualBuild: "sha-old",
        expectedBuild: EXPECTED_BUILD,
        expectedPath: "/production-smoke/issues",
      }),
    /sha-old/u,
  );
});

test("decodes the Clerk Frontend API host without exposing another secret", () => {
  const encoded = Buffer.from("clerk.example.test$", "utf8").toString("base64");
  assert.equal(
    decodeClerkFrontendApi(`pk_live_${encoded}`),
    "clerk.example.test",
  );
  assert.throws(() => decodeClerkFrontendApi("invalid"), /invalid format/u);
});

test("builds a valid desktop OAuth handoff and requires downstream navigation", () => {
  const url = new URL(
    buildGoogleOAuthProbeUrl({
      codeChallenge: "a".repeat(43),
      state: "b".repeat(43),
    }),
  );
  assert.equal(url.origin, "https://accounts.aspectlylabs.com");
  assert.equal(url.pathname, "/oauth/google");
  assert.equal(url.searchParams.get("platform"), "desktop");
  assert.equal(url.searchParams.get("code_challenge"), "a".repeat(43));
  assert.equal(url.searchParams.get("state"), "b".repeat(43));
  assert.throws(
    () => requireGoogleOAuthNavigation(url.href),
    /did not reach accounts\.google\.com/u,
  );
  assert.throws(
    () => requireGoogleOAuthNavigation("https://example.com/oauth"),
    /did not reach accounts\.google\.com/u,
  );
  assert.equal(
    requireGoogleOAuthNavigation("https://accounts.google.com/o/oauth2/auth")
      .hostname,
    "accounts.google.com",
  );
  assert.throws(
    () =>
      buildGoogleOAuthProbeUrl({
        codeChallenge: "too-short",
        state: "b".repeat(43),
      }),
    /valid desktop handoff/u,
  );
});

test("reads the settled page URL after Playwright waitForURL", () => {
  assert.match(
    browserVerifierSource,
    /await downstreamNavigation;\n\s+requireGoogleOAuthNavigation\(page\.url\(\)\);/u,
  );
  assert.doesNotMatch(browserVerifierSource, /downstream\.href/u);
});

test("ignores expected Chromium navigation cancellations only", () => {
  assert.equal(isExpectedBrowserRequestCancellation("net::ERR_ABORTED"), true);
  assert.equal(isExpectedBrowserRequestCancellation("net::ERR_FAILED"), false);
  assert.equal(isExpectedBrowserRequestCancellation(undefined), false);
  assert.match(
    browserVerifierSource,
    /if \(isExpectedBrowserRequestCancellation\(failure\?\.errorText\)\) return;/u,
  );
});

test("reports captured route failures when a protected landmark never renders", () => {
  assert.match(
    browserVerifierSource,
    /did not render \$\{landmark\}:\\n\$\{details\.join\("\\n"\)\}/u,
  );
  assert.match(browserVerifierSource, /visible alert: \$\{alert\}/u);
  assert.match(
    browserVerifierSource,
    /message\.type\(\) === "warning"[\s\S]*API response failed schema validation/u,
  );
});

test("builds the production three-task, two-edge dependency fixture", () => {
  const parentIssueId = "11111111-1111-4111-8111-111111111111";
  const plan = buildProductionSmokeDependencyPlan(parentIssueId);
  assert.equal(plan.parent_issue_id, parentIssueId);
  assert.equal(plan.tasks.length, 3);
  assert.equal(plan.edges.length, 2);
  assert.deepEqual(
    plan.edges.map((edge) => [edge.from, edge.to]),
    [
      ["task-1", "task-3"],
      ["task-2", "task-3"],
    ],
  );
  const dependent = plan.tasks.find((task) => task.temp_id === "task-3");
  assert.equal(dependent?.title, PRODUCTION_SMOKE_DEPENDENT_TASK_TITLE);
  assert.deepEqual(dependent?.acceptance_criteria, [
    PRODUCTION_SMOKE_DEPENDENT_ACCEPTANCE,
  ]);
  for (const edge of plan.edges) {
    const source = plan.tasks.find((task) => task.temp_id === edge.from);
    assert.ok(source?.outputs.includes(edge.consumed_output));
  }
});

test("rotates the graph apply idempotency key for each fixture incarnation", () => {
  const parentIssueId = "11111111-1111-4111-8111-111111111111";
  const first = buildProductionSmokeGraphIdempotencyKey(
    parentIssueId,
    "a".repeat(32),
  );
  const second = buildProductionSmokeGraphIdempotencyKey(
    parentIssueId,
    "b".repeat(32),
  );
  assert.notEqual(first, second);
  assert.equal(first, `production-smoke-${parentIssueId}-${"a".repeat(32)}`);
  assert.throws(
    () => buildProductionSmokeGraphIdempotencyKey(parentIssueId, "too-short"),
    /32 hexadecimal/u,
  );
});

test("finds an existing smoke graph across every cursor page", async () => {
  const expected = productionSmokeGraph();
  const cursors = [];
  const found = await findProductionSmokeGraph(async (cursor) => {
    cursors.push(cursor);
    if (cursor === null) return { graphs: [], next_cursor: "second-page" };
    return { graphs: [expected], next_cursor: null };
  });
  assert.equal(found, expected);
  assert.deepEqual(cursors, [null, "second-page"]);

  await assert.rejects(
    () =>
      findProductionSmokeGraph(async () => ({
        graphs: [],
        next_cursor: "repeated",
      })),
    /repeated cursor/u,
  );
});

test("reuses only the stable top-level smoke parent", () => {
  const parent = {
    id: "parent",
    title: PRODUCTION_SMOKE_PARENT_TITLE,
    parent_issue_id: null,
  };
  assert.equal(
    findProductionSmokeParentIssue({
      issues: [
        { ...parent, id: "child", parent_issue_id: "other" },
        { ...parent, title: "Different title" },
        parent,
      ],
    }),
    parent,
  );
  assert.equal(findProductionSmokeParentIssue({ issues: [] }), null);
});

test("refuses to apply a smoke graph when a default executor can run it", () => {
  assert.doesNotThrow(() =>
    requireNoDefaultExecutionAgent({
      policies: [{ category: "in_progress", default_execution_agent_id: null }],
    }),
  );
  assert.throws(
    () =>
      requireNoDefaultExecutionAgent({
        policies: [
          {
            category: "in_progress",
            default_execution_agent_id: "11111111-1111-4111-8111-111111111111",
          },
        ],
      }),
    /refusing to apply/u,
  );
});

test("requires the exact safe fixture topology and identifiers", () => {
  const graph = productionSmokeGraph();
  assert.deepEqual(requireProductionSmokeGraphContract(graph), {
    dependentIdentifier: "SMOKE-4",
    edges: [
      {
        fromIdentifier: "SMOKE-2",
        toIdentifier: "SMOKE-4",
        state: "blocked",
        stateLabel: "Blocked",
      },
      {
        fromIdentifier: "SMOKE-3",
        toIdentifier: "SMOKE-4",
        state: "satisfied",
        stateLabel: "Satisfied",
      },
    ],
  });

  const reversed = structuredClone(graph);
  reversed.edges[0].from = "task-3";
  reversed.edges[0].to = "task-1";
  assert.throws(
    () => requireProductionSmokeGraphContract(reversed),
    /missing edge task-1->task-3/u,
  );

  const assigned = structuredClone(graph);
  assigned.nodes[0].executor_type = "agent";
  assigned.nodes[0].executor_id = "11111111-1111-4111-8111-111111111111";
  assert.throws(
    () => requireProductionSmokeGraphContract(assigned),
    /assigned to an executor/u,
  );

  const wrongContract = structuredClone(graph);
  wrongContract.edges[0].type = "soft";
  assert.throws(
    () => requireProductionSmokeGraphContract(wrongContract),
    /wrong contract/u,
  );

  const wrongConsumedOutput = structuredClone(graph);
  wrongConsumedOutput.edges[0].consumed_output = "Wrong output";
  assert.throws(
    () => requireProductionSmokeGraphContract(wrongConsumedOutput),
    /wrong contract/u,
  );

  const invalidState = structuredClone(graph);
  delete invalidState.edges[0].satisfied;
  assert.throws(
    () => requireProductionSmokeGraphContract(invalidState),
    /invalid state/u,
  );
});

test("requires real graph nodes and edges and wires fixture acceptance", () => {
  assert.doesNotThrow(() =>
    requireProductionSmokeGraph({ nodeCount: 3, edgeCount: 2 }),
  );
  assert.throws(
    () => requireProductionSmokeGraph({ nodeCount: 2, edgeCount: 2 }),
    /expected at least 3/u,
  );
  assert.throws(
    () => requireProductionSmokeGraph({ nodeCount: 3, edgeCount: 1 }),
    /expected at least 2/u,
  );
  assert.match(
    browserVerifierSource,
    /await ensureSmokeDependencyGraph\(page, workspace\);/u,
  );
  assert.match(
    browserVerifierSource,
    /verifyProductionSmokeTaskGraph\(page, smokeGraph\.graph\)/u,
  );
  assert.match(browserVerifierSource, /findProductionSmokeGraph/u);
  assert.match(browserVerifierSource, /\/api\/issue-category-policies/u);
  assert.match(browserVerifierSource, /response\.status === 409/u);
  assert.match(
    browserVerifierSource,
    /Dependency from \$\{edge\.fromIdentifier\} to \$\{edge\.toIdentifier\} — \$\{edge\.stateLabel\}/u,
  );
  assert.match(
    browserVerifierSource,
    /toHaveAttribute\("data-edge-state", edge\.state/u,
  );
  assert.match(browserVerifierSource, /randomBytes\(16\)\.toString\("hex"\)/u);
});
