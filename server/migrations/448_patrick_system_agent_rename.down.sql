-- Roll back only when no canonical identity has been created alongside the
-- legacy one.

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM agent
        WHERE system_key IN ('mika', 'patrick')
        GROUP BY workspace_id
        HAVING COUNT(*) > 1
    ) THEN
        RAISE EXCEPTION
            'cannot roll back system agent rename: workspace contains duplicate Mika/Patrick identities';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM agent canonical
        JOIN agent conflicting
          ON conflicting.workspace_id = canonical.workspace_id
         AND conflicting.id <> canonical.id
         AND conflicting.name = 'Mika'
        WHERE canonical.system_key = 'patrick'
    ) THEN
        RAISE EXCEPTION
            'cannot roll back system agent rename: workspace already has an agent named Mika';
    END IF;
END
$$;

UPDATE agent
SET system_key = 'mika',
    name = CASE WHEN name = 'Patrick' THEN 'Mika' ELSE name END,
    description = CASE description
        WHEN 'Your workspace Chief of Staff. Patrick turns goals into issues, coordinates agents, and helps build reusable workflows.'
            THEN 'Your workspace Chief of Staff. Mika turns goals into issues, coordinates agents, and helps build reusable workflows.'
        WHEN '你的工作区 Chief of Staff。Patrick 会把目标转化为任务、协调智能体，并帮你建立可复用的工作流。'
            THEN '你的工作区 Chief of Staff。Mika 会把目标转化为任务、协调智能体，并帮你建立可复用的工作流。'
        WHEN '워크스페이스의 Chief of Staff입니다. Patrick이 목표를 태스크로 구체화하고 에이전트를 조율하며 재사용 가능한 워크플로 구성을 돕습니다.'
            THEN '워크스페이스의 Chief of Staff입니다. Mika가 목표를 태스크로 구체화하고 에이전트를 조율하며 재사용 가능한 워크플로 구성을 돕습니다.'
        WHEN 'ワークスペースの Chief of Staff。Patrick は目標をタスクに落とし込み、エージェントを調整し、再利用できるワークフローづくりを支援します。'
            THEN 'ワークスペースの Chief of Staff。Mika は目標をタスクに落とし込み、エージェントを調整し、再利用できるワークフローづくりを支援します。'
        ELSE description
    END,
    updated_at = now()
WHERE system_key = 'patrick';

UPDATE workspace
SET settings = jsonb_set(
        settings,
        '{orchestrator_system_key}',
        '"mika"'::jsonb,
        false
    ),
    updated_at = now()
WHERE settings ->> 'orchestrator_system_key' = 'patrick';
