import { useEffect, useMemo, useRef, useState, type FormEvent, type KeyboardEvent } from 'react'
import { invoke, isTauri } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import {
  Background,
  BackgroundVariant,
  Controls,
  Handle,
  MarkerType,
  MiniMap,
  Position,
  ReactFlow,
  useEdgesState,
  useNodesState,
  type Connection,
  type Node,
  type NodeProps,
  type ReactFlowInstance,
} from '@xyflow/react'
import {
  AlertCircle,
  Bot,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  ClipboardList,
  Code2,
  Database,
  FileCode2,
  FolderOpen,
  FolderPlus,
  GitBranch,
  LayoutGrid,
  Link2,
  LoaderCircle,
  ListChecks,

  MessageCircle,
  Monitor,
  MoreHorizontal,
  Plus,
  Send,
  ShieldCheck,
  Sparkles,
  SquareTerminal,
  Workflow,
  X,
  type LucideIcon,
} from 'lucide-react'
import {
  initialEdges,
  initialNodes,
  relationLabel,
  type ComponentData,
  type ComponentIcon,
  type HeimEdge,
  type HeimNode,
  type LayerData,
  type MapRelation,
} from './data/projectMap'
import { QuestionnairePanel } from './QuestionnairePanel'
import { DemoWorkspace } from './DemoWorkspace'
import '@xyflow/react/dist/style.css'
import './App.css'

type ComponentFlowNode = Node<ComponentData, 'component'>
type LayerFlowNode = Node<LayerData, 'layer'>
type WorkspaceTab = 'chat' | 'terminal'
type DetailTab = 'overview' | 'files' | 'tasks' | 'relationships'
type ChatMessage = { id: string; text: string; time: string; role: 'user' | 'agent' | 'status' }
type AgentEvent = { projectId: number; kind: 'delta' | 'message' | 'status' | 'protocol_error' | 'files' | 'changed' | 'done' | 'disconnected' | 'approval'; text: string; itemId?: string; requestId?: number }
type Approval = { id: number; text: string }
type MapProposal = { id: number; projectId: number; baseRevision: number; changedFiles: string[]; changes: {
  updates: { nodeId: string; summary: string; status: string; files: string[]; removeFiles: string[]; reason: string; evidence: string }[]
  additions: { title: string; summary: string; status: string; files: string[]; reason: string; evidence: string }[]
  removals: { nodeId: string; deletedFile: string; reason: string }[]
} }
type RuntimeInfo = { version: string; platform: string }
type Project = { id: number; name: string; root: string }
type StoredMap = { nodes: HeimNode[]; edges: HeimEdge[]; revision: number }
type NextDevelopmentRecommendation = { nodeId: string; rationale: string }
type RecommendationState = { key: string; status: 'loading' | 'ready' | 'none' | 'error'; recommendation: NextDevelopmentRecommendation | null; error?: string }
type DesignDraft = { id: number; projectId: number; baseRevision: number; version: number; draft: { goal: string; nodes: HeimNode[]; edges: HeimEdge[]; questions: string[]; assumptions: string[] } }
type MapEditDraftResult = { draft: DesignDraft; confirmedMap: StoredMap }
type DevelopmentReviewDecision = 'approved_completion' | 'rework_requested' | 'blocked' | 'incomplete'
type StoredDevelopmentReviewDecision = { id: number; unitId: number; runId: number; decision: DevelopmentReviewDecision; note: string | null; reviewedAt: string }
type DevelopmentUnit = { id: number; projectId: number; mapRevision: number; nodeId: string; goal: string; completionCriteria: string[]; status: 'confirmed' | 'started' | 'start_failed'; statusDetail: string | null; createdAt: string; startedAt: string | null; latestRunId: number | null; latestRunState: 'starting' | 'running' | 'finished' | 'interrupted' | 'failed' | 'disconnected' | 'start_failed' | null; latestRunDetail: string | null; latestRunStartedAt: string | null; latestRunFinishedAt: string | null; reviewDecision: DevelopmentReviewDecision | null; reviewNote: string | null; reviewedAt: string | null }
type DevelopmentUnitSpec = { projectId: number; mapRevision: number; nodeId: string; goal: string; completionCriteria: string[]; source: 'agent' | 'user'; version: number; updatedAt: string }
type DevelopmentRunReport = { outcome: 'completed' | 'incomplete' | 'blocked'; summary: string; criteria: { index: number; status: 'met' | 'not_met' | 'not_run'; evidence: string[] }[]; changes: { path: string; summary: string }[]; checks: { command: string; status: 'passed' | 'failed' | 'not_run'; summary: string }[]; blockers: string[] }
type DevelopmentRunEvidence = { id: number; kind: 'command_execution' | 'workspace_snapshot'; evidence: { command?: string; status?: string; exitCode?: number | null; durationMs?: number | null; changedFiles?: string[]; available?: boolean; error?: string }; observedAt: string }
type DevelopmentRunReview = { runId: number; unitId: number; state: DevelopmentUnit['latestRunState']; detail: string | null; startedAt: string; finishedAt: string | null; report: DevelopmentRunReport | null; evidence: DevelopmentRunEvidence[] }
type BackendStatus =
  | { kind: 'checking' | 'browser' }
  | { kind: 'ready'; info: RuntimeInfo }
  | { kind: 'error'; message: string }

const componentTitle = (nodes: HeimNode[], id: string) => {
  const node = nodes.find((item) => item.id === id)
  return node?.type === 'component' ? node.data.title : id
}

const unansweredQuestions = (data: ComponentData) =>
  (data.questions ?? []).filter((question) => !(data.answers?.[question.prompt] ?? '').trim())

const edgeForDisplay = (edge: HeimEdge): HeimEdge => ({
  ...edge,
  label: edge.data?.relation ? relationLabel(edge.data.relation) : 'Classify relationship',
})

const iconByName: Record<ComponentIcon, LucideIcon> = {
  desktop: Monitor,
  map: Workflow,
  bot: Bot,
  terminal: SquareTerminal,
  share: Link2,
  database: Database,
  shield: ShieldCheck,
  review: ClipboardList,
}

const nodeHandlePositions = [
  { side: 'top', position: Position.Top },
  { side: 'right', position: Position.Right },
  { side: 'bottom', position: Position.Bottom },
  { side: 'left', position: Position.Left },
] as const

function ComponentNodeView({ data, selected }: NodeProps<ComponentFlowNode>) {
  const Icon = iconByName[data.icon]

  return (
    <div className={`component-node${selected ? ' is-selected' : ''}${data.nextRecommendationRationale ? ' is-next' : ''}`}>
      {data.nextRecommendationRationale && <span className="next-node-badge" title={`Recommended next: ${data.nextRecommendationRationale}`} aria-label={`Recommended next: ${data.nextRecommendationRationale}`}>NEXT</span>}
      {nodeHandlePositions.map(({ side, position }) => <Handle key={`target-${side}`} id={`target-${side}`} className={`node-handle node-handle-target node-handle-${side}`} type="target" position={position} />)}
      <div className="component-node-icon">
        <Icon size={18} strokeWidth={1.8} />
      </div>
      <div className="component-node-copy">
        <strong>{data.title}</strong>
        <span>{data.kind}</span>
        <small>{data.technology}</small>
      </div>
      {unansweredQuestions(data).length > 0 && <span className="node-question-badge" title="Unresolved question"><AlertCircle size={13} /></span>}
      <span className={`node-status-dot ${data.status === 'In progress' ? 'is-active' : ''}`} />
      {nodeHandlePositions.map(({ side, position }) => <Handle key={`source-${side}`} id={`source-${side}`} className={`node-handle node-handle-source node-handle-${side}`} type="source" position={position} />)}
    </div>
  )
}

function LayerNodeView({ data }: NodeProps<LayerFlowNode>) {
  return (
    <div className={`layer-node layer-${data.tone}`}>
      <span>{data.label}</span>
    </div>
  )
}

const nodeTypes = {
  component: ComponentNodeView,
  layer: LayerNodeView,
}

const workspaceTabs: { id: WorkspaceTab; label: string; icon: LucideIcon }[] = [
  { id: 'chat', label: 'Chat', icon: MessageCircle },
  { id: 'terminal', label: 'Terminal', icon: SquareTerminal },
]

const detailTabs: { id: DetailTab; label: string }[] = [
  { id: 'overview', label: 'Overview' },
  { id: 'files', label: 'Files' },
  { id: 'tasks', label: 'Tasks' },
  { id: 'relationships', label: 'Relationships' },
]

function parsePersistedTimestamp(value: string | null | undefined): Date | null {
  if (!value) return null
  const iso = value.replace(' ', 'T')
  const timestamp = /(?:Z|[+-]\d{2}:?\d{2})$/i.test(iso) ? iso : `${iso}Z`
  const date = new Date(timestamp)
  return Number.isNaN(date.getTime()) ? null : date
}

function formatPersistedTimestamp(value: string | null | undefined): string | null {
  const date = parsePersistedTimestamp(value)
  return date ? new Intl.DateTimeFormat(undefined, { dateStyle: 'short', timeStyle: 'medium' }).format(date) : null
}

function formatRunDuration(start: string | null | undefined, finish: string | null | undefined): string | null {
  const startedAt = parsePersistedTimestamp(start)
  const finishedAt = parsePersistedTimestamp(finish)
  if (!startedAt || !finishedAt) return null
  const seconds = Math.max(0, Math.round((finishedAt.getTime() - startedAt.getTime()) / 1000))
  return seconds < 60
    ? `${seconds}s`
    : `${Math.floor(seconds / 60)}m ${seconds % 60}s`
}

const developmentReviewDecisionLabels: Record<DevelopmentReviewDecision, string> = {
  approved_completion: 'Completion approved',
  rework_requested: 'Rework requested',
  blocked: 'Marked blocked',
  incomplete: 'Marked incomplete',
}

