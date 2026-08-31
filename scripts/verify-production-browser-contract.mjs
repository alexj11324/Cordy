const SHA_PATTERN = /^[0-9a-f]{40}$/u;
const HANDOFF_VALUE_PATTERN = /^[A-Za-z0-9._~-]{43,128}$/u;
const GOOGLE_OAUTH_ENTRY = "https://accounts.aspectlylabs.com/oauth/google";

export const PRODUCTION_SMOKE_GRAPH_GOAL =
  "Verify the production task dependency graph end to end";
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
        title: "Production smoke: first prerequisite",
        description: "Provide the first independently verifiable graph input.",
        acceptance_criteria: [
          "The first prerequisite exposes its validated output",
        ],
        context: {},
        outputs: ["First prerequisite output"],
      },
      {
        temp_id: "task-2",
        title: "Production smoke: second prerequisite",
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
