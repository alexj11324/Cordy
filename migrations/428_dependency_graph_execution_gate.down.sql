DROP TRIGGER IF EXISTS trg_dependency_graph_task_admission ON agent_task_queue;
DROP FUNCTION IF EXISTS dependency_graph_task_admission_gate();
DROP FUNCTION IF EXISTS dependency_graph_issue_gate_open(UUID, UUID);