function DevelopmentRunReviewPanel({
  unit,
  review,
  loading,
  error,
  decisionNote,
  decisionSaving,
  decisionError,
  onDecisionNoteChange,
  onSaveDecision,
  onToggle,
}: {
  unit: DevelopmentUnit
  review: DevelopmentRunReview | null
  loading: boolean
  error: string | null
  decisionNote: string
  decisionSaving: boolean
  decisionError: string | null
  onDecisionNoteChange: (note: string) => void
  onSaveDecision: (decision: DevelopmentReviewDecision, note: string) => void
  onToggle: () => void
}) {
  const isOpen = review?.runId === unit.latestRunId
  const runActive = unit.latestRunState === 'starting' || unit.latestRunState === 'running'
  const report = isOpen ? review.report : null
  const snapshots = isOpen ? review.evidence.filter((item) => item.kind === 'workspace_snapshot') : []
  const commands = isOpen ? review.evidence.filter((item) => item.kind === 'command_execution') : []

  return <div className="run-review">
    <button className="run-review-toggle" type="button" onClick={onToggle} disabled={loading || runActive || !unit.latestRunId} aria-expanded={isOpen}>
      {loading ? 'Loading run details…' : runActive ? 'Run in progress' : isOpen ? 'Hide run details' : 'Review run details'}
    </button>
    {error && <p className="run-review-error" role="alert">Could not load run details: {error}</p>}
    {isOpen && <div className="run-review-content">
      <p className="run-review-note">Agent claims do not change status by themselves. Your decision below updates this development unit; the map node’s status remains user-controlled.</p>
      {!report
        ? <p className="run-review-empty">No valid Agent report was saved for this run.</p>
        : <>
          <section className="run-review-section">
            <h4>Agent report · {report.outcome}</h4>
            <p>{report.summary}</p>
            <ul className="run-review-list">
              {report.criteria.map((criterion) => <li key={criterion.index}>
                <strong>{criterion.status === 'met' ? 'Reported met' : criterion.status === 'not_met' ? 'Not met' : 'Not run'}</strong>
                <span>{unit.completionCriteria[criterion.index] ?? `Criterion ${criterion.index + 1}`}</span>
                {criterion.evidence.length > 0 && <small>{criterion.evidence.join(' · ')}</small>}
              </li>)}
            </ul>
            {report.changes.length > 0 && <><h5>Agent-reported changes</h5><ul className="run-review-list">{report.changes.map((change) => <li key={`${change.path}-${change.summary}`}><code>{change.path}</code><span>{change.summary}</span></li>)}</ul></>}
            {report.checks.length > 0 && <><h5>Agent-reported checks</h5><ul className="run-review-list">{report.checks.map((check, index) => <li key={`${check.command}-${index}`}><strong>{check.status}</strong><code>{check.command}</code><span>{check.summary}</span></li>)}</ul></>}
            {report.blockers.length > 0 && <><h5>Blockers</h5><ul className="run-review-list">{report.blockers.map((blocker) => <li key={blocker}><span>{blocker}</span></li>)}</ul></>}
          </section>
        </>}
      <section className="run-review-section">
        <h4>Heim-observed evidence</h4>
        {snapshots.length === 0 && commands.length === 0 && <p className="run-review-empty">No execution evidence was captured.</p>}
        {snapshots.map((snapshot) => snapshot.evidence.available === false
          ? <p className="run-review-empty" key={snapshot.id}>Workspace comparison unavailable: {snapshot.evidence.error ?? 'unknown error'}</p>
          : <div key={snapshot.id}><h5>Changed files</h5>{snapshot.evidence.changedFiles?.length
            ? <ul className="run-review-list">{snapshot.evidence.changedFiles.map((path) => <li key={path}><code>{path}</code></li>)}</ul>
            : <p className="run-review-empty">No changed files detected in the workspace snapshot.</p>}</div>)}
        {commands.length > 0 && <><h5>Executed commands</h5><ul className="run-review-list">{commands.map((command) => <li key={command.id}>
          <code>{command.evidence.command ?? 'Command unavailable'}</code>
          <span>{command.evidence.status ?? 'Unknown status'}{command.evidence.exitCode != null ? ` · exit ${command.evidence.exitCode}` : ''}</span>
        </li>)}</ul></>}
      </section>
      <section className="run-review-section run-review-decision">
        <h4>User review decision</h4>
        {unit.reviewDecision && <p className="run-review-note"><strong>{developmentReviewDecisionLabels[unit.reviewDecision]}</strong>{unit.reviewedAt ? ` · ${formatPersistedTimestamp(unit.reviewedAt) ?? unit.reviewedAt}` : ''}{unit.reviewNote ? ` · ${unit.reviewNote}` : ''}</p>}
        <label>Review note or rework guidance
          <textarea rows={2} maxLength={2000} value={decisionNote} onChange={(event) => onDecisionNoteChange(event.target.value)} disabled={decisionSaving || runActive} />
        </label>
        {decisionError && <p className="run-review-error" role="alert">Could not save review decision: {decisionError}</p>}
        <div className="run-review-decisions">
          <button className="button button-primary" type="button" onClick={() => onSaveDecision('approved_completion', decisionNote)} disabled={decisionSaving || runActive}>{decisionSaving ? 'Saving…' : 'Approve completion'}</button>
          <button className="button button-outline" type="button" onClick={() => onSaveDecision('rework_requested', decisionNote)} disabled={decisionSaving || runActive}>Request rework</button>
          <button className="button button-outline" type="button" onClick={() => onSaveDecision('blocked', decisionNote)} disabled={decisionSaving || runActive}>Keep blocked</button>
          <button className="button button-outline" type="button" onClick={() => onSaveDecision('incomplete', decisionNote)} disabled={decisionSaving || runActive}>Keep incomplete</button>
        </div>
      </section>
    </div>}
  </div>
}

