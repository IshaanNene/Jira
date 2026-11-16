import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, type AutomationRule, type AutomationRuleDefinition, type AutomationRuleList, type AutomationRunList } from "@/services/api";
import { useSession } from "@/state/session";

const BASE = "/api/v1/automation-rules";
const useOrgId = () => useSession((s) => s.organization?.id);
const keys = {
  rules: (orgId: string | undefined, projectKey: string) => ["org", orgId, "automation-rules", projectKey] as const,
  runs: (orgId: string | undefined, id: string) => ["org", orgId, "automation-runs", id] as const,
};

export function useAutomationRules(projectKey: string) {
  const orgId = useOrgId();
  return useQuery({
    queryKey: keys.rules(orgId, projectKey),
    queryFn: () => api.get<AutomationRuleList>(BASE, { query: { project: projectKey } }),
    select: (d) => d.items,
    enabled: Boolean(orgId),
  });
}

export function useAutomationRuns(id: string | null) {
  const orgId = useOrgId();
  return useQuery({
    queryKey: keys.runs(orgId, id ?? ""),
    queryFn: () => api.get<AutomationRunList>(`${BASE}/${id}/runs`),
    select: (d) => d.items,
    enabled: Boolean(orgId && id),
  });
}

export interface RuleInput {
  name: string;
  enabled: boolean;
  definition: AutomationRuleDefinition;
}

export function useAutomationMutations(projectKey: string) {
  const qc = useQueryClient();
  const orgId = useOrgId();
  const refresh = () => qc.invalidateQueries({ queryKey: keys.rules(orgId, projectKey) });
  return {
    create: useMutation({
      mutationFn: (input: RuleInput) => api.post<AutomationRule>(BASE, { project_key: projectKey, ...input }),
      onSettled: refresh,
    }),
    update: useMutation({
      mutationFn: ({ rule, input }: { rule: AutomationRule; input: RuleInput }) =>
        api.put<AutomationRule>(`${BASE}/${rule.id}`, input, { ifMatch: rule.version }),
      onSettled: refresh,
    }),
    remove: useMutation({
      mutationFn: (rule: AutomationRule) => api.delete(`${BASE}/${rule.id}`),
      onSettled: refresh,
    }),
  };
}
