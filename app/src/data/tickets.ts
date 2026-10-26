// Ticket types shared by the board and the app. Tickets are work units; map nodes are context.
export type TicketStatus = 'todo' | 'in_progress' | 'done'
export type TicketRunState = 'starting' | 'running' | 'finished' | 'interrupted' | 'failed' | 'disconnected' | 'start_failed'
export type TicketReviewDecisionKind = 'approved_completion' | 'rework_requested' | 'blocked' | 'incomplete'
export type Feature = { id: number; projectId: number; title: string; description: string; position: number; createdAt: string; ticketCount: number; doneCount: number; completed: boolean }
export type Ticket = {
  id: number
  projectId: number
  featureId: number | null
  title: string
  goal: string
  completionCriteria: string[]
  nodeIds: string[]
  status: TicketStatus
  position: number
  source: 'agent' | 'user'
  createdAt: string
  updatedAt: string
  latestRunId: number | null
  latestRunState: TicketRunState | null
  latestRunDetail: string | null
  latestRunStartedAt: string | null
  latestRunFinishedAt: string | null
  reportOutcome: 'completed' | 'incomplete' | 'blocked' | null
  reviewDecision: TicketReviewDecisionKind | null
  reviewNote: string | null
  reviewedAt: string | null
}
export type TicketBoardData = { features: Feature[]; tickets: Ticket[]; nextTicketId: number | null }
export type TicketInput = { featureId: number | null; title: string; goal: string; completionCriteria: string[]; nodeIds: string[] }
export type TicketReviewDecision = { id: number; ticketId: number; runId: number; decision: TicketReviewDecisionKind; note: string | null; reviewedAt: string; ticketStatus: TicketStatus }
export type RunReport = { outcome: 'completed' | 'incomplete' | 'blocked'; summary: string; criteria: { index: number; status: 'met' | 'not_met' | 'not_run'; evidence: string[] }[]; changes: { path: string; summary: string }[]; checks: { command: string; status: 'passed' | 'failed' | 'not_run'; summary: string }[]; blockers: string[] }
export type RunEvidence = { id: number; kind: 'command_execution' | 'workspace_snapshot'; evidence: { command?: string; status?: string; exitCode?: number | null; durationMs?: number | null; changedFiles?: string[]; available?: boolean; error?: string }; observedAt: string }
export type TicketRunReview = { runId: number; ticketId: number; mapRevision: number; completionCriteria: string[]; state: TicketRunState; detail: string | null; startedAt: string; finishedAt: string | null; report: RunReport | null; evidence: RunEvidence[] }

export const emptyTicketBoard: TicketBoardData = { features: [], tickets: [], nextTicketId: null }

const ticketStatusLabels: Record<TicketStatus, string> = { todo: 'To do', in_progress: 'In progress', done: 'Done' }

export const reviewDecisionLabels: Record<TicketReviewDecisionKind, string> = {
  approved_completion: 'Completion approved',
  rework_requested: 'Rework requested',
  blocked: 'Marked blocked',
  incomplete: 'Marked incomplete',
}

export const runStateLabels: Record<TicketRunState, string> = {
  starting: 'Agent starting',
  running: 'Agent running',
  finished: 'Run finished',
  interrupted: 'Run interrupted',
  failed: 'Run failed',
  disconnected: 'Agent disconnected',
  start_failed: 'Run could not start',
}

export const ticketIsRunning = (ticket: Ticket) => ticket.latestRunState === 'starting' || ticket.latestRunState === 'running'

export function ticketStatusText(ticket: Ticket) {
  if (ticketIsRunning(ticket)) return runStateLabels[ticket.latestRunState!]
  if (ticket.status === 'in_progress' && ticket.latestRunState === 'finished') return 'Awaiting your review'
  return ticketStatusLabels[ticket.status]
}