function App() {
  const [nodes, setNodes, onNodesChange] = useNodesState<HeimNode>(initialNodes)
  const [edges, setEdges, onEdgesChange] = useEdgesState<HeimEdge>(initialEdges)
  const [newEdgeRelation, setNewEdgeRelation] = useState<MapRelation | ''>('')
  const [selectedEdgeId, setSelectedEdgeId] = useState<string | null>(null)
  const [selectedNodeId, setSelectedNodeId] = useState('project-map')
  const [activeWorkspaceTab, setActiveWorkspaceTab] = useState<WorkspaceTab>('chat')
  const [activeDetailTab, setActiveDetailTab] = useState<DetailTab>('overview')
  const [draft, setDraft] = useState('')
  const [messages, setMessages] = useState<ChatMessage[]>([])
  const [agentVersion, setAgentVersion] = useState('')
  const [agentError, setAgentError] = useState('')
  const [agentRunning, setAgentRunning] = useState(false)
  const [initialDesignPending, setInitialDesignPending] = useState(false)
  const [approvals, setApprovals] = useState<Approval[]>([])
  const [changedFiles, setChangedFiles] = useState<string[]>([])
  const [mapProposals, setMapProposals] = useState<MapProposal[]>([])
  const [designDraft, setDesignDraft] = useState<DesignDraft | null>(null)
  const [draftEdit, setDraftEdit] = useState<DesignDraft['draft'] | null>(null)
  const [draftEditDirty, setDraftEditDirty] = useState(false)
  const [draftSelectedNodeId, setDraftSelectedNodeId] = useState('')
  const [draftSelectedPane, setDraftSelectedPane] = useState<'before' | 'after'>('after')
  const [developmentReady, setDevelopmentReady] = useState(false)
  const [developmentUnits, setDevelopmentUnits] = useState<DevelopmentUnit[]>([])
  const [developmentUnitsRefreshing, setDevelopmentUnitsRefreshing] = useState(false)
  const [recommendationState, setRecommendationState] = useState<RecommendationState | null>(null)
  const [recommendationRetry, setRecommendationRetry] = useState(0)
  const lastRecommendationRequest = useRef('')
  const [developmentRunReview, setDevelopmentRunReview] = useState<DevelopmentRunReview | null>(null)
  const [developmentRunReviewLoading, setDevelopmentRunReviewLoading] = useState<number | null>(null)
  const [developmentRunReviewError, setDevelopmentRunReviewError] = useState<{ runId: number; message: string } | null>(null)
  const [developmentReviewSavingRunId, setDevelopmentReviewSavingRunId] = useState<number | null>(null)
  const [developmentReviewErrors, setDevelopmentReviewErrors] = useState<Record<number, string>>({})
  const [developmentReviewNotes, setDevelopmentReviewNotes] = useState<Record<number, string>>({})
  const [developmentUnitSpecs, setDevelopmentUnitSpecs] = useState<Record<string, DevelopmentUnitSpec>>({})
  const [developmentUnitSpecDirty, setDevelopmentUnitSpecDirty] = useState<Record<string, boolean>>({})
  const [developmentUnitSpecLoading, setDevelopmentUnitSpecLoading] = useState<string | null>(null)
  const [developmentUnitSpecErrors, setDevelopmentUnitSpecErrors] = useState<Record<string, string>>({})
  const [developmentUnitSpecSaving, setDevelopmentUnitSpecSaving] = useState(false)
  const [developmentUnitSpecRetry, setDevelopmentUnitSpecRetry] = useState(0)
  const unitSpecRequests = useRef(new Set<string>())
  const [reviewFiles, setReviewFiles] = useState<string[]>([])
  const [reviewingMap, setReviewingMap] = useState(false)
  const [reviewError, setReviewError] = useState('')
  const [notice, setNotice] = useState('')
  const [backendStatus, setBackendStatus] = useState<BackendStatus>({ kind: 'checking' })
  const [project, setProject] = useState<Project | null>(null)
  const [projectHasContent, setProjectHasContent] = useState(false)
  const [recentProjects, setRecentProjects] = useState<Project[]>([])
  const [projectMenuOpen, setProjectMenuOpen] = useState(false)
  const [projectSidebarCollapsed, setProjectSidebarCollapsed] = useState(false)
  const [mapRevision, setMapRevision] = useState(0)
  const [mapStatusDismissed, setMapStatusDismissed] = useState(false)
  const [mapDirty, setMapDirty] = useState(false)
  const [editingNodeId, setEditingNodeId] = useState<string | null>(null)
  const [nodeTitle, setNodeTitle] = useState('')
  const [nodeSummary, setNodeSummary] = useState('')
  const [nodeResponsibilities, setNodeResponsibilities] = useState('')
  const [nodeStatus, setNodeStatus] = useState('')
  const [projectBusy, setProjectBusy] = useState(false)
  const [agentPanelHeight, setAgentPanelHeight] = useState(300)
  const [resizingAgentPanel, setResizingAgentPanel] = useState(false)
  const mainColumnRef = useRef<HTMLElement | null>(null)
  const projectMenuRef = useRef<HTMLDivElement | null>(null)
  const [flow, setFlow] = useState<ReactFlowInstance<HeimNode, HeimEdge> | null>(null)
  const noticeTimer = useRef<number | undefined>(undefined)
  const composerInput = useRef<HTMLTextAreaElement>(null)
  const mapDirtyRef = useRef(false)
  const projectId = project?.id
  const draftReviewActive = !!designDraft
  const progressKey = JSON.stringify(developmentUnits.filter((unit) => unit.projectId === projectId && unit.mapRevision === mapRevision).map((unit) => [unit.id, unit.nodeId, unit.status, unit.latestRunId, unit.latestRunState, unit.reviewDecision]))
  const recommendationKey = `${projectId}:${mapRevision}:${progressKey}:${recommendationRetry}`
  const currentRecommendationKey = useRef(recommendationKey)
  currentRecommendationKey.current = recommendationKey
  const recommendationUnitKey = projectId === undefined ? '' : `${projectId}:${mapRevision}:${selectedNodeId}`
  const canDisplayRecommendation = backendStatus.kind === 'ready' && projectId !== undefined && developmentReady && nodes.some((node) => node.type === 'component') && !mapDirty && !draftReviewActive && !initialDesignPending && !projectBusy && !reviewingMap && !developmentUnitsRefreshing && mapProposals.length === 0
  const canRequestRecommendation = canDisplayRecommendation && !!agentVersion && (!!developmentUnitSpecs[recommendationUnitKey] || !!developmentUnitSpecErrors[recommendationUnitKey]) && !agentRunning
  const nextRecommendation = canDisplayRecommendation && recommendationState?.key === recommendationKey && recommendationState.status === 'ready' && nodes.some((node) => node.type === 'component' && node.id === recommendationState.recommendation?.nodeId)
    ? recommendationState.recommendation : null
  const displayedRecommendationState = canDisplayRecommendation && recommendationState?.key === recommendationKey ? recommendationState : null
  const recommendationIsLoading = displayedRecommendationState?.status === 'loading'
  const displayedNodes = useMemo(() => nodes.map((node) => node.type === 'component' && node.id === nextRecommendation?.nodeId
    ? { ...node, data: { ...node.data, nextRecommendationRationale: nextRecommendation.rationale } }
    : node), [nodes, nextRecommendation])
  const isChatStart = !!project && nodes.length === 0 && !designDraft && !projectHasContent
  const showChatStarter = isChatStart && (messages.length === 0 || initialDesignPending)

  const projectMenuControl = (
    <div className="project-menu-anchor" ref={projectMenuRef}>
      <button
        className="project-switcher"
        type="button"
        aria-haspopup="menu"
        aria-expanded={projectMenuOpen}
        onClick={() => setProjectMenuOpen((open) => !open)}
      >
        <span className="project-name">{project?.name}</span>
        <ChevronDown size={16} />
      </button>
      {projectMenuOpen && <div className="project-menu" role="menu">
        <button type="button" role="menuitem" onClick={() => { setProjectMenuOpen(false); void chooseProject() }} disabled={backendStatus.kind !== 'ready' || projectBusy}>
          <FolderOpen size={15} /> Open another folder
        </button>
        <button type="button" role="menuitem" onClick={() => { setProjectMenuOpen(false); void chooseProject() }} disabled={backendStatus.kind !== 'ready' || projectBusy}>
          <FolderPlus size={15} /> New project
        </button>
      </div>}
    </div>
  )

  useEffect(() => {
    if (!projectMenuOpen) return
    const closeOnPointerDown = (event: PointerEvent) => {
      if (!(event.target instanceof globalThis.Node) || !projectMenuRef.current?.contains(event.target)) setProjectMenuOpen(false)
    }
    const closeOnEscape = (event: globalThis.KeyboardEvent) => {
      if (event.key === 'Escape') setProjectMenuOpen(false)
    }
    document.addEventListener('pointerdown', closeOnPointerDown)
    window.addEventListener('keydown', closeOnEscape)
    return () => {
      document.removeEventListener('pointerdown', closeOnPointerDown)
      window.removeEventListener('keydown', closeOnEscape)
    }
  }, [projectMenuOpen])

  useEffect(() => {
    if (!isTauri()) {
      setBackendStatus({ kind: 'browser' })
      return
    }

    let active = true
    invoke<RuntimeInfo>('runtime_info')
      .then((info) => {
        if (active) setBackendStatus({ kind: 'ready', info })
      })
      .catch((error: unknown) => {
        if (active) setBackendStatus({ kind: 'error', message: String(error) })
      })
    return () => { active = false }
  }, [])

  useEffect(() => {
    if (backendStatus.kind !== 'ready') return
    invoke<string>('agent_status').then(setAgentVersion).catch((error: unknown) => setAgentError(String(error)))
  }, [backendStatus.kind])

  useEffect(() => {
    if (backendStatus.kind !== 'ready') return
    let active = true
    invoke<Project[]>('recent_projects')
      .then((projects) => { if (active) setRecentProjects(projects) })
      .catch((error: unknown) => { if (active) setAgentError(String(error)) })
    return () => { active = false }
  }, [backendStatus.kind, projectId])

  useEffect(() => {
    if (!canDisplayRecommendation || projectId === undefined || recommendationState?.key === recommendationKey) return
    if (!canRequestRecommendation || lastRecommendationRequest.current === recommendationKey) return
    lastRecommendationRequest.current = recommendationKey
    let active = true
    let settled = false
    setRecommendationState({ key: recommendationKey, status: 'loading', recommendation: null })
    void invoke<NextDevelopmentRecommendation | null>('recommend_next_development_node', { projectId, mapRevision })
      .then((recommendation) => {
        settled = true
        if (!active) return
        setRecommendationState({ key: recommendationKey, status: recommendation ? 'ready' : 'none', recommendation })
      })
      .catch((error: unknown) => {
        settled = true
        if (active) setRecommendationState({ key: recommendationKey, status: 'error', recommendation: null, error: String(error) })
      })
    return () => {
      if (currentRecommendationKey.current !== recommendationKey) {
        active = false
        if (!settled && lastRecommendationRequest.current === recommendationKey) lastRecommendationRequest.current = ''
      }
    }
  }, [canDisplayRecommendation, canRequestRecommendation, projectId, mapRevision, recommendationKey, recommendationState?.key])

  useEffect(() => {
    if (projectId === undefined || backendStatus.kind !== 'ready') return
    let active = true
    let unlisten: (() => void) | undefined
    listen<AgentEvent>('agent-event', ({ payload }) => {
      if (!active || payload.projectId !== projectId) return
      if (payload.kind === 'delta' || payload.kind === 'message') {
        const id = payload.itemId ?? crypto.randomUUID()
        setMessages((current) => {
          const index = current.findIndex((message) => message.id === id)
          if (index < 0) return [...current, { id, text: payload.text, role: 'agent', time: new Date().toLocaleTimeString() }]
          return current.map((message) => message.id === id
            ? { ...message, text: payload.kind === 'message' ? payload.text : message.text + payload.text }
            : message)
        })
      } else if (payload.kind === 'protocol_error') {
        if (payload.itemId) setMessages((current) => current.filter((message) => message.id !== payload.itemId))
        setMessages((current) => [...current, { id: crypto.randomUUID(), role: 'status', text: `Agent response was not accepted: ${payload.text}`, time: new Date().toLocaleTimeString() }])
      } else if (payload.kind === 'approval' && payload.requestId !== undefined) {
        const requestId = payload.requestId
        setApprovals((current) => [...current, { id: requestId, text: payload.text }])
        setActiveWorkspaceTab('chat')
        showNotice(`Codex approval required: ${payload.text}`)
        if ('Notification' in window) {
          if (Notification.permission === 'granted') new Notification('Heim · Codex approval required', { body: payload.text })
          else if (Notification.permission === 'default') void Notification.requestPermission()
        }
      } else {
        if (payload.kind === 'changed') {
          let files: string[] = []
          try { files = JSON.parse(payload.text) as string[] } catch { /* Ignore malformed agent event. */ }
          setReviewFiles(files)
          setChangedFiles((current) => [...current, ...files])
          if (files.length && !mapDirtyRef.current) {
            setReviewError('')
            setReviewingMap(true)
            invoke<MapProposal | null>('propose_map_update', { projectId, changedFiles: files })
              .then((proposal) => {
                if (!active) return
                if (proposal) {
                  setMapProposals((current) => [...current, proposal])
                  setActiveWorkspaceTab('chat')
                } else {
                  setMessages((current) => [...current, { id: crypto.randomUUID(), role: 'status', text: 'Agent found no map changes needed for these files.', time: new Date().toLocaleTimeString() }])
                }
              })
              .catch((error: unknown) => { if (active) setReviewError(`Map review failed: ${String(error)}`) })
              .finally(() => { if (active) setReviewingMap(false) })
          }
        }
        if (payload.kind === 'files') setChangedFiles((current) => [...current, payload.text])
        if (payload.kind === 'done' || payload.kind === 'disconnected') {
          setDevelopmentUnitsRefreshing(true)
          setAgentRunning(false)
          setApprovals([])
          void invoke<DevelopmentUnit[]>('project_development_units', { projectId })
            .then(setDevelopmentUnits)
            .catch((error: unknown) => setAgentError(`Could not refresh development run status: ${String(error)}`))
            .finally(() => { if (active) setDevelopmentUnitsRefreshing(false) })
        }
        if (payload.kind !== 'changed') setMessages((current) => [...current, { id: crypto.randomUUID(), role: 'status', text: payload.text, time: new Date().toLocaleTimeString() }])
      }
    }).then((cleanup) => { if (active) unlisten = cleanup; else cleanup() })
      .catch((error: unknown) => setAgentError(String(error)))
    return () => { active = false; unlisten?.() }
  }, [projectId, backendStatus.kind])

  useEffect(() => {
    if (!resizingAgentPanel) return
    const move = (event: PointerEvent) => {
      const bounds = mainColumnRef.current?.getBoundingClientRect()
      if (!bounds) return
      const nextHeight = bounds.bottom - event.clientY
      const maxHeight = Math.min(520, bounds.height - 72 - 180)
      setAgentPanelHeight(Math.max(180, Math.min(maxHeight, nextHeight)))
    }
    const stop = () => setResizingAgentPanel(false)
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', stop)
    return () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', stop)
    }
  }, [resizingAgentPanel])

  useEffect(() => {
    setMapStatusDismissed(false)
  }, [projectId, mapRevision, developmentReady])

  const selectedNode = nodes.find(
    (node): node is ComponentFlowNode =>
      node.id === selectedNodeId && node.type === 'component',
  )
  const developmentUnitSpecKey = project && selectedNode
    ? `${project.id}:${mapRevision}:${selectedNode.id}`
    : ''
  const currentDevelopmentUnitSpec = developmentUnitSpecs[developmentUnitSpecKey]
  const currentUnitSpecDirty = developmentUnitSpecDirty[developmentUnitSpecKey] === true

  const relatedRelationships = useMemo(() => {
    return edges.flatMap((edge) => {
      if (edge.source !== selectedNodeId && edge.target !== selectedNodeId) return []
      const relatedId = edge.source === selectedNodeId ? edge.target : edge.source
      const node = nodes.find((item): item is ComponentFlowNode => item.id === relatedId && item.type === 'component')
      return node ? [{ edge, node, selectedIsSource: edge.source === selectedNodeId }] : []
    })
  }, [edges, nodes, selectedNodeId])

  useEffect(() => {
    if (!project || !selectedNode || !developmentReady || !agentVersion || agentRunning || projectBusy || mapDirty || draftReviewActive) return
    const key = developmentUnitSpecKey
    if (!key || developmentUnitSpecs[key] || developmentUnitSpecErrors[key] || unitSpecRequests.current.has(key)) return
    unitSpecRequests.current.add(key)
    setDevelopmentUnitSpecLoading(key)
    setAgentRunning(true)
    setDevelopmentUnitSpecErrors((current) => { const next = { ...current }; delete next[key]; return next })
    const projectId = project.id
    const nodeId = selectedNode.id
    void invoke<DevelopmentUnitSpec>('get_or_generate_development_unit_spec', {
      projectId,
      mapRevision,
      nodeId,
    }).then((spec) => {
      setDevelopmentUnitSpecs((current) => ({ ...current, [key]: spec }))
      setDevelopmentUnitSpecDirty((current) => ({ ...current, [key]: false }))
    }).catch((error: unknown) => {
      setDevelopmentUnitSpecErrors((current) => ({ ...current, [key]: String(error) }))
    }).finally(() => {
      unitSpecRequests.current.delete(key)
      setDevelopmentUnitSpecLoading((current) => current === key ? null : current)
      setAgentRunning(false)
    })
  }, [project, selectedNode, developmentUnitSpecKey, developmentReady, agentVersion, agentRunning, projectBusy, mapDirty, draftReviewActive, mapRevision, developmentUnitSpecs, developmentUnitSpecErrors, developmentUnitSpecRetry])

  const showNotice = (message: string) => {
    setNotice(message)
    window.clearTimeout(noticeTimer.current)
    noticeTimer.current = window.setTimeout(() => setNotice(''), 4200)
  }

  const markMapDirty = () => {
    mapDirtyRef.current = true
    setMapDirty(true)
  }

  const loadProject = async (next: Project) => {
    const map = await invoke<StoredMap>('load_project_map', { projectId: next.id })
    const pendingDraft = await invoke<DesignDraft | null>('pending_design_draft', { projectId: next.id })
    const canDevelop = await invoke<boolean>('development_ready', { projectId: next.id })
    const units = await invoke<DevelopmentUnit[]>('project_development_units', { projectId: next.id })
    const hasContent = await invoke<boolean>('project_has_content', { projectId: next.id })
    setNodes(map.nodes)
    setEdges(map.edges)
    setSelectedEdgeId(null)
    setNewEdgeRelation('')
    setProject(next)
    setProjectHasContent(hasContent)
    setDesignDraft(pendingDraft)
    setDraftEdit(pendingDraft?.draft ?? null)
    setDraftSelectedNodeId(pendingDraft?.draft.nodes[0]?.id ?? '')
    setDraftSelectedPane('after')
    setDraftEditDirty(false)
    setDevelopmentReady(canDevelop)
    setDevelopmentUnits(units)
    setDevelopmentRunReview(null)
    setDevelopmentRunReviewError(null)
    setDevelopmentRunReviewLoading(null)
    setDevelopmentReviewSavingRunId(null)
    setDevelopmentReviewErrors({})
    setDevelopmentReviewNotes({})
    setDevelopmentUnitSpecs({})
    setDevelopmentUnitSpecDirty({})
    setDevelopmentUnitSpecErrors({})
    setMapRevision(map.revision)
    setMapDirty(false)
    mapDirtyRef.current = false
    setSelectedNodeId(map.nodes.find((node) => node.type === 'component')?.id ?? '')
    setMessages([])
    setChangedFiles([])
    setApprovals([])
    setReviewFiles([])
    setReviewError('')
    setMapProposals(await invoke<MapProposal[]>('pending_map_proposals', { projectId: next.id }))
    setEditingNodeId(null)
    setActiveDetailTab('overview')
    setActiveWorkspaceTab('chat')

    void flow?.fitView({ duration: 350 })
  }

  const confirmSwitch = () => {
    if (initialDesignPending) { showNotice('Wait for the initial architecture map to finish before switching projects.'); return false }
    if (agentRunning) { showNotice('Cancel the running agent turn before switching projects.'); return false }
    return (!mapDirty && !draftEditDirty) || window.confirm('Unsaved map or design draft edits will be lost. Continue?')
  }

  const chooseProject = async () => {
    if (!confirmSwitch()) return
    setProjectBusy(true)
    try {
      const next = await invoke<Project | null>('pick_project')
      if (next) await loadProject(next)
    } catch (error) {
      showNotice(`Could not open project: ${String(error)}`)
    } finally {
      setProjectBusy(false)
    }
  }

  const openRecentProject = async (projectId: number) => {
    if (!confirmSwitch()) return
    setProjectBusy(true)
    try {
      const next = await invoke<Project>('reopen_project', { projectId })
      await loadProject(next)
    } catch (error) {
      showNotice(`Could not reopen project: ${String(error)}`)
    } finally {
      setProjectBusy(false)
    }
  }


  const saveMapDraft = async () => {
    if (!project || !mapDirty || designDraft) return
    setProjectBusy(true)
    try {
      const result = await invoke<MapEditDraftResult>('create_map_edit_draft', {
        projectId: project.id,
        map: {
          nodes: nodes.map(({ id, type, position, data, style, draggable, selectable, connectable, zIndex }) =>
            ({ id, type, position, data, style, draggable, selectable, connectable, zIndex })),
          edges: edges.map(({ id, source, target, sourceHandle, targetHandle, style, markerEnd, data }) =>
            ({ id, source, target, sourceHandle, targetHandle, style, markerEnd, data })),
          revision: mapRevision,
        },
      })
      setNodes(result.confirmedMap.nodes)
      setEdges(result.confirmedMap.edges)
      setMapRevision(result.confirmedMap.revision)
      setSelectedNodeId((current) => result.confirmedMap.nodes.some((node) => node.id === current)
        ? current
        : result.confirmedMap.nodes.find((node) => node.type === 'component')?.id ?? '')
      setSelectedEdgeId(null)
      setNewEdgeRelation('')
      setMapDirty(false)
      mapDirtyRef.current = false
      setEditingNodeId(null)
      setDesignDraft(result.draft)
      setDraftEdit(result.draft.draft)
      setDraftSelectedNodeId(result.draft.draft.nodes.find((node) => node.type === 'component')?.id ?? '')
      setDraftSelectedPane('after')
      setDraftEditDirty(false)
      showNotice('Map edits saved as a draft. The confirmed map is unchanged.')
    } catch (error) {
      showNotice(`Could not save map draft: ${String(error)}`)
    } finally {
      setProjectBusy(false)
    }
  }

  const updateNodeQuestionAnswer = (nodeId: string, question: string, answer: string) => {
    setNodes((current) => current.map((node) => node.id === nodeId && node.type === 'component'
      ? { ...node, data: { ...node.data, answers: { ...(node.data.answers ?? {}), [question]: answer } } }
      : node))
    markMapDirty()
  }

  const saveDraftEdits = async () => {
    if (!project || !designDraft || !draftEdit || !draftEditDirty) return
    setProjectBusy(true)
    try {
      const updated = await invoke<DesignDraft>('update_design_draft', {
        projectId: project.id,
        draftId: designDraft.id,
        expectedVersion: designDraft.version,
        draft: draftEdit,
      })
      setDesignDraft(updated)
      setDraftEdit(updated.draft)
      setDraftEditDirty(false)
      showNotice('Design draft edits saved. Confirmed map is unchanged.')
    } catch (error) {
      showNotice(`Could not save draft edits: ${String(error)}`)
    } finally {
      setProjectBusy(false)
    }
  }

  const resolveDesignDraft = async (accept: boolean) => {
    if (!project || !designDraft || draftEditDirty || agentRunning) return
    setProjectBusy(true)
    try {
      const map = await invoke<StoredMap | null>('resolve_design_draft', {
        projectId: project.id,
        draftId: designDraft.id,
        expectedVersion: designDraft.version,
        accept,
      })
      if (map) {
        setNodes(map.nodes)
        setEdges(map.edges)
        setMapRevision(map.revision)
        setSelectedNodeId(map.nodes.find((node) => node.type === 'component')?.id ?? '')
        mapDirtyRef.current = false
        setMapDirty(false)
        setDevelopmentReady(true)
      } else if (nodes.length === 0) {
        setDevelopmentReady(false)
      }
      setDesignDraft(null)
      setDraftEdit(null)
      setDraftEditDirty(false)
      setMessages((current) => [...current, {
        id: crypto.randomUUID(), role: 'status',
        text: accept ? 'Design approved and saved as the confirmed map. Code changes require a separate development request.' : 'Design draft rejected. The confirmed map was not changed.',
        time: new Date().toLocaleTimeString(),
      }])
    } catch (error) {
      showNotice(`Could not ${accept ? 'approve' : 'reject'} design draft: ${String(error)}`)
    } finally {
      setProjectBusy(false)
    }
  }

  const updateDraftNode = (nodeId: string, update: (node: HeimNode) => HeimNode) => {
    setDraftEdit((current) => current ? { ...current, nodes: current.nodes.map((node) => node.id === nodeId ? update(node) : node) } : current)
    setDraftEditDirty(true)
  }


  const addNode = () => {
    if (!project) return
    const id = crypto.randomUUID()
    const addedNodeCount = nodes.filter((node) => node.type === 'component').length - 8
    const offset = Math.max(0, addedNodeCount)
    const nextNode: ComponentFlowNode = {
      id,
      type: 'component',
      position: { x: 430 + (offset % 3) * 35, y: 340 + Math.floor(offset / 3) * 35 },
      data: {
        title: 'New component',
        kind: 'System responsibility',
        summary: 'Describe this system responsibility and what is outside its scope.',
        technology: 'Not decided',
        status: 'Draft',
        icon: 'map',
        responsibilities: ['Define the goal and responsibilities'],
        files: [],
      },
    }

    setNodes((current) => [...current, nextNode])
    markMapDirty()
    setSelectedNodeId(id)
    setActiveDetailTab('overview')
  }

  const editNode = () => {
    if (!selectedNode || !project) return
    setEditingNodeId(selectedNode.id)
    setNodeTitle(selectedNode.data.title)
    setNodeSummary(selectedNode.data.summary)
    setNodeResponsibilities(selectedNode.data.responsibilities.join('\n'))
    setNodeStatus(selectedNode.data.status)
    setActiveDetailTab('overview')
  }

  const updateNode = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (projectBusy) return
    const responsibilities = nodeResponsibilities.split('\n').map((item) => item.trim()).filter(Boolean)
    if (!editingNodeId || !nodeTitle.trim() || !nodeSummary.trim() || !nodeStatus.trim() || nodeStatus.trim().length > 80 || responsibilities.length === 0 || !project) return
    setNodes((current) => current.map((node) => node.id === editingNodeId && node.type === 'component'
      ? { ...node, data: { ...node.data, title: nodeTitle.trim(), summary: nodeSummary.trim(), responsibilities, status: nodeStatus.trim() } }
      : node))
    markMapDirty()
    setEditingNodeId(null)
  }

  const handleConnect = (connection: Connection) => {
    const { source, target } = connection
    if (!project || !newEdgeRelation || !source || !target || source === target) return
    const label = relationLabel(newEdgeRelation)
    setEdges((current) => [...current, {
      id: crypto.randomUUID(),
      source,
      target,
      sourceHandle: connection.sourceHandle,
      targetHandle: connection.targetHandle,
      type: 'smoothstep',
      data: { relation: newEdgeRelation },
      label,
      style: { stroke: '#aeb8c9', strokeWidth: 1.6 },
      markerEnd: { type: MarkerType.ArrowClosed, color: '#aeb8c9' },
    }])
    setSelectedEdgeId(null)
    setNewEdgeRelation('')
    markMapDirty()
  }

  const saveDevelopmentUnitSpec = async () => {
    if (!project || !selectedNode || !currentDevelopmentUnitSpec) return null
    try {
      const saved = await invoke<DevelopmentUnitSpec>('save_development_unit_spec', {
        projectId: project.id,
        mapRevision,
        nodeId: selectedNode.id,
        expectedVersion: currentDevelopmentUnitSpec.version,
        goal: currentDevelopmentUnitSpec.goal,
        completionCriteria: currentDevelopmentUnitSpec.completionCriteria,
      })
      setDevelopmentUnitSpecs((current) => ({ ...current, [developmentUnitSpecKey]: saved }))
      setDevelopmentUnitSpecDirty((current) => ({ ...current, [developmentUnitSpecKey]: false }))
      setDevelopmentUnitSpecErrors((current) => { const next = { ...current }; delete next[developmentUnitSpecKey]; return next })
      return saved
    } catch (error) {
      setDevelopmentUnitSpecErrors((current) => ({ ...current, [developmentUnitSpecKey]: String(error) }))
      return null
    }
  }

  const saveSelectedDevelopmentUnitSpec = async () => {
    if (developmentUnitSpecSaving) return
    setDevelopmentUnitSpecSaving(true)
    await saveDevelopmentUnitSpec()
    setDevelopmentUnitSpecSaving(false)
  }

  const retryDevelopmentUnitSpec = () => {
    setDevelopmentUnitSpecErrors((current) => { const next = { ...current }; delete next[developmentUnitSpecKey]; return next })
    setDevelopmentUnitSpecRetry((current) => current + 1)
  }

  const activateDevelopmentForSelectedNode = () => {
    if (!currentDevelopmentUnitSpec) {
      retryDevelopmentUnitSpec()
      return
    }
    void startDevelopment()
  }

  const toggleDevelopmentRunReview = async (runId: number) => {
    if (!project) return
    if (developmentRunReview?.runId === runId) {
      setDevelopmentRunReview(null)
      setDevelopmentRunReviewError(null)
      return
    }
    setDevelopmentRunReview(null)
    setDevelopmentRunReviewError(null)
    setDevelopmentRunReviewLoading(runId)
    const projectId = project.id
    try {
      const review = await invoke<DevelopmentRunReview>('development_unit_run_review', { projectId, runId })
      if (project?.id === projectId) setDevelopmentRunReview(review)
    } catch (error) {
      if (project?.id === projectId) setDevelopmentRunReviewError({ runId, message: String(error) })
    } finally {
      setDevelopmentRunReviewLoading((current) => current === runId ? null : current)
    }
  }

  const saveDevelopmentReview = async (
    unit: DevelopmentUnit,
    decision: DevelopmentReviewDecision,
    note: string,
  ) => {
    if (!project || !unit.latestRunId || developmentReviewSavingRunId !== null) return
    if (decision === 'approved_completion' && !window.confirm(
      'Approve this development unit as complete? This records your review decision only; it does not change the architecture node status.',
    )) return

    const projectId = project.id
    const runId = unit.latestRunId
    setDevelopmentReviewSavingRunId(runId)
    setDevelopmentReviewErrors((current) => { const next = { ...current }; delete next[runId]; return next })
    try {
      const saved = await invoke<StoredDevelopmentReviewDecision>('save_development_unit_review', {
        projectId,
        unitId: unit.id,
        runId,
        decision,
        note,
      })
      if (project?.id === projectId) {
        setDevelopmentUnits((current) => current.map((item) => item.id === unit.id
          ? { ...item, reviewDecision: saved.decision, reviewNote: saved.note, reviewedAt: saved.reviewedAt }
          : item))
        setDevelopmentReviewNotes((current) => ({ ...current, [runId]: saved.note ?? '' }))
      }
    } catch (error) {
      if (project?.id === projectId) setDevelopmentReviewErrors((current) => ({ ...current, [runId]: String(error) }))
    } finally {
      setDevelopmentReviewSavingRunId((current) => current === runId ? null : current)
    }
  }

  const startDevelopment = async () => {
    if (!project || !selectedNode || !currentDevelopmentUnitSpec || !agentVersion || agentRunning || developmentUnitSpecSaving || draftReviewActive || mapDirty || !developmentReady || !currentDevelopmentUnitSpec.goal.trim() || currentDevelopmentUnitSpec.completionCriteria.length === 0 || unansweredQuestions(selectedNode.data).length > 0) return
    setAgentRunning(true)
    try {
      let spec = currentDevelopmentUnitSpec
      if (currentUnitSpecDirty) {
        setDevelopmentUnitSpecSaving(true)
        const saved = await saveDevelopmentUnitSpec()
        setDevelopmentUnitSpecSaving(false)
        if (!saved) throw new Error(developmentUnitSpecErrors[developmentUnitSpecKey] ?? 'Could not save the edited unit plan')
        spec = saved
      }
      const { unitId } = await invoke<{ unitId: number; runId: number }>('send_development_message', {
        projectId: project.id,
        mapRevision,
        nodeId: selectedNode.id,
        goal: spec.goal.trim(),
        completionCriteria: spec.completionCriteria,
      })
      setMessages((current) => [...current, { id: crypto.randomUUID(), role: 'status', text: `Started development unit #${unitId} for ${selectedNode.data.title} against map revision ${mapRevision}.`, time: new Date().toLocaleTimeString() }])
      void invoke<DevelopmentUnit[]>('project_development_units', { projectId: project.id })
        .then(setDevelopmentUnits)
        .catch((error: unknown) => setAgentError(`Could not refresh development run status: ${String(error)}`))
    } catch (error) {
      setAgentRunning(false)
      setDevelopmentUnitSpecSaving(false)
      setMessages((current) => [...current, { id: crypto.randomUUID(), role: 'status', text: `Could not start development: ${String(error)}`, time: new Date().toLocaleTimeString() }])
      void invoke<DevelopmentUnit[]>('project_development_units', { projectId: project.id })
        .then(setDevelopmentUnits)
        .catch((refreshError: unknown) => setAgentError(`Could not refresh development units: ${String(refreshError)}`))
    }
  }

  const sendMessage = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const text = draft.trim()
    if (!text || !project || !agentVersion || agentRunning || initialDesignPending || reviewingMap || draftEditDirty || mapProposals.length > 0 || reviewError || (nodes.length > 0 && !draftReviewActive && (!selectedNode || !developmentReady))) return
    if (mapDirty) {
      showNotice('Save map edits as a draft and review them before sending a chat message.')
      return
    }

    const creatingDraft = nodes.length === 0 && !designDraft
    if (creatingDraft) setInitialDesignPending(true)
    const id = crypto.randomUUID()
    setMessages((current) => [
      ...current,
      {
        id,
        text,
        role: 'user',
        time: new Intl.DateTimeFormat(undefined, {
          hour: 'numeric',
          minute: '2-digit',
        }).format(new Date()),
      },
    ])
    if (!creatingDraft) setDraft('')
    setAgentRunning(true)
    try {
      if (creatingDraft) {
        const savedDraft = await invoke<DesignDraft>('create_design_draft', { projectId: project.id, description: text })
        setDesignDraft(savedDraft)
        setDraft('')
        setDraftEdit(savedDraft.draft)
        setDraftSelectedNodeId(savedDraft.draft.nodes[0]?.id ?? '')
        setDraftSelectedPane('after')
        setDraftEditDirty(false)
        setMessages((current) => [...current, { id: crypto.randomUUID(), role: 'status', text: `Design draft v${savedDraft.version} saved for review. The confirmed map is unchanged.`, time: new Date().toLocaleTimeString() }])
      } else if (draftReviewActive && designDraft) {
        const revisedDraft = await invoke<DesignDraft>('revise_design_draft', {
          projectId: project.id,
          draftId: designDraft.id,
          expectedVersion: designDraft.version,
          request: text,
        })
        setDesignDraft(revisedDraft)
        setDraftEdit(revisedDraft.draft)
        setDraftSelectedNodeId(revisedDraft.draft.nodes[0]?.id ?? '')
        setDraftSelectedPane('after')
        setDraftEditDirty(false)
        setMessages((current) => [...current, { id: crypto.randomUUID(), role: 'status', text: `Agent revision saved as draft v${revisedDraft.version}. The confirmed map is unchanged.`, time: new Date().toLocaleTimeString() }])
      } else {
        await invoke('send_design_message', { projectId: project.id, text })
      }
    } catch (error) {
      setAgentRunning(false)
      setMessages((current) => [...current, { id: crypto.randomUUID(), role: 'status', text: `Could not send message: ${String(error)}`, time: new Date().toLocaleTimeString() }])
    } finally {
      if (creatingDraft) setInitialDesignPending(false)
    }
  }

  const cancelAgent = async () => {
    if (!project) return
    try { await invoke('cancel_agent_turn', { projectId: project.id }) }
    catch (error) { showNotice(`Could not cancel agent: ${String(error)}`) }
  }

  const answerApproval = async (decision: 'accept' | 'decline') => {
    const approval = approvals[0]
    if (!project || !approval) return
    try {
      await invoke('answer_agent_approval', { projectId: project.id, requestId: approval.id, decision })
      setApprovals((current) => current.filter((item) => item.id !== approval.id))
    } catch (error) { showNotice(`Could not answer approval: ${String(error)}`) }
  }

  const reviewMapChanges = async () => {
    if (!project || !reviewFiles.length || mapDirty || reviewingMap) return
    setReviewError('')
    setReviewingMap(true)
    try {
      const proposal = await invoke<MapProposal | null>('propose_map_update', { projectId: project.id, changedFiles: reviewFiles })
      if (proposal) setMapProposals((current) => [...current, proposal])
      else showNotice('No map changes needed for these files.')
    } catch (error) { setReviewError(`Map review failed: ${String(error)}`) }
    finally { setReviewingMap(false) }
  }

  const decideMapProposal = async (accept: boolean) => {
    const proposal = mapProposals[0]
    if (!project || !proposal || (accept && (mapDirty || draftReviewActive))) return
    setProjectBusy(true)
    try {
      const map = await invoke<StoredMap | null>('resolve_map_proposal', { projectId: project.id, proposalId: proposal.id, accept })
      if (map) {
        setNodes(map.nodes)
        setEdges(map.edges)
        setMapRevision(map.revision)
        // Accepting a reviewed map-alignment proposal advances the revision of
        // the already-approved map; it does not create an unapproved design draft.
        setDevelopmentReady(true)
        setSelectedNodeId((current) => map.nodes.some((node) => node.id === current) ? current : map.nodes.find((node) => node.type === 'component')?.id ?? '')
      }
      setMapProposals((current) => current.filter((item) => item.id !== proposal.id))
      setReviewFiles([])
      setReviewError('')
      showNotice(accept ? 'Map updated to match the reviewed code changes.' : 'Map proposal rejected; source changes were not reverted.')
    } catch (error) { showNotice(`Could not apply map proposal: ${String(error)}`) }
    finally { setProjectBusy(false) }
  }

  const handleComposerKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
      event.preventDefault()
      event.currentTarget.form?.requestSubmit()
    }
  }

  const clearChat = () => {
    setMessages([])
    setDraft('')
    showNotice('Chat view cleared; Agent session still remembers this conversation.')
  }

  const activeNodeData = selectedNode?.data
  const beforeSelectedNode = nodes.find((node) => node.id === draftSelectedNodeId)
  const draftSelectedNode = draftEdit?.nodes.find((node) => node.id === draftSelectedNodeId)
  const activeDraftNode = draftSelectedPane === 'before' ? beforeSelectedNode : draftSelectedNode
  const DraftNodeIcon = activeDraftNode?.type === 'component' ? iconByName[activeDraftNode.data.icon] : null
  const isUiDemo = import.meta.env.DEV && new URLSearchParams(window.location.search).has('demo')

  if (isUiDemo) return <DemoWorkspace />

  return (
    <main className="app-shell">
      <div className={`workspace${!project ? ' startup-workspace' : isChatStart ? ' chat-start-workspace' : ` has-project-sidebar${projectSidebarCollapsed ? ' is-sidebar-collapsed' : ''}`}`}>
        {!project ? <section className="startup-screen" aria-label="Start Heim">
          <div className="startup-card">
            <div className="startup-icon"><Workflow size={30} /></div>
            <h1>Start with a project</h1>
            <p>Open an existing project folder or begin a new design from an empty folder.</p>
            <div className="startup-actions">
              <button className="button button-primary" type="button" onClick={chooseProject} disabled={backendStatus.kind !== 'ready' || projectBusy}>Open folder</button>
              <button className="button button-outline" type="button" onClick={chooseProject} disabled={backendStatus.kind !== 'ready' || projectBusy}>Start new project</button>
            </div>
            <small>{backendStatus.kind === 'ready' ? 'Your project files stay on this computer.' : 'Desktop backend is connecting…'}</small>
          </div>
        </section> : <>
        {!isChatStart && <aside className="recent-project-sidebar" aria-label="Project folders">
          <div className="project-sidebar-header">
            {!projectSidebarCollapsed && <span>Projects</span>}
            <button className="project-sidebar-toggle" type="button" onClick={() => setProjectSidebarCollapsed((collapsed) => !collapsed)} aria-label={projectSidebarCollapsed ? 'Expand project sidebar' : 'Collapse project sidebar'} title={projectSidebarCollapsed ? 'Expand sidebar' : 'Collapse sidebar'}>
              {projectSidebarCollapsed ? <ChevronRight size={17} /> : <ChevronLeft size={17} />}
            </button>
          </div>
          <div className="recent-project-list">
            {recentProjects.map((recent) => (
              <button
                className={`recent-project-item${project.id === recent.id ? ' is-active' : ''}`}
                type="button"
                key={recent.id}
                onClick={() => void openRecentProject(recent.id)}
                disabled={projectBusy}
                title={recent.root}
              >
                <span><strong>{recent.name}</strong><small>{recent.root}</small></span>
              </button>
            ))}
          </div>
        </aside>}
        <section ref={mainColumnRef} className={`main-column${resizingAgentPanel ? ' is-resizing' : ''}${isChatStart ? ' is-chat-start' : ''}`} style={{ gridTemplateRows: isChatStart ? 'minmax(0, 1fr)' : `72px minmax(180px, 1fr) ${agentPanelHeight}px` }} aria-label="Project map and agent workspace">
          {isChatStart && project && <div className="chat-start-project-floating">{projectMenuControl}</div>}
          <div className="map-toolbar">
            <div className="map-heading-icon"><Workflow size={21} /></div>
            <div className="map-heading-copy">{!isChatStart && projectMenuControl}</div>
            <div className="map-toolbar-actions">
              {developmentReady && <span className="recommendation-status" role="status" title={displayedRecommendationState?.error ?? nextRecommendation?.rationale}>
                {displayedRecommendationState?.status === 'loading' ? 'Finding next…' : displayedRecommendationState?.status === 'error' ? 'Next unavailable' : displayedRecommendationState?.status === 'none' || displayedRecommendationState?.status === 'ready' && !nextRecommendation ? 'No next node' : nextRecommendation ? 'Next suggested' : ''}
              </span>}
              {displayedRecommendationState?.status === 'error' && <button className="button button-subtle recommendation-retry" type="button" onClick={() => setRecommendationRetry((current) => current + 1)} aria-label="Retry next-node recommendation">Retry</button>}
              {project && <button
                className="button button-development"
                type="button"
                onClick={activateDevelopmentForSelectedNode}
                disabled={!agentVersion || agentRunning || developmentUnitSpecSaving || draftReviewActive || mapDirty || projectBusy || !developmentReady || !selectedNode || developmentUnitSpecLoading === developmentUnitSpecKey || (!!currentDevelopmentUnitSpec && (!currentDevelopmentUnitSpec.goal.trim() || currentDevelopmentUnitSpec.completionCriteria.length === 0 || currentDevelopmentUnitSpec.completionCriteria.length > 20 || currentDevelopmentUnitSpec.completionCriteria.some((criterion) => criterion.length > 1000))) || (!selectedNode || unansweredQuestions(selectedNode.data).length > 0)}
                title={!selectedNode ? 'Select a node first' : !agentVersion ? 'Connect to the Agent first' : agentRunning ? 'Wait for the current Agent request to finish' : developmentUnitSpecLoading === developmentUnitSpecKey ? 'AI is preparing this node’s development unit' : !currentDevelopmentUnitSpec ? 'Generate a development plan for this node' : `Develop ${selectedNode.data.title} using its saved unit plan`}
                aria-label={selectedNode ? currentDevelopmentUnitSpec ? `Develop selected node ${selectedNode.data.title}` : `Generate a development plan for ${selectedNode.data.title}` : 'Select a node to develop'}
              >
                <Code2 size={15} /> <span className="develop-label">{developmentUnitSpecLoading === developmentUnitSpecKey ? 'Preparing…' : currentDevelopmentUnitSpec ? 'Develop selected node' : 'Generate plan'}</span>
              </button>}
              {!draftReviewActive && <button className="button button-outline" type="button" onClick={addNode} disabled={!project || projectBusy}>
                <Plus size={16} /> Add node
              </button>}
              {!draftReviewActive && <select
                className="map-relation-select"
                aria-label={selectedEdgeId ? 'Relationship type for selected connection' : 'Relationship type for next connection'}
                value={newEdgeRelation}
                onChange={(event) => {
                  const relation = event.target.value as MapRelation | ''
                  if (selectedEdgeId && relation) {
                    setEdges((current) => current.map((edge) => edge.id === selectedEdgeId
                      ? { ...edge, data: { relation }, label: relationLabel(relation) }
                      : edge))
                    setSelectedEdgeId(null)
                    setNewEdgeRelation('')
                    markMapDirty()
                  } else {
                    setNewEdgeRelation(relation)
                  }
                }}
                disabled={!project || projectBusy}
                title={selectedEdgeId ? 'Choose a type for the selected relationship' : 'Choose the relationship type, then connect two nodes'}
              >
                <option value="">{selectedEdgeId ? 'Classify selected edge…' : 'Choose relationship…'}</option>
                <option value="depends_on">A depends on B →</option>
                <option value="interacts_with">A interacts with B →</option>
              </select>}
              <button className="button button-primary" type="button" onClick={saveMapDraft} disabled={!project || !mapDirty || projectBusy || draftReviewActive}>Save as draft</button>
              <button
                className="button button-subtle fit-button"
                type="button"
                onClick={() => flow?.fitView({ duration: 350, padding: 0.08 })}
                title="Fit map to view"
              >
                <LayoutGrid size={16} /> <span>Fit map</span>
              </button>
              <button className="icon-button more-button" type="button" onClick={() => showNotice('Map options will be available as editing features are added.')} aria-label="More map options">
                <MoreHorizontal size={19} />
              </button>
            </div>
          </div>

          <div className="map-canvas">
            <ReactFlow<HeimNode, HeimEdge>
              nodes={displayedNodes}
              edges={edges.map(edgeForDisplay)}
              nodeTypes={nodeTypes}
              onNodesChange={(changes) => {
                onNodesChange(changes)
                if (project && changes.some((change) => change.type === 'position' || change.type === 'remove')) markMapDirty()
              }}
              onEdgesChange={(changes) => {
                onEdgesChange(changes)
                if (project && changes.some((change) => change.type === 'remove' || change.type === 'add')) markMapDirty()
              }}
              onConnect={handleConnect}
              onEdgeClick={(_, edge) => {
                setSelectedEdgeId(edge.id)
                setNewEdgeRelation('')
              }}
              onNodeClick={(_, node) => {
                if (node.type === 'component') {
                  setSelectedNodeId(node.id)
                  setActiveDetailTab('overview')
                }
              }}
              onInit={setFlow}
              fitView
              fitViewOptions={{ padding: 0.07 }}
              minZoom={0.25}
              maxZoom={1.5}
              deleteKeyCode={null}
              nodesConnectable={!!project && !!newEdgeRelation && !projectBusy && !draftReviewActive}
              nodesDraggable={!!project && !projectBusy && !draftReviewActive}
              elementsSelectable
              proOptions={{ hideAttribution: true }}
            >
              <Background variant={BackgroundVariant.Dots} gap={18} size={1} color="#dce2ec" />
              <Controls showInteractive={false} />
              <MiniMap
                style={{ width: 160, height: 120 }}
                pannable
                zoomable
                nodeColor={(node) => node.type === 'layer' ? '#e6eaf2' : '#dce7ff'}
                maskColor="rgba(248, 250, 253, 0.72)"
              />
            </ReactFlow>
            {designDraft && draftEdit && <div className={`draft-review-overlay${nodes.length === 0 ? ' is-initial' : ''}`} aria-label="Before and after design draft">
              {nodes.length > 0 && <section className="draft-graph-pane"><header><strong>Before · confirmed map</strong><span>노드를 선택하면 확정본 정보가 표시됩니다</span></header><ReactFlow nodes={nodes} edges={edges.map(edgeForDisplay)} nodeTypes={nodeTypes} fitView fitViewOptions={{ padding: 0.18 }} nodesDraggable={false} nodesConnectable={false} onNodeClick={(_, node) => { if (node.type === 'component') { setDraftSelectedNodeId(node.id); setDraftSelectedPane('before') } }} zoomOnDoubleClick={false} panOnDrag><Background variant={BackgroundVariant.Dots} gap={18} size={1} color="#dce2ec" /></ReactFlow></section>}
              <section className="draft-graph-pane draft-after-pane"><header><div><strong>After · draft v{designDraft.version}</strong><span>{draftEdit.nodes.length}개 노드 · {draftEdit.edges.length}개 관계 · 노드를 눌러 상세 확인</span></div><button className="button button-primary" type="button" onClick={() => resolveDesignDraft(true)} disabled={draftEditDirty || projectBusy || agentRunning}>Approve map</button></header><ReactFlow nodes={draftEdit.nodes} edges={draftEdit.edges.map(edgeForDisplay)} nodeTypes={nodeTypes} fitView fitViewOptions={{ padding: 0.18 }} nodesDraggable={false} nodesConnectable={false} onNodeClick={(_, node) => { if (node.type === 'component') { setDraftSelectedNodeId(node.id); setDraftSelectedPane('after') } }}><Background variant={BackgroundVariant.Dots} gap={18} size={1} color="#dce2ec" /><Controls showInteractive={false} /></ReactFlow><div className="draft-graph-footer">{draftEditDirty ? 'Save edits in the node details panel before approving.' : 'Select a node to edit its name, description, or open questions.'}<button className="button button-outline" type="button" onClick={() => resolveDesignDraft(false)} disabled={draftEditDirty || projectBusy || agentRunning}>Reject draft</button><button className="button button-outline" type="button" onClick={saveDraftEdits} disabled={!draftEditDirty || projectBusy || agentRunning}>Save edits</button></div></section>
            </div>}
          </div>

          <section className="agent-panel" aria-label="Agent workspace">
            <button className="agent-panel-resizer" type="button" aria-label="Resize bottom workspace" onPointerDown={(event) => { event.preventDefault(); setResizingAgentPanel(true) }} title="Drag to resize" />
            <div className="agent-panel-header">
              <div className="workspace-tabs" role="tablist" aria-label="Workspace panels">
                {workspaceTabs.map(({ id, label, icon: Icon }) => (
                  <button
                    type="button"
                    role="tab"
                    aria-selected={activeWorkspaceTab === id}
                    className={`workspace-tab${activeWorkspaceTab === id ? ' is-active' : ''}`}
                    key={id}
                    onClick={() => setActiveWorkspaceTab(id)}
                  >
                    <Icon size={15} /> {label}
                  </button>
                ))}
              </div>
              <div className="agent-panel-actions">
                <span className="selected-context-label"><Workflow size={15} /> Talking about <strong>{activeNodeData?.title ?? 'Project'}</strong></span>
                <span className="agent-offline" title={agentError || agentVersion}><span className={agentVersion ? 'is-ready' : ''} /> {agentRunning ? 'Agent working' : agentVersion ? 'Codex ready' : 'Agent unavailable'}</span>
                {agentRunning && <button className="text-action" type="button" onClick={cancelAgent}>Stop</button>}
                {activeWorkspaceTab === 'chat' && (
                  <button className="text-action" type="button" onClick={clearChat} disabled={messages.length === 0}>
                    <X size={14} /> Clear
                  </button>
                )}
                <button className="icon-button compact-icon-button" type="button" onClick={() => showNotice('Panel options are not available in this preview.')} aria-label="Workspace options">
                  <MoreHorizontal size={17} />
                </button>
              </div>
            </div>

            {(isChatStart || activeWorkspaceTab === 'chat') && (
              <div className={`chat-panel${showChatStarter ? ' chat-panel-starter' : ''}`}>
                {nodes.length > 0 && !mapStatusDismissed && <div className="map-review map-status-notice" role="status">
                  <span>{developmentReady
                    ? `Approved map revision ${mapRevision}. Define and confirm a target, goal, and completion criteria before starting development.`
                    : 'Development-unit definition is locked until this map revision has an approved design draft.'}</span>
                  <button type="button" className="map-status-dismiss" aria-label="Dismiss map status notice" onClick={() => setMapStatusDismissed(true)}><X size={16} /></button>
                </div>}
                <div className="chat-messages" aria-live="polite">
                  {showChatStarter && <div className="chat-start-intro"><h1>무엇을 함께 만들어볼까요?</h1></div>}
                  {!initialDesignPending && messages.map((message) => (
                    <article className={`chat-message ${message.role}-message`} key={message.id}>
                      <div className="message-avatar">{message.role === 'user' ? 'J' : <Sparkles size={14} />}</div>
                      <div className="message-body">
                        <div className="message-meta"><strong>{message.role === 'user' ? 'You' : message.role === 'agent' ? 'Codex' : 'Activity'}</strong><time>{message.time}</time></div>
                        <p>{message.text}</p>
                      </div>
                    </article>
                  ))}
                {approvals.length > 0 && <div className="agent-approval" role="alert">
                  <strong>Codex requests approval{approvals.length > 1 ? ` (${approvals.length} pending)` : ''}</strong><span>{approvals[0].text}</span>
                  <button className="button button-outline" type="button" onClick={() => answerApproval('decline')}>Decline</button>
                  <button className="button button-primary" type="button" onClick={() => answerApproval('accept')}>Approve once</button>
                </div>}
                {designDraft && <div className="map-review" aria-label="Saved design draft">
                  <strong>Unconfirmed design draft · v{designDraft.version}</strong>
                  <p>{designDraft.draft.goal}</p>
                  <span>{designDraft.draft.nodes.length} planned nodes · {designDraft.draft.edges.length} relationships · base map revision {designDraft.baseRevision}</span>
                  {designDraft.draft.questions.map((question, index) => <small key={`q-${index}`}>Question: {question}</small>)}
                  {designDraft.draft.assumptions.map((assumption, index) => <small key={`a-${index}`}>Assumption: {assumption}</small>)}
                </div>}
                {draftReviewActive && draftEditDirty && <div className="map-review" role="status">Save your direct After edits before asking the Agent to revise the draft.</div>}
                {reviewingMap && <div className="map-review" role="status">Agent is comparing the changed code with the project map…</div>}
                {mapProposals.length > 0 && <div className="map-review" aria-label="Map change proposal">
                  <strong>Agent map update · {mapProposals.length} pending</strong>
                  <p>Based on changed files: {mapProposals[0].changedFiles.join(' · ')}</p>
                  {mapProposals[0].changes.updates.map((update) => <div className="map-review-item" key={update.nodeId}>
                    <strong>Update {componentTitle(nodes, update.nodeId)}</strong>
                    <span>{update.summary} · {update.status}</span><small>{update.files.join(' · ')} {update.removeFiles.length > 0 && `· remove ${update.removeFiles.join(' · ')}`} — {update.reason}</small><code>{update.evidence}</code>
                  </div>)}
                  {mapProposals[0].changes.additions.map((addition, index) => <div className="map-review-item" key={`${addition.title}-${index}`}>
                    <strong>Add {addition.title}</strong><span>{addition.summary} · {addition.status}</span>
                    <small>{addition.files.join(' · ')} — {addition.reason}</small><code>{addition.evidence}</code>
                  </div>)}
                  {mapProposals[0].changes.removals.map((removal) => <div className="map-review-item" key={removal.nodeId}>
                    <strong>Remove {componentTitle(nodes, removal.nodeId)} and its relationships</strong>
                    <small>{removal.deletedFile} — {removal.reason}</small>
                  </div>)}
                  {mapDirty && <p>Save your map edits as a draft before accepting this proposal.</p>}
                  {draftReviewActive && <p>Resolve the pending design draft before accepting this proposal.</p>}
                  <div><button className="button button-outline" type="button" onClick={() => decideMapProposal(false)} disabled={projectBusy}>Reject map update</button>
                    <button className="button button-primary" type="button" onClick={() => decideMapProposal(true)} disabled={projectBusy || mapDirty || draftReviewActive}>Apply map update</button></div>
                </div>}
                {!reviewingMap && mapProposals.length === 0 && reviewFiles.length > 0 && <div className="map-review">
                  <span>Changed files: {reviewFiles.join(' · ')}</span>
                  {reviewError && <strong role="alert">{reviewError}</strong>}
                  <button className="button button-outline" type="button" onClick={reviewMapChanges} disabled={mapDirty}>Review map alignment</button>
                  {mapDirty && <small>Save map edits as a draft first.</small>}
                </div>}
                </div>
                <form className="composer" onSubmit={sendMessage}>
                  {!project
                    ? <button className="composer-project-button" type="button" onClick={chooseProject} disabled={backendStatus.kind !== 'ready' || projectBusy}><FolderPlus size={15} /> Choose project</button>
                    : <button className="composer-attach" type="button" disabled title="Attachments are not available yet"><Plus size={17} /></button>}
                  <textarea
                    ref={composerInput}
                    aria-label="Message the selected project component"
                    value={draft}
                    rows={1}
                    placeholder={!project ? 'Choose a project folder to start chatting' : showChatStarter ? '만들고 싶은 것을 편하게 설명해 주세요' : mapDirty ? 'Save map edits as a draft before chatting…' : draftReviewActive ? 'Ask the Agent to revise the saved draft…' : nodes.length > 0 ? developmentReady ? unansweredQuestions(activeNodeData as ComponentData).length > 0 ? 'Answer the open question in Details to unlock development…' : `Request development for ${activeNodeData?.title ?? 'the selected node'}…` : 'Approve the map before requesting development…' : `Message ${activeNodeData?.title ?? 'the project'}... (⌘ ↵ to send)`}
                    disabled={!project || !agentVersion || agentRunning || recommendationIsLoading || initialDesignPending || reviewingMap || draftEditDirty || mapProposals.length > 0 || !!reviewError || (nodes.length > 0 && (!developmentReady || !selectedNode))}
                    onChange={(event) => setDraft(event.target.value)}
                    onKeyDown={handleComposerKeyDown}
                  />
                  <button className="send-button" type="submit" disabled={!draft.trim() || !project || !agentVersion || agentRunning || recommendationIsLoading || initialDesignPending || reviewingMap || draftEditDirty || mapProposals.length > 0 || !!reviewError || (nodes.length > 0 && (!developmentReady || !selectedNode))} aria-label={agentRunning || initialDesignPending ? 'Generating design map' : draftReviewActive ? 'Request design draft revision' : nodes.length > 0 ? 'Request development for selected map node' : 'Send message'}>
                    {agentRunning || initialDesignPending ? <LoaderCircle className="send-spinner" size={17} /> : <Send size={17} />}
                  </button>
                </form>
              </div>
            )}


            {!isChatStart && activeWorkspaceTab === 'terminal' && (
              <div className="workspace-content terminal-content">
                <div className="terminal-heading"><SquareTerminal size={17} /><strong>Agent activity</strong><span className="terminal-state">{agentRunning ? 'Running' : 'Idle'}</span></div>
                {messages.filter((message) => message.role === 'status').map((message) => <p key={message.id}>{message.text}</p>)}
                {changedFiles.length > 0 && <p>Changed files: {changedFiles.join(' · ')}</p>}
              </div>
            )}
          </section>
        </section>

        {!isChatStart && project && <aside className={`details-panel${draftReviewActive ? ' draft-details-panel' : ''}`} aria-label="Selected component details">
          <div className="details-summary">
            <div className="details-icon">{DraftNodeIcon ? <DraftNodeIcon size={23} /> : activeNodeData ? (() => { const Icon = iconByName[activeNodeData.icon]; return <Icon size={23} /> })() : <Workflow size={23} />}</div>
            <div className="details-title-copy">
              <h2>{activeDraftNode?.type === 'component' ? activeDraftNode.data.title : activeNodeData?.title ?? 'Project Map'}</h2>
              <span className="status-badge"><span />{draftReviewActive ? activeDraftNode ? draftSelectedPane === 'before' ? 'Confirmed map' : 'Unconfirmed draft' : 'Select a node' : activeNodeData?.status ?? 'No selection'}</span>
            </div>
            {!draftReviewActive && <button className="button button-outline" type="button" onClick={editNode} disabled={!project || !activeNodeData || projectBusy}>Edit</button>}
          </div>

          {!draftReviewActive && <div className="detail-tabs" role="tablist" aria-label="Component details">
            {detailTabs.map(({ id, label }) => (
              <button
                className={`detail-tab${activeDetailTab === id ? ' is-active' : ''}`}
                type="button"
                role="tab"
                aria-selected={activeDetailTab === id}
                key={id}
                onClick={() => setActiveDetailTab(id)}
              >
                {label}
              </button>
            ))}
          </div>}

          <div className="detail-scroll-area">
            {draftReviewActive && draftSelectedPane === 'before' && beforeSelectedNode?.type === 'component' && (
              <div className="detail-section draft-node-detail">
                <p className="detail-eyebrow">BEFORE NODE · CONFIRMED MAP</p>
                <h3>Responsibility and scope</h3>
                <p className="detail-summary-text">{beforeSelectedNode.data.summary}</p>
                <h3>Responsibilities</h3>
                <ul className="responsibility-list">{beforeSelectedNode.data.responsibilities.map((item) => <li key={item}>{item}</li>)}</ul>
                <dl className="detail-facts"><div><dt>Status</dt><dd>{beforeSelectedNode.data.status}</dd></div></dl>
                {(beforeSelectedNode.data.questions ?? []).length > 0 && <div className="draft-question-panel"><h3><AlertCircle size={16} /> Open questions</h3>{beforeSelectedNode.data.questions?.map((question) => <p key={question.prompt}>{question.prompt}</p>)}</div>}
                <p className="draft-detail-hint">This is the confirmed node before the proposed changes. Select a node in After to edit its draft.</p>
              </div>
            )}
            {draftReviewActive && draftSelectedPane === 'after' && draftSelectedNode?.type === 'component' && (
              <div className="detail-section draft-node-detail">
                <p className="detail-eyebrow">AFTER NODE</p>
                <label>Node name<input value={draftSelectedNode.data.title} maxLength={120} disabled={projectBusy || agentRunning} onChange={(event) => updateDraftNode(draftSelectedNode.id, (node) => node.type === 'component' ? { ...node, data: { ...node.data, title: event.target.value } } : node)} /></label>
                <label>Status<input list="node-status-suggestions" value={draftSelectedNode.data.status} maxLength={80} disabled={projectBusy || agentRunning} onChange={(event) => updateDraftNode(draftSelectedNode.id, (node) => node.type === 'component' ? { ...node, data: { ...node.data, status: event.target.value } } : node)} /><small>Choose a suggestion or enter your own status.</small></label>
                <label>Responsibility and scope<textarea value={draftSelectedNode.data.summary} rows={6} maxLength={4000} disabled={projectBusy || agentRunning} onChange={(event) => updateDraftNode(draftSelectedNode.id, (node) => node.type === 'component' ? { ...node, data: { ...node.data, summary: event.target.value } } : node)} /></label>
                <label>Responsibilities (one per line)<textarea value={draftSelectedNode.data.responsibilities.join('\n')} rows={4} maxLength={4000} disabled={projectBusy || agentRunning} onChange={(event) => updateDraftNode(draftSelectedNode.id, (node) => node.type === 'component' ? { ...node, data: { ...node.data, responsibilities: event.target.value.split('\n').map((item) => item.trim()).filter(Boolean) } } : node)} /></label>
                <h3>Responsibilities</h3>
                <ul className="responsibility-list">{draftSelectedNode.data.responsibilities.map((item) => <li key={item}>{item}</li>)}</ul>
                <div className="draft-question-panel"><h3><AlertCircle size={16} /> Open questions for this node</h3>{(draftSelectedNode.data.questions ?? []).length > 0 ? draftSelectedNode.data.questions?.map((question) => <p key={question.prompt}>{question.prompt}</p>) : <p className="muted-copy">No unresolved decisions for this node.</p>}</div>
                <p className="draft-detail-hint">Edit the selected node here. The graph stays focused on structure; questions stay in the details panel.</p>
              </div>
            )}
            {!draftReviewActive && activeDetailTab === 'overview' && activeNodeData && (
              <div className="detail-section">
                {editingNodeId === selectedNodeId && (
                  <form className="node-editor" onSubmit={updateNode}>
                    <label>Title<input value={nodeTitle} onChange={(event) => setNodeTitle(event.target.value)} maxLength={120} required disabled={projectBusy} /></label>
                    <label>Status<input list="node-status-suggestions" value={nodeStatus} onChange={(event) => setNodeStatus(event.target.value)} maxLength={80} required disabled={projectBusy} /><small>Choose a suggestion or enter your own status.</small></label>
                    <label>Responsibility and scope<textarea value={nodeSummary} onChange={(event) => setNodeSummary(event.target.value)} rows={4} disabled={projectBusy} /></label>
                    <label>Responsibilities (one per line)<textarea value={nodeResponsibilities} onChange={(event) => setNodeResponsibilities(event.target.value)} rows={4} required disabled={projectBusy} /></label>
                    <div><button className="button button-primary" type="submit" disabled={projectBusy}>Apply changes</button><button className="button button-outline" type="button" onClick={() => setEditingNodeId(null)} disabled={projectBusy}>Cancel</button></div>
                  </form>
                )}
                <h3>Responsibility and scope</h3>
                <p className="detail-summary-text">{activeNodeData.summary}</p>
                <QuestionnairePanel
                  questions={activeNodeData.questions ?? []}
                  answers={activeNodeData.answers ?? {}}
                  disabled={projectBusy}
                  onAnswer={(question, answer) => updateNodeQuestionAnswer(selectedNodeId, question, answer)}
                />
                <h3 className="section-heading-spaced">Key responsibilities</h3>
                <ul className="responsibility-list">
                  {activeNodeData.responsibilities.map((item) => <li key={item}>{item}</li>)}
                </ul>
                <section className="node-unit-plan" aria-label="Development unit for selected node">
                  <div className="node-unit-plan-heading">
                    <div><h3>Unit goal and completion criteria</h3><p>For {activeNodeData.title} · map revision {mapRevision}</p></div>
                    {currentDevelopmentUnitSpec && <span>{currentDevelopmentUnitSpec.source === 'agent' ? 'AI-generated' : 'Edited'}</span>}
                  </div>
                  {developmentUnitSpecLoading === developmentUnitSpecKey && <p className="node-unit-plan-message" role="status">AI is preparing a focused development unit for this node…</p>}
                  {!developmentReady && <p className="node-unit-plan-message">Approve the map before generating or editing a development unit.</p>}
                  {developmentReady && mapDirty && <p className="node-unit-plan-message">Save these map edits as a draft and approve them before changing this node’s plan.</p>}
                  {developmentReady && unansweredQuestions(activeNodeData).length > 0 && <p className="node-unit-plan-message">Answer this node’s open questions and approve the map before generating its plan.</p>}
                  {!agentVersion && <p className="node-unit-plan-message">Connect to the Agent to generate this node’s plan.</p>}
                  {developmentUnitSpecErrors[developmentUnitSpecKey] && <p className="node-unit-plan-error" role="alert">{developmentUnitSpecErrors[developmentUnitSpecKey]}</p>}
                  {currentDevelopmentUnitSpec && <>
                    <label>Unit goal
                      <textarea rows={3} maxLength={4000} value={currentDevelopmentUnitSpec.goal} disabled={projectBusy || agentRunning || developmentUnitSpecSaving} onChange={(event) => {
                        setDevelopmentUnitSpecs((current) => ({ ...current, [developmentUnitSpecKey]: { ...currentDevelopmentUnitSpec, goal: event.target.value } }))
                        setDevelopmentUnitSpecDirty((current) => ({ ...current, [developmentUnitSpecKey]: true }))
                      }} />
                    </label>
                    <label>Completion criteria (one per line)
                      <textarea rows={4} maxLength={20000} value={currentDevelopmentUnitSpec.completionCriteria.join('\n')} disabled={projectBusy || agentRunning || developmentUnitSpecSaving} onChange={(event) => {
                        const completionCriteria = event.target.value.split(/\r?\n/).map((criterion) => criterion.trim()).filter(Boolean)
                        setDevelopmentUnitSpecs((current) => ({ ...current, [developmentUnitSpecKey]: { ...currentDevelopmentUnitSpec, completionCriteria } }))
                        setDevelopmentUnitSpecDirty((current) => ({ ...current, [developmentUnitSpecKey]: true }))
                      }} />
                    </label>
                    <div className="node-unit-plan-actions">
                      <span>{currentUnitSpecDirty ? 'Unsaved edits' : 'Saved'}</span>
                      {currentUnitSpecDirty && <button className="button button-outline" type="button" onClick={() => void saveSelectedDevelopmentUnitSpec()} disabled={developmentUnitSpecSaving || projectBusy || agentRunning || !currentDevelopmentUnitSpec.goal.trim() || currentDevelopmentUnitSpec.completionCriteria.length === 0 || currentDevelopmentUnitSpec.completionCriteria.length > 20 || currentDevelopmentUnitSpec.completionCriteria.some((criterion) => criterion.length > 1000)}>{developmentUnitSpecSaving ? 'Saving…' : 'Save plan'}</button>}
                    </div>
                    {currentDevelopmentUnitSpec.completionCriteria.length > 20 && <p className="node-unit-plan-error">A plan can have at most 20 completion criteria.</p>}
                  </>}
                  {!currentDevelopmentUnitSpec && !developmentUnitSpecLoading && developmentReady && !mapDirty && agentVersion && unansweredQuestions(activeNodeData).length === 0 && <button className="button button-outline" type="button" onClick={retryDevelopmentUnitSpec} disabled={projectBusy}>Generate plan again</button>}
                  <p className="node-unit-plan-footer">Develop selected node saves any edits and starts this exact unit.</p>
                </section>
                <h3 className="section-heading-spaced">Details</h3>
                <dl className="detail-facts">
                  <div><dt>Type</dt><dd><span className="type-pill">{activeNodeData.kind}</span></dd></div>
                  <div><dt>Technology</dt><dd>{activeNodeData.technology}</dd></div>
                  <div><dt>Status</dt><dd><span className="inline-status"><span />{activeNodeData.status}</span></dd></div>
                  <div><dt>Files</dt><dd>{activeNodeData.files.length ? `${activeNodeData.files.length} linked` : 'None linked'}</dd></div>
                </dl>
                <div className="relationships-heading"><h3>Relationships</h3><button type="button" className="text-action" onClick={() => setActiveDetailTab('relationships')}>View all</button></div>
                <RelationshipList relationships={relatedRelationships} onSelect={(id) => { setSelectedNodeId(id); setActiveDetailTab('overview') }} />
              </div>
            )}

            {!draftReviewActive && activeDetailTab === 'files' && (
              <div className="detail-section">
                <h3>Linked files</h3>
                {activeNodeData?.files.length ? activeNodeData.files.map((file) => (
                  <div className="file-row" key={file}><Code2 size={16} /><span>{file}</span><ChevronRight size={14} /></div>
                )) : <div className="detail-empty"><FileCode2 size={20} /><strong>No files linked yet</strong><p>Files can be connected to a map component as development progresses.</p></div>}
              </div>
            )}

            {!draftReviewActive && activeDetailTab === 'tasks' && (
              <div className="detail-section">
                <h3>Development units are separate from node responsibilities</h3>
                {developmentUnits.length === 0
                  ? <div className="detail-empty"><ListChecks size={20} /><strong>No development units yet</strong><p>Select a map node to see its AI-generated unit plan, then develop that node.</p></div>
                  : developmentUnits.map((unit) => {
                    const unitNode = nodes.find((node) => node.id === unit.nodeId && node.type === 'component')
                    const statusLabel = unit.reviewDecision
                      ? developmentReviewDecisionLabels[unit.reviewDecision]
                      : unit.status === 'started' ? 'Started' : unit.status === 'start_failed' ? 'Could not start' : 'Confirmed'
                    const runStateLabel = unit.latestRunState === 'finished' ? unit.reviewDecision ? 'Run finished' : 'Run finished · awaiting review' : unit.latestRunState === 'interrupted' ? 'Run interrupted' : unit.latestRunState === 'failed' ? 'Run failed' : unit.latestRunState === 'disconnected' ? 'Agent disconnected' : unit.latestRunState === 'start_failed' ? 'Run could not start' : unit.latestRunState === 'running' ? 'Agent running' : unit.latestRunState === 'starting' ? 'Agent starting' : null
                    const runStartedAt = unit.latestRunStartedAt ?? unit.startedAt ?? unit.createdAt
                    const runStartedAtLabel = formatPersistedTimestamp(runStartedAt) ?? 'time unavailable'
                    const runFinishedAtLabel = formatPersistedTimestamp(unit.latestRunFinishedAt)
                    const runDuration = formatRunDuration(runStartedAt, unit.latestRunFinishedAt)
                    return <article className="detail-task development-unit-card" key={unit.id}>
                      <ListChecks size={17} />
                      <div>
                        <strong>{unit.goal}</strong>
                        <small>{unitNode?.type === 'component' ? unitNode.data.title : unit.nodeId} · map revision {unit.mapRevision} · {statusLabel}{unit.latestRunId ? ` · run #${unit.latestRunId}` : ''}{runStateLabel ? ` · ${runStateLabel}` : ''}</small>
                        <small className="development-run-timestamp">
                          {unit.latestRunId
                            ? <>Started {runStartedAtLabel}{unit.latestRunFinishedAt && <> · Finished {runFinishedAtLabel}{runDuration ? ` · ${runDuration}` : ''}</>}</>
                            : <>Created {formatPersistedTimestamp(unit.createdAt) ?? 'time unavailable'}</>}
                        </small>
                        <ul>{unit.completionCriteria.map((criterion) => <li key={criterion}>{criterion}</li>)}</ul>
                        {unit.statusDetail && <small role="alert">{unit.statusDetail}</small>}
                        {unit.latestRunDetail && <small role="alert">{unit.latestRunDetail}</small>}
                        {unit.latestRunId && <DevelopmentRunReviewPanel
                          unit={unit}
                          review={developmentRunReview?.runId === unit.latestRunId ? developmentRunReview : null}
                          loading={developmentRunReviewLoading === unit.latestRunId}
                          error={developmentRunReviewError?.runId === unit.latestRunId ? developmentRunReviewError.message : null}
                          decisionNote={developmentReviewNotes[unit.latestRunId] ?? unit.reviewNote ?? ''}
                          decisionSaving={developmentReviewSavingRunId === unit.latestRunId}
                          decisionError={developmentReviewErrors[unit.latestRunId] ?? null}
                          onDecisionNoteChange={(note) => setDevelopmentReviewNotes((current) => ({ ...current, [unit.latestRunId!]: note }))}
                          onSaveDecision={(decision, note) => { void saveDevelopmentReview(unit, decision, note) }}
                          onToggle={() => { void toggleDevelopmentRunReview(unit.latestRunId!) }}
                        />}
                      </div>
                    </article>
                  })}
              </div>
            )}

            {!draftReviewActive && activeDetailTab === 'relationships' && (
              <div className="detail-section">
                <h3>Connected components</h3>
                <RelationshipList relationships={relatedRelationships} onSelect={(id) => { setSelectedNodeId(id); setActiveDetailTab('overview') }} />
                {relatedRelationships.length === 0 && <div className="detail-empty"><GitBranch size={20} /><strong>No relationships</strong><p>Choose a relationship type before connecting nodes on the map.</p></div>}
              </div>
            )}
          </div>


        </aside>}
        </>}
      </div>

      {project && !isChatStart && projectSidebarCollapsed && <div className="sidebar-reopen-hotspot">
        <button className="sidebar-reopen-button" type="button" onClick={() => setProjectSidebarCollapsed(false)} aria-label="Expand project sidebar" title="Expand project sidebar">
          <ChevronRight size={18} />
        </button>
      </div>}

      {notice && <div className="toast-notice" role="status"><Check size={16} />{notice}<button type="button" onClick={() => setNotice('')} aria-label="Dismiss notification"><X size={15} /></button></div>}
      <datalist id="node-status-suggestions">
        <option value="Planned" />
        <option value="Not started" />
        <option value="In progress" />
        <option value="Blocked" />
        <option value="Completed" />
        <option value="Not connected" />
        <option value="Prototype" />
      </datalist>
    </main>
  )
}

function RelationshipList({ relationships, onSelect }: { relationships: { node: ComponentFlowNode; edge: HeimEdge; selectedIsSource: boolean }[]; onSelect: (id: string) => void }) {
  if (relationships.length === 0) {
    return <p className="muted-copy">No connected components yet.</p>
  }

  return (
    <div className="relationship-list">
      {relationships.map(({ node, edge, selectedIsSource }) => {
        const Icon = iconByName[node.data.icon]
        const relation = edge.data?.relation === 'depends_on'
          ? selectedIsSource ? 'Depends on →' : '← Required by'
          : edge.data?.relation === 'interacts_with'
            ? selectedIsSource ? 'Interacts with →' : '← Interacts with'
            : 'Unclassified'
        return (
          <button type="button" className="relationship-row" key={edge.id} onClick={() => onSelect(node.id)}>
            <span className="relationship-icon"><Icon size={15} /></span>
            <span className="relationship-name">{node.data.title}</span>
            <span className="relationship-type">{relation}</span>
          </button>
        )
      })}
    </div>
  )
}

export default App
