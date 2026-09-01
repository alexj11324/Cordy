/**
 * Bottom chip row for the new-issue form. Mirrors `attribute-row.tsx`'s
 * visual pattern but operates on the `useNewIssueDraftStore` instead of an
 * `issue` object + mutation. Tapping a chip pushes a formSheet picker
 * route under `new-issue-picker/<field>` — the route reads/writes the same
 * draft store, so the chip rehydrates automatically when the sheet
 * dismisses.
 *
 * Why a draft store: the picker routes are siblings of new-issue.tsx in
 * the Stack — they can't reach into the new-issue screen's local state.
 * The draft store is the cross-screen channel.
 */
import { View } from "react-native";
import { router } from "expo-router";
import { Ionicons } from "@expo/vector-icons";
import { AttributeChip } from "@/components/issue/attribute-chip";
import { ActorAvatar } from "@/components/ui/actor-avatar";
import { PriorityIcon } from "@/components/ui/priority-icon";
import { ProjectIcon } from "@/components/ui/project-icon";
import { StatusIcon } from "@/components/ui/status-icon";
import { formatDateOnly } from "@patchbay/core/issues/date";
import { useActorLookup } from "@/data/use-actor-name";
import { useNewIssueDraftStore } from "@/data/stores/new-issue-draft-store";
import { useWorkspaceStore } from "@/data/workspace-store";
import { PRIORITY_LABEL } from "@/lib/issue-status";
import { useIssueStatuses } from "@/lib/use-issue-statuses";

/**
 * Picker fields the new-issue draft form can open. Bound to a typed map
 * of Expo Router pathnames so typos become compile errors (previously
 * the call site used `as never` on a template string).
 */
type NewIssuePickerField =
  | "status"
  | "priority"
  | "owner"
  | "executor"
  | "reviewer"
  | "project"
  | "due-date";

const NEW_ISSUE_PICKER_PATHNAMES = {
  status: "/[workspace]/new-issue-picker/status",
  priority: "/[workspace]/new-issue-picker/priority",
  owner: "/[workspace]/new-issue-picker/owner",
  executor: "/[workspace]/new-issue-picker/executor",
  reviewer: "/[workspace]/new-issue-picker/reviewer",
  project: "/[workspace]/new-issue-picker/project",
  "due-date": "/[workspace]/new-issue-picker/due-date",
} as const satisfies Record<NewIssuePickerField, string>;

export function CreateFormAttributeRow() {
  const wsSlug = useWorkspaceStore((s) => s.currentWorkspaceSlug);
  const status = useNewIssueDraftStore((s) => s.status);
  const priority = useNewIssueDraftStore((s) => s.priority);
  const owner = useNewIssueDraftStore((s) => s.owner);
  const executor = useNewIssueDraftStore((s) => s.executor);
  const reviewer = useNewIssueDraftStore((s) => s.reviewer);
  const dueDate = useNewIssueDraftStore((s) => s.dueDate);
  const project = useNewIssueDraftStore((s) => s.project);

  const { getName } = useActorLookup();
  // The draft can hold a custom status the user picked in the sheet. (PB-6243)
  const { categoryOf, colorOf, labelOf } = useIssueStatuses();
  const executorLabel = executor
    ? getName(executor.type, executor.id)
    : "Executor";
  const ownerLabel = owner ? getName(owner.type, owner.id) : "Owner";
  const reviewerLabel = reviewer ? getName(reviewer.type, reviewer.id) : "Reviewer";
  const priorityLabel =
    priority === "none" ? "Priority" : PRIORITY_LABEL[priority];

  const open = (field: NewIssuePickerField) => {
    if (!wsSlug) return;
    router.push({
      pathname: NEW_ISSUE_PICKER_PATHNAMES[field],
      params: { workspace: wsSlug },
    });
  };

  return (
    <View>
      <View className="flex-row flex-wrap gap-2">
        <AttributeChip
          icon={
            <StatusIcon
              status={status}
              category={categoryOf(status)}
              color={colorOf(status)}
              size={12}
            />
          }
          label={labelOf(status)}
          variant="filled"
          onPress={() => open("status")}
        />
        <AttributeChip
          icon={<PriorityIcon priority={priority} />}
          label={priorityLabel}
          variant={priority === "none" ? "dimmed" : "filled"}
          onPress={() => open("priority")}
        />
        <AttributeChip
          icon={
            owner ? (
              <ActorAvatar type="member" id={owner.id} size={16} />
            ) : (
              <Ionicons name="person-outline" size={16} color="#a1a1aa" />
            )
          }
          label={ownerLabel}
          variant={owner ? "filled" : "dimmed"}
          onPress={() => open("owner")}
        />
        <AttributeChip
          icon={
            executor ? (
              <ActorAvatar
                type={executor.type}
                id={executor.id}
                size={16}
                showPresence
              />
            ) : (
              <Ionicons
                name="person-circle-outline"
                size={16}
                color="#a1a1aa"
              />
            )
          }
          label={executorLabel}
          variant={executor ? "filled" : "dimmed"}
          onPress={() => open("executor")}
        />
        <AttributeChip
          icon={
            reviewer ? (
              <ActorAvatar type={reviewer.type} id={reviewer.id} size={16} />
            ) : (
              <Ionicons name="checkmark-circle-outline" size={16} color="#a1a1aa" />
            )
          }
          label={reviewerLabel}
          variant={reviewer ? "filled" : "dimmed"}
          onPress={() => open("reviewer")}
        />
        <AttributeChip
          icon={
            <Ionicons
              name="calendar-outline"
              size={14}
              color={dueDate ? undefined : "#a1a1aa"}
            />
          }
          label={dueDate ? formatDueDate(dueDate) : "Due date"}
          variant={dueDate ? "filled" : "dimmed"}
          onPress={() => open("due-date")}
        />
        <AttributeChip
          icon={
            project ? (
              <ProjectIcon icon={project.icon} size="sm" />
            ) : (
              <Ionicons name="folder-outline" size={14} color="#a1a1aa" />
            )
          }
          label={project?.title ?? "Project"}
          variant={project ? "filled" : "dimmed"}
          onPress={() => open("project")}
        />
      </View>
    </View>
  );
}

// due_date is a calendar day — format timezone-safely (no offset day shift).
function formatDueDate(iso: string): string {
  return formatDateOnly(iso, { month: "short", day: "numeric" }) || "Due date";
}
