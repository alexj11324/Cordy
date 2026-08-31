const SHA_PATTERN = /^[0-9a-f]{40}$/u;
const HANDOFF_VALUE_PATTERN = /^[A-Za-z0-9._~-]{43,128}$/u;
const GOOGLE_OAUTH_ENTRY = "https://accounts.aspectlylabs.com/oauth/google";

export const PRODUCTION_SMOKE_GRAPH_GOAL =
  "Verify the production task dependency graph end to end";
export const PRODUCTION_SMOKE_PARENT_TITLE =
  "Production smoke dependency graph";
export const PRODUCTION_SMOKE_FIRST_TASK_TITLE =
  "Production smoke: first prerequisite";
export const PRODUCTION_SMOKE_SECOND_TASK_TITLE =
  "Production smoke: second prerequisite";
export const PRODUCTION_SMOKE_DEPENDENT_TASK_TITLE =
  "Production smoke: combine prerequisites";
export const PRODUCTION_SMOKE_DEPENDENT_ACCEPTANCE =
  "Both prerequisite outputs are visible before the dependent task can run";

export function requiredString(value, label) {
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error(`${label} is required`);
  }
  return value.trim();
}

export function buildProductionSmokeDependencyPlan(parentIssueId) {
  return {
    goal: PRODUCTION_SMOKE_GRAPH_GOAL,
    parent_issue_id: requiredString(parentIssueId, "parent issue id"),
    tasks: [
      {
        temp_id: "task-1",
        title: PRODUCTION_SMOKE_FIRST_TASK_TITLE,
        description: "Provide the first independently verifiable graph input.",
        acceptance_criteria: [
          "The first prerequisite exposes its validated output",
        ],
        context: {},
        outputs: ["First prerequisite output"],
      },
      {
        temp_id: "task-2",
        title: PRODUCTION_SMOKE_SECOND_TASK_TITLE,
        description: "Provide the second independently verifiable graph input.",
        acceptance_criteria: [
          "The second prerequisite exposes its validated output",
        ],
        context: {},
        outputs: ["Second prerequisite output"],
      },
      {
        temp_id: "task-3",
        title: PRODUCTION_SMOKE_DEPENDENT_TASK_TITLE,
        description:
          "Consume both prerequisite outputs through two explicit dependency edges.",
        acceptance_criteria: [PRODUCTION_SMOKE_DEPENDENT_ACCEPTANCE],
        context: {},
        outputs: ["Combined production smoke result"],
      },
    ],
    edges: [
      {
        from: "task-1",
        to: "task-3",
        type: "hard",
        reason: "The dependent task requires the first validated input.",
        consumed_output: "First prerequisite output",
      },
      {
        from: "task-2",
        to: "task-3",
        type: "hard",
        reason: "The dependent task requires the second validated input.",
        consumed_output: "Second prerequisite output",
      },
    ],
  };
}

export function requireProductionSmokeGraph({ nodeCount, edgeCount }) {
  if (!Number.isInteger(nodeCount) || nodeCount < 3) {
    throw new Error(
      `production task graph rendered ${nodeCount} nodes, expected at least 3`,
    );
  }
  if (!Number.isInteger(edgeCount) || edgeCount < 2) {
    throw new Error(
      `production task graph rendered ${edgeCount} edges, expected at least 2`,
    );
  }
}

export async function findProductionSmokeGraph(fetchPage) {
  if (typeof fetchPage !== "function") {
    throw new Error("dependency graph page loader is required");
  }
  let cursor = null;
  const visitedCursors = new Set();
  while (true) {
    const page = await fetchPage(cursor);
    if (!Array.isArray(page?.graphs)) {
      throw new Error("dependency graph API returned an invalid list response");
    }
    const existing = page.graphs.find(
      (graph) => graph?.plan?.goal === PRODUCTION_SMOKE_GRAPH_GOAL,
    );
    if (existing) return existing;

    if (page.next_cursor === null || page.next_cursor === undefined) {
      return null;
    }
    const nextCursor = requiredString(
      page.next_cursor,
      "dependency graph next cursor",
    );
    if (visitedCursors.has(nextCursor)) {
      throw new Error("dependency graph API returned a repeated cursor");
    }
    visitedCursors.add(nextCursor);
    cursor = nextCursor;
  }
}

export function findProductionSmokeParentIssue(response) {
  if (!Array.isArray(response?.issues)) {
    throw new Error("issue API returned an invalid list response");
  }
  return (
    response.issues.find(
      (issue) =>
        issue?.title === PRODUCTION_SMOKE_PARENT_TITLE &&
        issue?.parent_issue_id == null,
    ) ?? null
  );
}

export function requireNoDefaultExecutionAgent(response) {
  if (!Array.isArray(response?.policies)) {
    throw new Error("issue category policy API returned an invalid response");
  }
  const executionPolicy = response.policies.find(
    (policy) => policy?.category === "in_progress",
  );
  if (executionPolicy?.default_execution_agent_id != null) {
    throw new Error(
      "production smoke workspace has a default execution agent; refusing to apply a graph fixture that could start real agent work",
    );
  }
}

