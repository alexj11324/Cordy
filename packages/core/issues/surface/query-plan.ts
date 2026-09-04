import type { CreateIssueRequest } from "../../types";
import type { MyIssuesFilter } from "../queries";
import {
  roleFiltersForActorKind,
  issueScopeKey,
  UnsupportedIssueScopeError,
  type IssueScope,
} from "./scope";

/**
 * The scope's non-Table residue. Row membership for every list-shaped mode
 * (table, list, board, swimlane) is compiled into an IssueTableQuerySpec by
 * the surface controller and answered by the server-owned Table channel —
 * this plan only carries what that channel does not cover:
 *
 * - `scopeKey`: the surface's cache/persistence identity.
 * - `queryFilter`: the scope as legacy list-API params, consumed solely by
 *   the Gantt projection (whose scheduled-only window is not expressible in
 *   the Table spec).
 * - `createDefaults`: what a new issue created on this surface inherits.
 */
export interface IssueSurfaceQueryPlan {
  scopeKey: string;
  queryFilter: MyIssuesFilter;
  createDefaults: Partial<CreateIssueRequest>;
}

function buildMyRelationPlan(
  scope: Extract<IssueScope, { type: "my" }>,
  scopeKey: string,
): IssueSurfaceQueryPlan {
  switch (scope.relation) {
    case "assigned":
      return {
        scopeKey,
        queryFilter: { owner_id: scope.userId },
        createDefaults: {
          owner_type: "member",
          owner_id: scope.userId,
        },
      };
    case "created":
      return {
        scopeKey,
        queryFilter: { creator_id: scope.userId },
        createDefaults: {},
      };
    case "involved":
      return {
        scopeKey,
        queryFilter: { involves_user_id: scope.userId },
        createDefaults: {},
      };
    case "all":
      return { scopeKey, queryFilter: {}, createDefaults: {} };
  }
}

export function buildIssueSurfaceQueryPlan(
  scope: IssueScope,
): IssueSurfaceQueryPlan {
  const scopeKey = issueScopeKey(scope);

  switch (scope.type) {
    case "workspace": {
      const { ownerTypes, executorTypes } = roleFiltersForActorKind(scope.actorKind);
      return {
        scopeKey,
        queryFilter: {
          ...(ownerTypes ? { owner_types: ownerTypes } : {}),
          ...(executorTypes ? { executor_types: executorTypes } : {}),
        },
        createDefaults: {},
      };
    }
    case "project": {
      const { ownerTypes, executorTypes } = roleFiltersForActorKind(scope.actorKind);
      return {
        scopeKey,
        queryFilter: {
          project_id: scope.projectId,
          ...(ownerTypes ? { owner_types: ownerTypes } : {}),
          ...(executorTypes ? { executor_types: executorTypes } : {}),
        },
        createDefaults: { project_id: scope.projectId },
      };
    }
    case "my":
      return buildMyRelationPlan(scope, scopeKey);
    case "actor":
      return {
        scopeKey,
        queryFilter:
          scope.relation === "assigned"
            ? scope.actorType === "member"
              ? { owner_id: scope.actorId }
              : { executor_id: scope.actorId }
            : { creator_id: scope.actorId },
        createDefaults:
          scope.relation === "assigned"
            ? scope.actorType === "member"
              ? { owner_type: "member", owner_id: scope.actorId }
              : { executor_type: scope.actorType, executor_id: scope.actorId }
            : {},
      };
    case "team":
      throw new UnsupportedIssueScopeError(scope, "issue surface query plan");
  }
}
