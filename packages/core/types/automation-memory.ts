export interface AutomationMemorySummary {
  name: string;
  revision: number;
  updated_at: string;
}

export interface AutomationMemoryFile extends AutomationMemorySummary {
  content: string;
}

export interface ListAutomationMemoriesResponse {
  items: AutomationMemorySummary[];
}

export interface UpdateAutomationMemoryRequest {
  content: string;
  expected_revision: number;
}
