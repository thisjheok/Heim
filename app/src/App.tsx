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
import '@xyflow/react/dist/style.css'
import './App.css'

type ComponentFlowNode = Node<ComponentData, 'component'>
type LayerFlowNode = Node<LayerData, 'layer'>
type WorkspaceTab = 'chat' | 'terminal'
type DetailTab = 'overview' | 'files' | 'tasks' | 'relationships'
type ChatMessage = { id: string; text: string; time: string; role: 'user' | 'agent' | 'status' }
type AgentEvent = { projectId: number; kind: 'delta' | 'message' | 'status' | 'files' | 'changed' | 'done' | 'disconnected' | 'approval'; text: string; itemId?: string; requestId?: number }
type Approval = { id: number; text: string }
type MapProposal = { id: number; projectId: number; baseRevision: number; changedFiles: string[]; changes: {
  updates: { nodeId: string; summary: string; status: string; files: string[]; removeFiles: string[]; reason: string; evidence: string }[]
  additions: { title: string; summary: string; status: string; files: string[]; reason: string; evidence: string }[]
  removals: { nodeId: string; deletedFile: string; reason: string }[]
} }
type RuntimeInfo = { version: string; platform: string }
type Project = { id: number; name: string; root: string }
type StoredMap = { nodes: HeimNode[]; edges: HeimEdge[]; revision: number }
type DesignDraft = { id: number; projectId: number; baseRevision: number; version: number; draft: { goal: string; nodes: HeimNode[]; edges: HeimEdge[]; questions: string[]; assumptions: string[] } }
type MapEditDraftResult = { draft: DesignDraft; confirmedMap: StoredMap }
type DevelopmentUnit = { id: number; projectId: number; mapRevision: number; nodeId: string; goal: string; completionCriteria: string[]; status: 'confirmed' | 'started' | 'start_failed'; statusDetail: string | null; createdAt: string; startedAt: string | null }
type BackendStatus =
  | { kind: 'checking' | 'browser' }
  | { kind: 'ready'; info: RuntimeInfo }
  | { kind: 'error'; message: string }

const componentTitle = (nodes: HeimNode[], id: string) => {
  const node = nodes.find((item) => item.id === id)
  return node?.type === 'component' ? node.data.title : id
}

