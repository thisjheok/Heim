import { useEffect, useMemo, useRef, useState, type FormEvent, type KeyboardEvent } from 'react'
import { invoke, isTauri } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import {
  addEdge,
  Background,
  BackgroundVariant,
  Controls,
  Handle,
  MiniMap,
  Position,
  ReactFlow,
  useEdgesState,
  useNodesState,
  type Connection,
  type Edge,
  type Node,
  type NodeProps,
  type ReactFlowInstance,
} from '@xyflow/react'
import {
  Activity,
  Bot,
  Boxes,
  Check,
  ChevronDown,
  ChevronRight,
  CircleHelp,
  ClipboardList,
  Code2,
  Database,
  FileCode2,
  FileText,
  Folder,
  FolderOpen,
  GitBranch,
  LayoutGrid,
  Link2,
  ListChecks,

  MessageCircle,
  Monitor,
  MoreHorizontal,
  PanelRightClose,
  Plus,
  Search,
  Send,
  Settings2,
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
  type ComponentData,
  type ComponentIcon,
  type HeimNode,
  type LayerData,
} from './data/projectMap'
import '@xyflow/react/dist/style.css'
import './App.css'

type ComponentFlowNode = Node<ComponentData, 'component'>
type LayerFlowNode = Node<LayerData, 'layer'>
type WorkspaceTab = 'chat' | 'tasks' | 'context' | 'terminal'
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
type StoredMap = { nodes: HeimNode[]; edges: Edge[]; revision: number }
type BackendStatus =
  | { kind: 'checking' | 'browser' }
  | { kind: 'ready'; info: RuntimeInfo }
  | { kind: 'error'; message: string }

const componentTitle = (nodes: HeimNode[], id: string) => {
  const node = nodes.find((item) => item.id === id)
  return node?.type === 'component' ? node.data.title : id
}

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

