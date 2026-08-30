import { useLocalSearchParams } from "expo-router";
import { Header } from "@/components/ui/header";
import { TaskAgentThreadScreen } from "@/components/agent-thread/task-agent-thread-screen";

/**
 * Native formSheet route for one persisted Agent task. The row and this
 * screen are intentionally separate navigation states so the same thread
 * contract also works from future non-Issue and Automation entry points.
 */
export default function TaskAgentThreadRoute() {
  const { taskId } = useLocalSearchParams<{ taskId: string }>();

  if (!taskId) return null;

  return (
    <>
      <Header title="Agent thread" />
      <TaskAgentThreadScreen taskId={taskId} />
    </>
  );
}