const unansweredQuestions = (data: ComponentData) =>
  (data.questions ?? []).filter((question) => !(data.answers?.[question] ?? '').trim())

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
    <div className={`component-node${selected ? ' is-selected' : ''}`}>
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
  const [developmentUnitOpen, setDevelopmentUnitOpen] = useState(false)
  const [developmentUnitNodeId, setDevelopmentUnitNodeId] = useState('')
  const [developmentUnitGoal, setDevelopmentUnitGoal] = useState('')
  const [developmentUnitCriteria, setDevelopmentUnitCriteria] = useState('')
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
  const [mapDirty, setMapDirty] = useState(false)
  const [editingNodeId, setEditingNodeId] = useState<string | null>(null)
  const [nodeTitle, setNodeTitle] = useState('')
  const [nodeSummary, setNodeSummary] = useState('')
  const [nodeResponsibilities, setNodeResponsibilities] = useState('')
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
          setAgentRunning(false)
          setApprovals([])
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

  const selectedNode = nodes.find(
    (node): node is ComponentFlowNode =>
      node.id === selectedNodeId && node.type === 'component',
  )
  const developmentNode = nodes.find((node): node is ComponentFlowNode => node.type === 'component' && node.id === developmentUnitNodeId)
  const completionCriteria = developmentUnitCriteria.split(/\r?\n/).map((criterion) => criterion.trim()).filter(Boolean)

  const relatedRelationships = useMemo(() => {
    return edges.flatMap((edge) => {
      if (edge.source !== selectedNodeId && edge.target !== selectedNodeId) return []
      const relatedId = edge.source === selectedNodeId ? edge.target : edge.source
      const node = nodes.find((item): item is ComponentFlowNode => item.id === relatedId && item.type === 'component')
      return node ? [{ edge, node, selectedIsSource: edge.source === selectedNodeId }] : []
    })
  }, [edges, nodes, selectedNodeId])

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
    setActiveDetailTab('overview')
  }

  const updateNode = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (projectBusy) return
    const responsibilities = nodeResponsibilities.split('\n').map((item) => item.trim()).filter(Boolean)
    if (!editingNodeId || !nodeTitle.trim() || !nodeSummary.trim() || responsibilities.length === 0 || !project) return
    setNodes((current) => current.map((node) => node.id === editingNodeId && node.type === 'component'
      ? { ...node, data: { ...node.data, title: nodeTitle.trim(), summary: nodeSummary.trim(), responsibilities } }
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

  const openDevelopmentUnit = () => {
    if (!project || !agentVersion || agentRunning || draftReviewActive || mapDirty || !developmentReady) return
    setDevelopmentUnitNodeId('')
    setDevelopmentUnitGoal('')
    setDevelopmentUnitCriteria('')
    setDevelopmentUnitOpen(true)
  }

  const startDevelopment = async () => {
    if (!project || !agentVersion || agentRunning || draftReviewActive || mapDirty || !developmentReady || !developmentNode || !developmentUnitGoal.trim() || completionCriteria.length === 0 || unansweredQuestions(developmentNode.data).length > 0) return
    setAgentRunning(true)
    try {
      const unitId = await invoke<number>('send_development_message', {
        projectId: project.id,
        mapRevision,
        nodeId: developmentNode.id,
        goal: developmentUnitGoal.trim(),
        completionCriteria,
      })
      setDevelopmentUnits((current) => [{
        id: unitId,
        projectId: project.id,
        mapRevision,
        nodeId: developmentNode.id,
        goal: developmentUnitGoal.trim(),
        completionCriteria,
        status: 'started',
        statusDetail: null,
        createdAt: new Date().toISOString(),
        startedAt: new Date().toISOString(),
      }, ...current])
      setMessages((current) => [...current, { id: crypto.randomUUID(), role: 'status', text: `Started development unit #${unitId} for ${developmentNode.data.title} against map revision ${mapRevision}.`, time: new Date().toLocaleTimeString() }])
      setDevelopmentUnitOpen(false)
    } catch (error) {
      setAgentRunning(false)
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
        await invoke('send_design_message', { projectId: project.id, text, mapContext: JSON.stringify({ nodes, edges }) })
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
        setDevelopmentReady(false)
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
              {project && <button
                className="button button-development"
                type="button"
                onClick={openDevelopmentUnit}
                disabled={!agentVersion || agentRunning || draftReviewActive || mapDirty || projectBusy || !developmentReady}
                title={!developmentReady ? 'Approve a map before defining a development unit' : 'Choose a node, define a goal and completion criteria, then confirm the unit'}
                aria-label="Define development unit"
              >
                <Code2 size={15} /> <span className="develop-label">Define unit</span>
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
              nodes={nodes}
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
                </div>
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
                {nodes.length > 0 && <div className="map-review" role="status">
                  {developmentReady
                    ? `Approved map revision ${mapRevision}. Define and confirm a target, goal, and completion criteria before starting development.`
                    : 'Development-unit definition is locked until this map revision has an approved design draft.'}
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
                    disabled={!project || !agentVersion || agentRunning || initialDesignPending || reviewingMap || draftEditDirty || mapProposals.length > 0 || !!reviewError || (nodes.length > 0 && (!developmentReady || !selectedNode))}
                    onChange={(event) => setDraft(event.target.value)}
                    onKeyDown={handleComposerKeyDown}
                  />
                  <button className="send-button" type="submit" disabled={!draft.trim() || !project || !agentVersion || agentRunning || initialDesignPending || reviewingMap || draftEditDirty || mapProposals.length > 0 || !!reviewError || (nodes.length > 0 && (!developmentReady || !selectedNode))} aria-label={agentRunning || initialDesignPending ? 'Generating design map' : draftReviewActive ? 'Request design draft revision' : nodes.length > 0 ? 'Request development for selected map node' : 'Send message'}>
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
                {(beforeSelectedNode.data.questions ?? []).length > 0 && <div className="draft-question-panel"><h3><AlertCircle size={16} /> Open questions</h3>{beforeSelectedNode.data.questions?.map((question) => <p key={question}>{question}</p>)}</div>}
                <p className="draft-detail-hint">This is the confirmed node before the proposed changes. Select a node in After to edit its draft.</p>
              </div>
            )}
            {draftReviewActive && draftSelectedPane === 'after' && draftSelectedNode?.type === 'component' && (
              <div className="detail-section draft-node-detail">
                <p className="detail-eyebrow">AFTER NODE</p>
                <label>Node name<input value={draftSelectedNode.data.title} maxLength={120} disabled={projectBusy || agentRunning} onChange={(event) => updateDraftNode(draftSelectedNode.id, (node) => node.type === 'component' ? { ...node, data: { ...node.data, title: event.target.value } } : node)} /></label>
                <label>Responsibility and scope<textarea value={draftSelectedNode.data.summary} rows={6} maxLength={4000} disabled={projectBusy || agentRunning} onChange={(event) => updateDraftNode(draftSelectedNode.id, (node) => node.type === 'component' ? { ...node, data: { ...node.data, summary: event.target.value } } : node)} /></label>
                <label>Responsibilities (one per line)<textarea value={draftSelectedNode.data.responsibilities.join('\n')} rows={4} maxLength={4000} disabled={projectBusy || agentRunning} onChange={(event) => updateDraftNode(draftSelectedNode.id, (node) => node.type === 'component' ? { ...node, data: { ...node.data, responsibilities: event.target.value.split('\n').map((item) => item.trim()).filter(Boolean) } } : node)} /></label>
                <h3>Responsibilities</h3>
                <ul className="responsibility-list">{draftSelectedNode.data.responsibilities.map((item) => <li key={item}>{item}</li>)}</ul>
                <div className="draft-question-panel"><h3><AlertCircle size={16} /> Open questions for this node</h3>{(draftSelectedNode.data.questions ?? []).length > 0 ? draftSelectedNode.data.questions?.map((question) => <p key={question}>{question}</p>) : <p className="muted-copy">No unresolved decisions for this node.</p>}</div>
                <p className="draft-detail-hint">Edit the selected node here. The graph stays focused on structure; questions stay in the details panel.</p>
              </div>
            )}
            {!draftReviewActive && activeDetailTab === 'overview' && activeNodeData && (
              <div className="detail-section">
                {editingNodeId === selectedNodeId && (
                  <form className="node-editor" onSubmit={updateNode}>
                    <label>Title<input value={nodeTitle} onChange={(event) => setNodeTitle(event.target.value)} maxLength={120} required disabled={projectBusy} /></label>
                    <label>Responsibility and scope<textarea value={nodeSummary} onChange={(event) => setNodeSummary(event.target.value)} rows={4} disabled={projectBusy} /></label>
                    <label>Responsibilities (one per line)<textarea value={nodeResponsibilities} onChange={(event) => setNodeResponsibilities(event.target.value)} rows={4} required disabled={projectBusy} /></label>
                    <div><button className="button button-primary" type="submit" disabled={projectBusy}>Apply changes</button><button className="button button-outline" type="button" onClick={() => setEditingNodeId(null)} disabled={projectBusy}>Cancel</button></div>
                  </form>
                )}
                <h3>Responsibility and scope</h3>
                <p className="detail-summary-text">{activeNodeData.summary}</p>
                {(activeNodeData.questions ?? []).length > 0 && <div className="draft-question-panel detail-question-panel"><h3><AlertCircle size={16} /> Open questions</h3>{activeNodeData.questions?.map((question) => <label className="question-answer" key={question}><span>{question}</span><textarea rows={3} placeholder="Answer this question to unlock development…" value={activeNodeData.answers?.[question] ?? ''} disabled={projectBusy} onChange={(event) => updateNodeQuestionAnswer(selectedNodeId, question, event.target.value)} /> </label>)}<small>{unansweredQuestions(activeNodeData).length > 0 ? 'Answer all questions, then save the map before development.' : 'All questions answered. Save the map to confirm these decisions.'}</small></div>}
                <h3 className="section-heading-spaced">Key responsibilities</h3>
                <ul className="responsibility-list">
                  {activeNodeData.responsibilities.map((item) => <li key={item}>{item}</li>)}
                </ul>
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
                  ? <div className="detail-empty"><ListChecks size={20} /><strong>No confirmed units yet</strong><p>Define a target, goal, and observable completion criteria to start a unit.</p></div>
                  : developmentUnits.map((unit) => {
                    const unitNode = nodes.find((node) => node.id === unit.nodeId && node.type === 'component')
                    const statusLabel = unit.status === 'started' ? 'Started' : unit.status === 'start_failed' ? 'Could not start' : 'Confirmed'
                    return <article className="detail-task development-unit-card" key={unit.id}>
                      <ListChecks size={17} />
                      <div>
                        <strong>{unit.goal}</strong>
                        <small>{unitNode?.type === 'component' ? unitNode.data.title : unit.nodeId} · map revision {unit.mapRevision} · {statusLabel}</small>
                        <ul>{unit.completionCriteria.map((criterion) => <li key={criterion}>{criterion}</li>)}</ul>
                        {unit.statusDetail && <small role="alert">{unit.statusDetail}</small>}
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

      {developmentUnitOpen && <div className="development-unit-backdrop">
        <section className="development-unit-dialog" role="dialog" aria-modal="true" aria-labelledby="development-unit-title">
          <header className="development-unit-heading">
            <div><p className="detail-eyebrow">CONFIRMED MAP · REVISION {mapRevision}</p><h2 id="development-unit-title">Define one development unit</h2></div>
            <button className="button button-outline" type="button" onClick={() => setDevelopmentUnitOpen(false)} disabled={agentRunning} aria-label="Close development-unit dialog"><X size={17} /></button>
          </header>
          <form className="development-unit-form" onSubmit={(event) => { event.preventDefault(); void startDevelopment() }}>
            <label>Architecture responsibility
              <select value={developmentUnitNodeId} onChange={(event) => setDevelopmentUnitNodeId(event.target.value)} disabled={agentRunning}>
                <option value="">Choose a node</option>
                {nodes.filter((node): node is ComponentFlowNode => node.type === 'component').map((node) => <option key={node.id} value={node.id}>{node.data.title}</option>)}
              </select>
            </label>
            {developmentNode && <div className="development-unit-node-summary">
              <strong>{developmentNode.data.title}</strong><p>{developmentNode.data.summary}</p>
              <ul>{developmentNode.data.responsibilities.map((responsibility) => <li key={responsibility}>{responsibility}</li>)}</ul>
              {unansweredQuestions(developmentNode.data).length > 0 && <p className="development-unit-warning" role="alert">Answer this node’s open questions before starting its unit.</p>}
            </div>}
            <label>Unit goal
              <textarea value={developmentUnitGoal} onChange={(event) => setDevelopmentUnitGoal(event.target.value)} rows={3} maxLength={4000} placeholder="Describe the specific outcome for this unit." disabled={agentRunning} />
            </label>
            <label>Completion criteria (one observable condition per line)
              <textarea value={developmentUnitCriteria} onChange={(event) => setDevelopmentUnitCriteria(event.target.value)} rows={5} maxLength={20000} placeholder={'A user can…\nThe result remains correct after…'} disabled={agentRunning} />
            </label>
            <p className="development-unit-hint">Only this confirmed goal and its criteria will define the work. The unit is pinned to map revision {mapRevision}.</p>
            <footer>
              <button className="button button-outline" type="button" onClick={() => setDevelopmentUnitOpen(false)} disabled={agentRunning}>Cancel</button>
              <button className="button button-primary" type="submit" disabled={!agentVersion || agentRunning || !developmentNode || !developmentUnitGoal.trim() || completionCriteria.length === 0 || completionCriteria.length > 20 || completionCriteria.some((criterion) => criterion.length > 1000) || unansweredQuestions(developmentNode.data).length > 0}>Confirm unit and start</button>
            </footer>
          </form>
        </section>
      </div>}

      {project && !isChatStart && projectSidebarCollapsed && <div className="sidebar-reopen-hotspot">
        <button className="sidebar-reopen-button" type="button" onClick={() => setProjectSidebarCollapsed(false)} aria-label="Expand project sidebar" title="Expand project sidebar">
          <ChevronRight size={18} />
        </button>
      </div>}

      {notice && <div className="toast-notice" role="status"><Check size={16} />{notice}<button type="button" onClick={() => setNotice('')} aria-label="Dismiss notification"><X size={15} /></button></div>}
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