function ComponentNodeView({ data, selected }: NodeProps<ComponentFlowNode>) {
  const Icon = iconByName[data.icon]

  return (
    <div className={`component-node${selected ? ' is-selected' : ''}`}>
      <Handle type="target" position={Position.Left} />
      <div className="component-node-icon">
        <Icon size={18} strokeWidth={1.8} />
      </div>
      <div className="component-node-copy">
        <strong>{data.title}</strong>
        <span>{data.kind}</span>
        <small>{data.technology}</small>
      </div>
      <span className={`node-status-dot ${data.status === 'In progress' ? 'is-active' : ''}`} />
      <Handle type="source" position={Position.Right} />
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

const navItems = [
  { label: 'Overview', caption: 'Project overview', icon: Activity },
  { label: 'Project Map', caption: 'Visualize architecture', icon: Workflow },
  { label: 'Components', caption: 'Manage components', icon: Boxes },
  { label: 'Docs', caption: 'Architecture & notes', icon: FileText },
  { label: 'Agents', caption: 'AI collaborators', icon: Bot },
]

const workspaceTabs: { id: WorkspaceTab; label: string; icon: LucideIcon }[] = [
  { id: 'chat', label: 'Chat', icon: MessageCircle },
  { id: 'tasks', label: 'Tasks', icon: ListChecks },
  { id: 'context', label: 'Context', icon: FileCode2 },
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
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>(initialEdges)
  const [selectedNodeId, setSelectedNodeId] = useState('project-map')
  const [activeWorkspaceTab, setActiveWorkspaceTab] = useState<WorkspaceTab>('chat')
  const [activeDetailTab, setActiveDetailTab] = useState<DetailTab>('overview')
  const [searchText, setSearchText] = useState('')
  const [draft, setDraft] = useState('')
  const [messages, setMessages] = useState<ChatMessage[]>([])
  const [agentVersion, setAgentVersion] = useState('')
  const [agentError, setAgentError] = useState('')
  const [agentRunning, setAgentRunning] = useState(false)
  const [approvals, setApprovals] = useState<Approval[]>([])
  const [changedFiles, setChangedFiles] = useState<string[]>([])
  const [mapProposals, setMapProposals] = useState<MapProposal[]>([])
  const [reviewFiles, setReviewFiles] = useState<string[]>([])
  const [reviewingMap, setReviewingMap] = useState(false)
  const [reviewError, setReviewError] = useState('')
  const [notice, setNotice] = useState('')
  const [backendStatus, setBackendStatus] = useState<BackendStatus>({ kind: 'checking' })
  const [project, setProject] = useState<Project | null>(null)
  const [recentProjects, setRecentProjects] = useState<Project[]>([])
  const [projectMenuOpen, setProjectMenuOpen] = useState(false)
  const [mapRevision, setMapRevision] = useState(0)
  const [mapDirty, setMapDirty] = useState(false)
  const [editingNodeId, setEditingNodeId] = useState<string | null>(null)
  const [nodeTitle, setNodeTitle] = useState('')
  const [nodeSummary, setNodeSummary] = useState('')
  const [projectBusy, setProjectBusy] = useState(false)
  const [projectTreeOpen, setProjectTreeOpen] = useState(true)
  const [appTreeOpen, setAppTreeOpen] = useState(true)
  const [flow, setFlow] = useState<ReactFlowInstance<HeimNode, Edge> | null>(null)
  const noticeTimer = useRef<number | undefined>(undefined)
  const searchInput = useRef<HTMLInputElement>(null)
  const composerInput = useRef<HTMLTextAreaElement>(null)
  const mapChangeCount = useRef(0)
  const mapDirtyRef = useRef(false)
  const projectId = project?.id

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
    invoke<Project[]>('recent_projects')
      .then(setRecentProjects)
      .catch((error: unknown) => showNotice(`Could not load recent projects: ${String(error)}`))
  }, [backendStatus.kind])

  useEffect(() => {
    if (backendStatus.kind !== 'ready') return
    invoke<string>('agent_status').then(setAgentVersion).catch((error: unknown) => setAgentError(String(error)))
  }, [backendStatus.kind])

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
    const focusSearch = (event: globalThis.KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault()
        searchInput.current?.focus()
      }
    }

    window.addEventListener('keydown', focusSearch)
    return () => window.removeEventListener('keydown', focusSearch)
  }, [])

  const selectedNode = nodes.find(
    (node): node is ComponentFlowNode =>
      node.id === selectedNodeId && node.type === 'component',
  )

  const relatedNodes = useMemo(() => {
    const relatedIds = new Set(
      edges
        .filter((edge) => edge.source === selectedNodeId || edge.target === selectedNodeId)
        .flatMap((edge) => [edge.source, edge.target])
        .filter((id) => id !== selectedNodeId),
    )

    return nodes.filter(
      (node): node is ComponentFlowNode =>
        relatedIds.has(node.id) && node.type === 'component',
    )
  }, [edges, nodes, selectedNodeId])

  const showNotice = (message: string) => {
    setNotice(message)
    window.clearTimeout(noticeTimer.current)
    noticeTimer.current = window.setTimeout(() => setNotice(''), 4200)
  }

  const markMapDirty = () => {
    mapChangeCount.current += 1
    mapDirtyRef.current = true
    setMapDirty(true)
  }

  const loadProject = async (next: Project) => {
    const map = await invoke<StoredMap>('load_project_map', { projectId: next.id })
    setNodes(map.nodes)
    setEdges(map.edges)
    setProject(next)
    setMapRevision(map.revision)
    setMapDirty(false)
    mapDirtyRef.current = false
    mapChangeCount.current = 0
    setSelectedNodeId(map.nodes.find((node) => node.type === 'component')?.id ?? '')
    setMessages([])
    setChangedFiles([])
    setApprovals([])
    setReviewFiles([])
    setReviewError('')
    setMapProposals(await invoke<MapProposal[]>('pending_map_proposals', { projectId: next.id }))
    setEditingNodeId(null)
    setActiveDetailTab('overview')
    setProjectMenuOpen(false)
    void flow?.fitView({ duration: 350 })
    setRecentProjects(await invoke<Project[]>('recent_projects'))
  }

  const confirmSwitch = () => {
    if (agentRunning) { showNotice('Cancel the running agent turn before switching projects.'); return false }
    return !mapDirty || window.confirm('Unsaved map changes will be lost. Continue?')
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

  const reopenProject = async (id: number) => {
    if (!confirmSwitch()) return
    setProjectBusy(true)
    try {
      const next = await invoke<Project>('reopen_project', { projectId: id })
      await loadProject(next)
    } catch (error) {
      showNotice(`Could not reopen project: ${String(error)}`)
    } finally {
      setProjectBusy(false)
    }
  }

  const closeProject = () => {
    if (!confirmSwitch()) return
    setProject(null)
    setNodes(initialNodes)
    setEdges(initialEdges)
    setSelectedNodeId('project-map')
    setMapDirty(false)
    mapDirtyRef.current = false
    setMessages([])
    setChangedFiles([])
    setApprovals([])
    setReviewFiles([])
    setReviewError('')
    setMapProposals([])
    setEditingNodeId(null)
    setProjectMenuOpen(false)
  }

  const saveMap = async () => {
    if (!project || !mapDirty) return
    setProjectBusy(true)
    const currentChangeCount = mapChangeCount.current
    try {
      const revision = await invoke<number>('save_project_map', {
        projectId: project.id,
        map: {
          nodes: nodes.map(({ id, type, position, data, style, draggable, selectable, connectable, zIndex }) =>
            ({ id, type, position, data, style, draggable, selectable, connectable, zIndex })),
          edges: edges.map(({ id, source, target, sourceHandle, targetHandle, style, markerEnd }) =>
            ({ id, source, target, sourceHandle, targetHandle, style, markerEnd })),
          revision: mapRevision,
        },
      })
      setMapRevision(revision)
      if (mapChangeCount.current === currentChangeCount) { setMapDirty(false); mapDirtyRef.current = false }
      showNotice('Project map saved.')
    } catch (error) {
      showNotice(`Could not save map: ${String(error)}`)
    } finally {
      setProjectBusy(false)
    }
  }

  const handleSearch = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key !== 'Enter' || !searchText.trim()) return

    const query = searchText.trim().toLowerCase()
    const match = nodes.find(
      (node) =>
        node.type === 'component' &&
        [node.data.title, node.data.kind, node.data.summary, ...node.data.files]
          .join(' ')
          .toLowerCase()
          .includes(query),
    )

    if (!match) {
      showNotice('No matching component or file was found in this map.')
      return
    }

    setSelectedNodeId(match.id)
    flow?.fitView({ nodes: [{ id: match.id }], duration: 350, padding: 0.55 })
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
        kind: 'Proposed component',
        summary: 'Describe what this part of the project is responsible for.',
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
    setActiveDetailTab('overview')
  }

  const updateNode = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!editingNodeId || !nodeTitle.trim() || !project) return
    setNodes((current) => current.map((node) => node.id === editingNodeId && node.type === 'component'
      ? { ...node, data: { ...node.data, title: nodeTitle.trim(), summary: nodeSummary.trim() } }
      : node))
    markMapDirty()
    setEditingNodeId(null)
  }

  const handleConnect = (connection: Connection) => {
    if (!project || connection.source === connection.target) return
    setEdges((current) => addEdge(connection, current))
    markMapDirty()
  }

  const sendMessage = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const text = draft.trim()
    if (!text || !project || !agentVersion || agentRunning || reviewingMap || mapDirty || mapProposals.length > 0 || reviewError) return

    const context = selectedNode ? JSON.stringify({ selectedNode: {
      id: selectedNode.id, title: selectedNode.data.title, summary: selectedNode.data.summary,
      responsibilities: selectedNode.data.responsibilities,
    }, relatedNodes: relatedNodes.map((node) => ({ id: node.id, title: node.data.title })) }) : ''
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
    setDraft('')
    setAgentRunning(true)
    try {
      await invoke('send_agent_message', { projectId: project.id, text, context })
    } catch (error) {
      setAgentRunning(false)
      setMessages((current) => [...current, { id: crypto.randomUUID(), role: 'status', text: `Could not send message: ${String(error)}`, time: new Date().toLocaleTimeString() }])
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
    if (!project || !proposal || (accept && mapDirty)) return
    setProjectBusy(true)
    try {
      const map = await invoke<StoredMap | null>('resolve_map_proposal', { projectId: project.id, proposalId: proposal.id, accept })
      if (map) {
        setNodes(map.nodes)
        setEdges(map.edges)
        setMapRevision(map.revision)
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

  return (
    <main className="app-shell">
      <header className="topbar">
        <div className="brand-area">
          <div className="brand-mark"><Workflow size={19} strokeWidth={2.4} /></div>
          <span className="brand-name">Heim</span>
        </div>

        <div className="topbar-project">
          <button className="project-switcher" type="button" aria-label="Current project" aria-expanded={projectMenuOpen} onClick={() => setProjectMenuOpen((open) => !open)}>
            <span className="project-name">{project?.name ?? 'Open a project'}</span>
            <ChevronDown size={15} />
            <span className="project-stage">{project ? (mapDirty ? 'Unsaved map changes' : 'Project map') : 'Desktop workspace'}</span>
          </button>
          {projectMenuOpen && (
            <div className="project-menu">
              <button type="button" onClick={chooseProject} disabled={backendStatus.kind !== 'ready' || projectBusy}>Open folder…</button>
              {project && <button type="button" onClick={closeProject} disabled={projectBusy}>Close project</button>}
              {recentProjects.length > 0 && <span>RECENT PROJECTS</span>}
              {recentProjects.map((item) => (
                <button type="button" key={item.id} disabled={projectBusy} onClick={() => reopenProject(item.id)} title={item.root}>{item.name}</button>
              ))}
              {backendStatus.kind !== 'ready' && <span>Project folders require the desktop app.</span>}
            </div>
          )}
        </div>

        <label className="global-search">
          <Search size={17} />
          <input
            ref={searchInput}
            aria-label="Search project map"
            placeholder="Search components, files, or ask anything..."
            value={searchText}
            onChange={(event) => setSearchText(event.target.value)}
            onKeyDown={handleSearch}
          />
          <kbd>⌘ K</kbd>
        </label>

        <div className="topbar-actions">
          <button className="button button-subtle share-button" type="button" disabled title="Team sharing is not part of this prototype">
            <Link2 size={15} /> Share
          </button>
          <button className="button button-primary deploy-button" type="button" disabled={!project || !agentVersion} onClick={() => { setActiveWorkspaceTab('chat'); requestAnimationFrame(() => composerInput.current?.focus()) }}>
            <Sparkles size={15} /> Talk to Codex
          </button>
          <button className="avatar-button" type="button" aria-label="User profile">J</button>
        </div>
      </header>

      <div className="workspace">
        <aside className="sidebar">
          <nav className="primary-nav" aria-label="Main navigation">
            {navItems.map(({ label, caption, icon: Icon }) => (
              <button
                className={`nav-item${label === 'Project Map' ? ' is-active' : ''}`}
                type="button"
                key={label}
                disabled={label !== 'Project Map'}
                title={label === 'Project Map' ? undefined : 'Available in a later development stage'}
              >
                <Icon size={18} strokeWidth={1.8} />
                <span className="nav-copy">
                  <strong>{label}</strong>
                  <small>{caption}</small>
                </span>
              </button>
            ))}
          </nav>

          <div className="project-tree">
            <div className="tree-heading">
              <span>PROJECT</span>
              <ChevronDown size={14} />
            </div>
            <button className="tree-row tree-root" type="button" onClick={() => setProjectTreeOpen((open) => !open)} title={project?.root}>
              {projectTreeOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
              {projectTreeOpen ? <FolderOpen size={16} /> : <Folder size={16} />}
              <span>{project?.name ?? 'Sample project'}</span>
            </button>
            {projectTreeOpen && !project && (
              <div className="tree-children">
                <button className="tree-row" type="button" onClick={() => setAppTreeOpen((open) => !open)}>
                  {appTreeOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                  {appTreeOpen ? <FolderOpen size={16} /> : <Folder size={16} />}
                  <span>app</span>
                </button>
                {appTreeOpen && (
                  <div className="tree-children tree-children-nested">
                    <div className="tree-row tree-static"><ChevronRight size={13} /><Folder size={15} /><span>src</span></div>
                    <div className="tree-row tree-static"><ChevronRight size={13} /><Folder size={15} /><span>src-tauri</span></div>
                  </div>
                )}
                <div className="tree-row tree-static"><ChevronRight size={13} /><Folder size={16} /><span>docs</span></div>
                <div className="tree-row tree-static"><ChevronRight size={13} /><Folder size={16} /><span>assets</span></div>
              </div>
            )}
            {projectTreeOpen && project && <div className="tree-path" title={project.root}>{project.root}</div>}
          </div>

          <div className="sidebar-footer">
            <div className="connection-indicator" role="status" title={backendStatus.kind === 'error' ? backendStatus.message : backendStatus.kind === 'ready' ? backendStatus.info.platform : undefined}>
              <span className={backendStatus.kind === 'ready' ? 'is-ready' : ''} />
              {backendStatus.kind === 'ready'
                ? `Desktop connected · v${backendStatus.info.version} · ${agentRunning ? 'Agent working' : agentVersion ? 'Codex available' : 'Agent unavailable'}`
                : backendStatus.kind === 'error'
                  ? 'Desktop backend unavailable · Agent offline'
                  : backendStatus.kind === 'browser'
                    ? 'Browser preview · Agent offline'
                    : 'Connecting to desktop backend…'}
            </div>
            <button className="footer-action" type="button" disabled title="Settings are not available in this prototype">
              <Settings2 size={16} /> Settings
            </button>
            <button className="footer-action" type="button" disabled title="Help is not available in this prototype">
              <CircleHelp size={16} /> Help & feedback
            </button>
          </div>
        </aside>

        <section className="main-column" aria-label="Project map and agent workspace">
          <div className="map-toolbar">
            <div className="map-heading-icon"><Workflow size={21} /></div>
            <div className="map-heading-copy">
              <h1>Project Map</h1>
              <p>Explore the shared plan and discuss each part with your agent.</p>
            </div>
            <div className="map-toolbar-actions">
              <button className="button button-outline" type="button" onClick={addNode} disabled={!project || projectBusy}>
                <Plus size={16} /> Add node
              </button>
              <button className="button button-primary" type="button" onClick={saveMap} disabled={!project || !mapDirty || projectBusy}>Save map</button>
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
            <ReactFlow<HeimNode, Edge>
              nodes={nodes}
              edges={edges}
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
              nodesConnectable={!!project && !projectBusy}
              nodesDraggable={!!project && !projectBusy}
              elementsSelectable
              proOptions={{ hideAttribution: false }}
            >
              <Background variant={BackgroundVariant.Dots} gap={18} size={1} color="#dce2ec" />
              <Controls showInteractive={false} />
              <MiniMap
                pannable
                zoomable
                nodeColor={(node) => node.type === 'layer' ? '#e6eaf2' : '#dce7ff'}
                maskColor="rgba(248, 250, 253, 0.72)"
              />
            </ReactFlow>
            <div className="preview-watermark">{project ? (mapDirty ? 'Unsaved project map' : project.name) : 'Sample project map'}</div>
          </div>

          <section className="agent-panel" aria-label="Agent workspace">
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

            {activeWorkspaceTab === 'chat' && (
              <div className="chat-panel">
                <div className="chat-messages" aria-live="polite">
                  {messages.length === 0 ? (
                    <div className="chat-empty-state">
                      <div className="empty-state-icon"><Sparkles size={17} /></div>
                      <div>
                        <strong>Discuss this part of your project</strong>
                        <p>{project ? 'Select a map node for focused context, then message Codex.' : 'Open a project to chat with the agent.'}</p>
                      </div>
                      <span className="preview-note">{agentVersion || agentError || 'Desktop agent required'}</span>
                    </div>
                  ) : messages.map((message) => (
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
                  {mapDirty && <p>Save your map edits before accepting this proposal. If the saved map changed, request a new review.</p>}
                  <div><button className="button button-outline" type="button" onClick={() => decideMapProposal(false)} disabled={projectBusy}>Reject map update</button>
                    <button className="button button-primary" type="button" onClick={() => decideMapProposal(true)} disabled={projectBusy || mapDirty}>Apply map update</button></div>
                </div>}
                {!reviewingMap && mapProposals.length === 0 && reviewFiles.length > 0 && <div className="map-review">
                  <span>Changed files: {reviewFiles.join(' · ')}</span>
                  {reviewError && <strong role="alert">{reviewError}</strong>}
                  <button className="button button-outline" type="button" onClick={reviewMapChanges} disabled={mapDirty}>Review map alignment</button>
                  {mapDirty && <small>Save the map first.</small>}
                </div>}
                <form className="composer" onSubmit={sendMessage}>
                  <button className="composer-attach" type="button" disabled title="Attachments are not available yet">
                    <Plus size={17} />
                  </button>
                  <textarea
                    ref={composerInput}
                    aria-label="Message the selected project component"
                    placeholder={`Message ${activeNodeData?.title ?? 'the project'}... (⌘ ↵ to send)`}
                    value={draft}
                    rows={1}
                    disabled={!project || !agentVersion || agentRunning || reviewingMap || mapDirty || mapProposals.length > 0 || !!reviewError}
                    onChange={(event) => setDraft(event.target.value)}
                    onKeyDown={handleComposerKeyDown}
                  />
                  <button className="send-button" type="submit" disabled={!draft.trim() || !project || !agentVersion || agentRunning || reviewingMap || mapDirty || mapProposals.length > 0 || !!reviewError} aria-label="Send message">
                    <Send size={17} />
                  </button>
                </form>
              </div>
            )}

            {activeWorkspaceTab === 'tasks' && (
              <div className="workspace-content task-list">
                <div className="workspace-content-heading"><ListChecks size={17} /><strong>Responsibilities for {activeNodeData?.title ?? 'this project'}</strong></div>
                {(activeNodeData?.responsibilities ?? []).map((task) => (
                  <div className="task-row" key={task}><span className="task-checkbox" /><span>{task}</span><small>Not started</small></div>
                ))}
                {!activeNodeData && <p className="muted-copy">Select a component to see its responsibilities.</p>}
              </div>
            )}

            {activeWorkspaceTab === 'context' && (
              <div className="workspace-content context-content">
                <div className="workspace-content-heading"><FileCode2 size={17} /><strong>Context for this conversation</strong></div>
                <div className="context-card"><span>SELECTED COMPONENT</span><strong>{activeNodeData?.title ?? 'No component selected'}</strong><p>{activeNodeData?.summary ?? 'Choose a node on the map to scope the conversation.'}</p></div>
                <div className="context-card context-card-inline"><span>RELATED COMPONENTS</span><strong>{relatedNodes.length ? relatedNodes.map((node) => node.data.title).join(' · ') : 'None selected'}</strong></div>
                <p className="context-footnote">The selected node and related titles are sent with each message to Codex.</p>
              </div>
            )}

            {activeWorkspaceTab === 'terminal' && (
              <div className="workspace-content terminal-content">
                <div className="terminal-heading"><SquareTerminal size={17} /><strong>Agent activity</strong><span className="terminal-state">{agentRunning ? 'Running' : 'Idle'}</span></div>
                {messages.filter((message) => message.role === 'status').map((message) => <p key={message.id}>{message.text}</p>)}
                {changedFiles.length > 0 && <p>Changed files: {changedFiles.join(' · ')}</p>}
              </div>
            )}
          </section>
        </section>

        <aside className="details-panel" aria-label="Selected component details">
          <div className="details-summary">
            <div className="details-icon">{activeNodeData ? (() => { const Icon = iconByName[activeNodeData.icon]; return <Icon size={23} /> })() : <Workflow size={23} />}</div>
            <div className="details-title-copy">
              <h2>{activeNodeData?.title ?? 'Project Map'}</h2>
              <span className="status-badge"><span />{activeNodeData?.status ?? 'No selection'}</span>
            </div>
            <button className="button button-outline" type="button" onClick={editNode} disabled={!project || !activeNodeData || projectBusy}>Edit</button>
          </div>

          <div className="detail-tabs" role="tablist" aria-label="Component details">
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
          </div>

          <div className="detail-scroll-area">
            {activeDetailTab === 'overview' && activeNodeData && (
              <div className="detail-section">
                {editingNodeId === selectedNodeId && (
                  <form className="node-editor" onSubmit={updateNode}>
                    <label>Title<input value={nodeTitle} onChange={(event) => setNodeTitle(event.target.value)} maxLength={120} required /></label>
                    <label>Summary<textarea value={nodeSummary} onChange={(event) => setNodeSummary(event.target.value)} rows={4} /></label>
                    <div><button className="button button-primary" type="submit">Apply changes</button><button className="button button-outline" type="button" onClick={() => setEditingNodeId(null)}>Cancel</button></div>
                  </form>
                )}
                <h3>Summary</h3>
                <p className="detail-summary-text">{activeNodeData.summary}</p>
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
                <RelationshipList relatedNodes={relatedNodes} onSelect={(id) => { setSelectedNodeId(id); setActiveDetailTab('overview') }} />
              </div>
            )}

            {activeDetailTab === 'files' && (
              <div className="detail-section">
                <h3>Linked files</h3>
                {activeNodeData?.files.length ? activeNodeData.files.map((file) => (
                  <div className="file-row" key={file}><Code2 size={16} /><span>{file}</span><ChevronRight size={14} /></div>
                )) : <div className="detail-empty"><FileCode2 size={20} /><strong>No files linked yet</strong><p>Files can be connected to a map component as development progresses.</p></div>}
              </div>
            )}

            {activeDetailTab === 'tasks' && (
              <div className="detail-section">
                <h3>Development tasks</h3>
                {(activeNodeData?.responsibilities ?? []).map((task) => (
                  <div className="detail-task" key={task}><span className="task-checkbox" /><div><strong>{task}</strong><small>Not started</small></div></div>
                ))}
                {!activeNodeData && <div className="detail-empty"><ListChecks size={20} /><strong>Select a component</strong><p>Task details are linked to individual map nodes.</p></div>}
              </div>
            )}

            {activeDetailTab === 'relationships' && (
              <div className="detail-section">
                <h3>Connected components</h3>
                <RelationshipList relatedNodes={relatedNodes} onSelect={(id) => { setSelectedNodeId(id); setActiveDetailTab('overview') }} />
                {relatedNodes.length === 0 && <div className="detail-empty"><GitBranch size={20} /><strong>No relationships</strong><p>Connect this component to another node on the map.</p></div>}
              </div>
            )}
          </div>

          <div className="details-footer"><PanelRightClose size={15} /><span>Details are scoped to the selected map node</span></div>
        </aside>
      </div>

      {notice && <div className="toast-notice" role="status"><Check size={16} />{notice}<button type="button" onClick={() => setNotice('')} aria-label="Dismiss notification"><X size={15} /></button></div>}
    </main>
  )
}

function RelationshipList({ relatedNodes, onSelect }: { relatedNodes: ComponentFlowNode[]; onSelect: (id: string) => void }) {
  if (relatedNodes.length === 0) {
    return <p className="muted-copy">No connected components yet.</p>
  }

  return (
    <div className="relationship-list">
      {relatedNodes.map((node) => {
        const Icon = iconByName[node.data.icon]
        return (
          <button type="button" className="relationship-row" key={node.id} onClick={() => onSelect(node.id)}>
            <span className="relationship-icon"><Icon size={15} /></span>
            <span className="relationship-name">{node.data.title}</span>
            <span className="relationship-type">Related</span>
          </button>
        )
      })}
    </div>
  )
}

export default App