export function requireProductionSmokeGraphContract(graph) {
  if (graph?.plan?.goal !== PRODUCTION_SMOKE_GRAPH_GOAL) {
    throw new Error("production smoke dependency graph has the wrong goal");
  }
  if (!Array.isArray(graph?.nodes) || graph.nodes.length !== 3) {
    throw new Error("production smoke dependency graph must contain 3 nodes");
  }
  if (!Array.isArray(graph?.edges) || graph.edges.length !== 2) {
    throw new Error("production smoke dependency graph must contain 2 edges");
  }

  const nodesByTempId = new Map(
    graph.nodes.map((node) => [node?.temp_id, node]),
  );
  const expectedTasks = [
    ["task-1", PRODUCTION_SMOKE_FIRST_TASK_TITLE],
    ["task-2", PRODUCTION_SMOKE_SECOND_TASK_TITLE],
    ["task-3", PRODUCTION_SMOKE_DEPENDENT_TASK_TITLE],
  ];
  for (const [tempId, title] of expectedTasks) {
    const node = nodesByTempId.get(tempId);
    if (node?.title !== title) {
      throw new Error(`production smoke dependency graph is missing ${tempId}`);
    }
    if (node.executor_type != null || node.executor_id != null) {
      throw new Error(
        `production smoke dependency graph task ${tempId} is assigned to an executor`,
      );
    }
  }

  const expectedPairs = [
    ["task-1", "task-3"],
    ["task-2", "task-3"],
  ];
  const actualPairs = new Set(
    graph.edges.map((edge) => `${edge?.from}->${edge?.to}`),
  );
  for (const [from, to] of expectedPairs) {
    if (!actualPairs.has(`${from}->${to}`)) {
      throw new Error(
        `production smoke dependency graph is missing edge ${from}->${to}`,
      );
    }
  }

  const dependent = nodesByTempId.get("task-3");
  if (
    !Array.isArray(dependent.acceptance_criteria) ||
    !dependent.acceptance_criteria.includes(
      PRODUCTION_SMOKE_DEPENDENT_ACCEPTANCE,
    )
  ) {
    throw new Error(
      "production smoke dependent task is missing its acceptance criterion",
    );
  }

  const identifiers = new Map(
    expectedTasks.map(([tempId]) => [
      tempId,
      requiredString(
        nodesByTempId.get(tempId)?.issue?.identifier,
        `${tempId} issue identifier`,
      ),
    ]),
  );
  return {
    dependentIdentifier: identifiers.get("task-3"),
    edges: expectedPairs.map(([from, to]) => ({
      fromIdentifier: identifiers.get(from),
      toIdentifier: identifiers.get(to),
    })),
  };
}

export function isExpectedBrowserRequestCancellation(errorText) {
  return errorText === "net::ERR_ABORTED";
}

export function decodeClerkFrontendApi(publishableKey) {
  const key = requiredString(publishableKey, "CLERK_PUBLISHABLE_KEY");
  const match = /^pk_(?:live|test)_(.+)$/u.exec(key);
  if (!match) throw new Error("CLERK_PUBLISHABLE_KEY has an invalid format");
  const decoded = Buffer.from(match[1], "base64")
    .toString("utf8")
    .replace(/\$$/u, "");
  if (!/^[a-z0-9.-]+$/iu.test(decoded) || !decoded.includes(".")) {
    throw new Error(
      "CLERK_PUBLISHABLE_KEY does not contain a valid Frontend API host",
    );
  }
  return decoded;
}

export function requireBrowserReceipt(receipt, sourceSha) {
  if (!SHA_PATTERN.test(sourceSha)) {
    throw new Error("source SHA must be 40 lowercase hexadecimal characters");
  }
  if (
    receipt?.ok !== true ||
    receipt?.action !== "deploy" ||
    receipt?.source_sha !== sourceSha
  ) {
    throw new Error(
      "deployment receipt does not match the requested source SHA",
    );
  }
  return {
    signInTicket: requiredString(
      receipt.browser_auth?.sign_in_ticket,
      "browser sign-in ticket",
    ),
    testingToken: requiredString(
      receipt.browser_auth?.testing_token,
      "browser testing token",
    ),
  };
}

export function requireProtectedNavigation({
  url,
  status,
  actualBuild,
  expectedBuild,
  expectedPath,
}) {
  const parsed = new URL(url);
  if (status !== 200) {
    throw new Error(`${url} returned HTTP ${status}, expected 200`);
  }
  if (parsed.pathname !== expectedPath) {
    throw new Error(
      `${url} ended at ${parsed.pathname}, expected ${expectedPath}`,
    );
  }
  if (actualBuild !== expectedBuild) {
    throw new Error(
      `${url} reported build ${actualBuild ?? "<missing>"}, expected ${expectedBuild}`,
    );
  }
}

export function buildGoogleOAuthProbeUrl({ codeChallenge, state }) {
  if (
    !HANDOFF_VALUE_PATTERN.test(codeChallenge) ||
    !HANDOFF_VALUE_PATTERN.test(state)
  ) {
    throw new Error("Google OAuth probe requires a valid desktop handoff");
  }
  const url = new URL(GOOGLE_OAUTH_ENTRY);
  url.search = new URLSearchParams({
    platform: "desktop",
    code_challenge: codeChallenge,
    state,
  }).toString();
  return url.href;
}

export function requireGoogleOAuthNavigation(url) {
  const parsed = new URL(url);
  if (
    parsed.protocol !== "https:" ||
    parsed.hostname !== "accounts.google.com"
  ) {
    throw new Error(
      `Google OAuth did not reach accounts.google.com (ended at ${parsed.origin})`,
    );
  }
  return parsed;
}
